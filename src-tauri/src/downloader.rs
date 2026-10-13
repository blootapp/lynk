//! Download queue and yt-dlp process supervision.
//!
//! The backend owns download state: it persists every status change itself
//! (in order, from the thread that observed it) and pushes `download:update`
//! events to the UI. The UI never writes download state back.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tauri::window::{ProgressBarState, ProgressBarStatus};
use tauri::{AppHandle, Emitter, Manager as _};
use tauri_plugin_notification::NotificationExt;
use uuid::Uuid;

use crate::db::Db;
use crate::engine::Engine;
use crate::model::{now_ms, Download, NewDownload, PlaylistEntry, PlaylistPreview};
use crate::parse::{self, ArgSpec, Line, Meta, Tracker, PLAYLIST_LIMIT};
use crate::process::{self, Killer};
use crate::staging;

const EMIT_INTERVAL: Duration = Duration::from_millis(200);
const INDICATOR_INTERVAL: Duration = Duration::from_millis(500);

pub struct Manager {
    app: AppHandle,
    db: Arc<Db>,
    engine: Arc<Engine>,
    inner: Mutex<Inner>,
    shutting_down: AtomicBool,
    last_indicator: Mutex<Option<Instant>>,
}

#[derive(Default)]
struct Inner {
    /// Everything not yet finished, plus playlist prompts awaiting an answer.
    active: HashMap<String, Download>,
    queue: VecDeque<String>,
    running: HashMap<String, Running>,
    playlists: HashMap<String, Vec<PlaylistEntry>>,
    max_concurrent: usize,
}

struct Running {
    cancel: Arc<AtomicBool>,
    /// Removed by the user while running: delete instead of recording a result.
    discard: Arc<AtomicBool>,
    killer: Option<Killer>,
}

enum Outcome {
    Done { file: Option<PathBuf> },
    Playlist { title: String, entries: Vec<PlaylistEntry>, total: Option<i64>, nested: usize },
    Failed { message: String, detail: Option<String> },
    Cancelled,
}

#[derive(Default)]
struct RunState {
    tracker: Tracker,
    entries: Vec<Meta>,
    files: Vec<String>,
    errors: Vec<String>,
    last_stderr: Option<String>,
    expected: Option<String>,
    downloading: bool,
    last_emit: Option<Instant>,
}

impl Manager {
    pub fn new(app: AppHandle, db: Arc<Db>, engine: Arc<Engine>) -> Arc<Self> {
        let max = db
            .pref("max_concurrent")
            .and_then(|v| v.parse().ok())
            .unwrap_or(3usize)
            .clamp(1, 5);
        Arc::new(Manager {
            app,
            db,
            engine,
            inner: Mutex::new(Inner { max_concurrent: max, ..Default::default() }),
            shutting_down: AtomicBool::new(false),
            last_indicator: Mutex::new(None),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn emit(&self, d: &Download) {
        let _ = self.app.emit("download:update", d);
    }

    fn emit_removed(&self, id: &str) {
        let _ = self.app.emit("download:removed", id);
    }

    /// In-memory records (with live progress) for everything unfinished.
    pub fn snapshot(&self) -> Vec<Download> {
        self.lock().active.values().cloned().collect()
    }

    // ── Public operations ────────────────────────────────────────────────────

    pub fn enqueue(self: &Arc<Self>, items: Vec<NewDownload>) -> Result<Vec<Download>, String> {
        let now = now_ms();
        let records: Vec<Download> = items
            .into_iter()
            .enumerate()
            .map(|(i, n)| Download {
                id: Uuid::new_v4().to_string(),
                ext: Some(if n.format == "audio" { n.audio_format.clone() } else { n.video_format.clone() }),
                url: n.url,
                format: n.format,
                quality: n.quality,
                audio_format: n.audio_format,
                video_format: n.video_format,
                prefer_h264: n.prefer_h264,
                save_path: Some(n.save_path),
                status: "queued".into(),
                // Newest-first lists then show a pasted batch in pasted order.
                created_at: now - i as i64,
                ..Default::default()
            })
            .collect();
        self.db.save_many(&records).map_err(|e| e.to_string())?;
        {
            let mut inner = self.lock();
            for d in &records {
                inner.active.insert(d.id.clone(), d.clone());
                inner.queue.push_back(d.id.clone());
            }
        }
        for d in &records {
            self.emit(d);
        }
        self.pump();
        self.refresh_indicators(true);
        Ok(records)
    }

    pub fn retry(self: &Arc<Self>, id: &str) -> Result<Download, String> {
        if self.lock().active.contains_key(id) {
            return Err("This download is already running.".into());
        }
        let mut d = self
            .db
            .get(id)
            .map_err(|e| e.to_string())?
            .ok_or("This download no longer exists.")?;
        d.reset_for_run();
        d.created_at = now_ms();
        self.db.save(&d).map_err(|e| e.to_string())?;
        {
            let mut inner = self.lock();
            inner.active.insert(d.id.clone(), d.clone());
            inner.queue.push_back(d.id.clone());
        }
        self.emit(&d);
        self.pump();
        self.refresh_indicators(true);
        Ok(d)
    }

    pub fn cancel(self: &Arc<Self>, id: &str) {
        let queued = {
            let mut inner = self.lock();
            if let Some(pos) = inner.queue.iter().position(|q| q == id) {
                inner.queue.remove(pos);
                inner.active.remove(id)
            } else {
                if let Some(r) = inner.running.get(id) {
                    r.cancel.store(true, Ordering::SeqCst);
                    if let Some(k) = &r.killer {
                        k.kill();
                    }
                }
                None
            }
        };
        if let Some(mut d) = queued {
            mark_finished(&mut d, "cancelled");
            discard_staging(&d);
            let _ = self.db.save(&d);
            self.emit(&d);
            self.refresh_indicators(true);
        }
    }

    /// Cancels every unfinished item of a playlist in one step. Queued items
    /// are dropped first so a freed slot can't start the next one.
    pub fn cancel_group(self: &Arc<Self>, group_id: &str) {
        let in_group = |d: Option<&Download>| d.is_some_and(|d| d.group_id.as_deref() == Some(group_id));
        let (queued, killers) = {
            let mut inner = self.lock();
            let ids: Vec<String> = inner
                .queue
                .iter()
                .filter(|id| in_group(inner.active.get(*id)))
                .cloned()
                .collect();
            inner.queue.retain(|id| !ids.contains(id));
            let queued: Vec<Download> = ids.iter().filter_map(|id| inner.active.remove(id)).collect();
            let mut killers = Vec::new();
            for (id, r) in inner.running.iter() {
                if in_group(inner.active.get(id)) {
                    r.cancel.store(true, Ordering::SeqCst);
                    if let Some(k) = &r.killer {
                        killers.push(k.clone());
                    }
                }
            }
            (queued, killers)
        };
        for k in killers {
            k.kill();
        }
        for mut d in queued {
            mark_finished(&mut d, "cancelled");
            discard_staging(&d);
            let _ = self.db.save(&d);
            self.emit(&d);
        }
        self.refresh_indicators(true);
    }

    /// Removes a download from history, stopping it first if needed.
    pub fn remove(self: &Arc<Self>, id: &str) {
        let mut delete_now = true;
        let mut record = None;
        {
            let mut inner = self.lock();
            inner.playlists.remove(id);
            if let Some(pos) = inner.queue.iter().position(|q| q == id) {
                inner.queue.remove(pos);
            }
            if let Some(r) = inner.running.get(id) {
                // The run thread deletes the row once the process is gone,
                // so a late status write can't resurrect it.
                r.discard.store(true, Ordering::SeqCst);
                r.cancel.store(true, Ordering::SeqCst);
                if let Some(k) = &r.killer {
                    k.kill();
                }
                delete_now = false;
            } else {
                record = inner.active.remove(id);
            }
        }
        if delete_now {
            if let Some(d) = record.or_else(|| self.db.get(id).ok().flatten()) {
                discard_staging(&d);
            }
            let _ = self.db.delete(id);
            self.emit_removed(id);
            self.refresh_indicators(true);
        }
    }

    pub fn clear_history(&self) -> Result<Vec<String>, String> {
        let removed = self.db.clear_finished().map_err(|e| e.to_string())?;
        for d in &removed {
            discard_staging(d);
        }
        Ok(removed.into_iter().map(|d| d.id).collect())
    }

    /// Startup: drop staging folders that no failed download could resume.
    pub fn sweep_staging(&self) {
        let Ok(rows) = self.db.list() else { return };
        let keep = rows
            .iter()
            .filter(|d| d.status == "error")
            .map(|d| d.id.clone())
            .collect();
        let mut dirs: Vec<PathBuf> =
            rows.iter().filter_map(|d| d.save_path.as_ref().map(PathBuf::from)).collect();
        if let Some(p) = self.db.pref("save_path") {
            dirs.push(PathBuf::from(p));
        }
        dirs.sort();
        dirs.dedup();
        staging::sweep(dirs, &keep);
    }

    pub fn set_max_concurrent(self: &Arc<Self>, n: usize) {
        self.lock().max_concurrent = n.clamp(1, 5);
        self.pump();
    }

    /// Turns a detected playlist into one queued download per entry, saved
    /// into a subfolder named after the playlist.
    pub fn confirm_playlist(self: &Arc<Self>, id: &str) -> Result<usize, String> {
        let (parent, entries) = {
            let mut inner = self.lock();
            let entries = inner
                .playlists
                .remove(id)
                .ok_or("This playlist prompt has expired. Paste the link again.")?;
            let parent = inner.active.remove(id).ok_or("Playlist not found.")?;
            (parent, entries)
        };
        let title = parent
            .playlist
            .as_ref()
            .map(|p| p.title.clone())
            .or_else(|| parent.title.clone())
            .unwrap_or_else(|| "Playlist".into());
        let base = parent.save_path.clone().unwrap_or_default();
        let dir = Path::new(&base).join(parse::sanitize_component(&title));
        let dir = dir.to_string_lossy().to_string();
        let count = entries.len() as i64;
        let width = entries
            .iter()
            .map(|e| e.index)
            .max()
            .unwrap_or(count)
            .to_string()
            .len()
            .max(2);
        let now = now_ms();

        let children: Vec<Download> = entries
            .into_iter()
            .enumerate()
            .map(|(i, e)| Download {
                id: Uuid::new_v4().to_string(),
                url: e.url,
                title: e.title,
                thumbnail: e.thumbnail,
                duration: e.duration,
                site: parent.site.clone(),
                format: parent.format.clone(),
                quality: parent.quality.clone(),
                audio_format: parent.audio_format.clone(),
                video_format: parent.video_format.clone(),
                prefer_h264: parent.prefer_h264,
                ext: parent.ext.clone(),
                save_path: Some(dir.clone()),
                status: "queued".into(),
                // Item 1 sorts first in newest-first history.
                created_at: now - i as i64,
                group_id: Some(parent.id.clone()),
                group_title: Some(title.clone()),
                group_index: Some(e.index),
                group_count: Some(count),
                name_prefix: Some(format!("{:0width$} - ", e.index)),
                ..Default::default()
            })
            .collect();

        self.db.save_many(&children).map_err(|e| e.to_string())?;
        let _ = self.db.delete(&parent.id);
        self.emit_removed(&parent.id);
        {
            let mut inner = self.lock();
            for c in &children {
                inner.active.insert(c.id.clone(), c.clone());
                inner.queue.push_back(c.id.clone());
            }
        }
        for c in &children {
            self.emit(c);
        }
        self.pump();
        self.refresh_indicators(true);
        Ok(children.len())
    }

    /// Called when quitting: stop everything and leave it retryable. Partial
    /// files are kept so a retry resumes where it stopped.
    pub fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::SeqCst) {
            return;
        }
        let (records, killers) = {
            let mut inner = self.lock();
            inner.queue.clear();
            let records: Vec<Download> =
                inner.active.values().filter(|d| d.is_active()).cloned().collect();
            let killers: Vec<Killer> =
                inner.running.values().filter_map(|r| r.killer.clone()).collect();
            (records, killers)
        };
        for k in killers {
            k.kill();
        }
        for mut d in records {
            mark_finished(&mut d, "error");
            d.error_msg = Some("Interrupted".into());
            let _ = self.db.save(&d);
        }
    }

    // ── Scheduling ───────────────────────────────────────────────────────────

    fn pump(self: &Arc<Self>) {
        loop {
            let next = {
                let mut inner = self.lock();
                if self.shutting_down.load(Ordering::SeqCst)
                    || inner.running.len() >= inner.max_concurrent
                {
                    return;
                }
                let Some(id) = inner.queue.pop_front() else { return };
                let Some(d) = inner.active.get_mut(&id) else { continue };
                d.status = "fetching".into();
                d.stage = Some("fetching".into());
                let snapshot = d.clone();
                let cancel = Arc::new(AtomicBool::new(false));
                let discard = Arc::new(AtomicBool::new(false));
                inner.running.insert(
                    id.clone(),
                    Running { cancel: cancel.clone(), discard: discard.clone(), killer: None },
                );
                (snapshot, cancel, discard)
            };
            let (rec, cancel, discard) = next;
            self.emit(&rec);
            let me = self.clone();
            std::thread::spawn(move || {
                let outcome = me.execute(&rec, &cancel);
                me.finish(&rec.id, outcome, &cancel, &discard);
                me.pump();
            });
        }
    }

    // ── One run of yt-dlp ────────────────────────────────────────────────────

    fn execute(&self, rec: &Download, cancel: &AtomicBool) -> Outcome {
        let fail = |message: &str, detail: Option<String>| Outcome::Failed {
            message: message.to_string(),
            detail,
        };
        // An outdated yt-dlp is the most common cause of failures, so let an
        // update that is already running finish first.
        if self.engine.is_updating() {
            self.update(&rec.id, true, |d| d.stage = Some("engine".into()));
            let started = Instant::now();
            while self.engine.is_updating()
                && started.elapsed() < Duration::from_secs(90)
                && !cancel.load(Ordering::SeqCst)
            {
                std::thread::sleep(Duration::from_millis(250));
            }
            if cancel.load(Ordering::SeqCst) {
                return Outcome::Cancelled;
            }
            self.update(&rec.id, true, |d| d.stage = Some("fetching".into()));
        }
        let Some(bin) = self.engine.ytdlp() else {
            return fail("The download engine (yt-dlp) is missing. Reinstall Lynk.", None);
        };
        let save_dir = PathBuf::from(rec.save_path.clone().unwrap_or_default());
        if save_dir.as_os_str().is_empty() {
            return fail("No save folder is set. Choose one in Settings.", None);
        }
        let work_dir = match std::fs::create_dir_all(&save_dir)
            .and_then(|_| staging::create(&save_dir, &rec.id))
        {
            Ok(d) => d,
            Err(e) => {
                return fail(
                    "Lynk can't write to the save folder. Choose another folder in Settings.",
                    Some(e.to_string()),
                )
            }
        };
        let dir = work_dir.to_string_lossy().to_string();

        let cookies = self.db.pref("cookies_browser");
        let ffmpeg = self.engine.ffmpeg();
        let args = parse::build_args(&ArgSpec {
            url: &rec.url,
            format: &rec.format,
            quality: &rec.quality,
            audio_format: &rec.audio_format,
            video_format: &rec.video_format,
            prefer_h264: rec.prefer_h264,
            dir: &dir,
            name_prefix: rec.name_prefix.as_deref(),
            ffmpeg: ffmpeg.as_deref(),
            cookies_browser: cookies.as_deref(),
        });

        let mut cmd = process::command(&bin);
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (mut child, killer) = match process::spawn(cmd) {
            Ok(x) => x,
            Err(e) => return fail("Couldn't start the download engine.", Some(e.to_string())),
        };
        if let Some(r) = self.lock().running.get_mut(&rec.id) {
            r.killer = Some(killer.clone());
        }
        if cancel.load(Ordering::SeqCst) {
            killer.kill();
        }

        let (tx, rx) = mpsc::channel();
        if let Some(out) = child.stdout.take() {
            process::forward_lines(out, tx.clone(), false);
        }
        if let Some(err) = child.stderr.take() {
            process::forward_lines(err, tx, true);
        }

        let mut st = RunState::default();
        for (is_err, line) in rx.iter() {
            self.handle_line(&rec.id, &mut st, is_err, &line);
        }
        let status = child.wait();
        drop(killer);

        if cancel.load(Ordering::SeqCst) {
            return Outcome::Cancelled;
        }
        let success = status.as_ref().map(|s| s.success()).unwrap_or(false);

        // A flat-listed playlist prints its entries and exits without
        // downloading anything.
        if success && !st.entries.is_empty() && !st.downloading && st.files.is_empty() {
            let first = &st.entries[0];
            let title = first.playlist_title.clone().unwrap_or_else(|| "Playlist".into());
            let total = first.playlist_count;
            let mut nested = 0;
            let entries = st
                .entries
                .iter()
                .filter_map(|m| {
                    if m.is_nested_playlist() {
                        nested += 1;
                        return None;
                    }
                    Some(PlaylistEntry {
                        url: m.url.clone().or_else(|| m.webpage_url.clone())?,
                        title: m.title.clone(),
                        thumbnail: m.thumbnail.clone(),
                        duration: m.duration,
                        index: m.playlist_index.unwrap_or(0),
                    })
                })
                .collect();
            return Outcome::Playlist { title, entries, total, nested };
        }

        let produced = st.files.iter().rev().map(PathBuf::from).find(|p| p.is_file());
        if success || produced.is_some() {
            let file = produced.or_else(|| expected_outputs(rec, &st.expected).into_iter().find(|p| p.is_file()));
            return deliver(file, &save_dir, &rec.id);
        }

        let raw = st
            .errors
            .last()
            .cloned()
            .or(st.last_stderr)
            .unwrap_or_else(|| match status {
                Ok(s) => format!("yt-dlp exited with code {}", s.code().unwrap_or(-1)),
                Err(e) => e.to_string(),
            });
        // Tagging/conversion failed but the media itself is on disk: keep it.
        if raw.to_lowercase().contains("postprocessing") {
            if let Some(f) = expected_outputs(rec, &st.expected).into_iter().find(|p| p.is_file()) {
                return deliver(Some(f), &save_dir, &rec.id);
            }
        }
        Outcome::Failed { message: parse::humanize_error(&raw), detail: Some(parse::clean_error(&raw)) }
    }

    fn handle_line(&self, id: &str, st: &mut RunState, is_err: bool, line: &str) {
        match parse::parse_line(line) {
            Line::Meta(m) => {
                st.tracker.on_meta(&m);
                if m.filename.is_some() {
                    st.expected = m.filename.clone();
                }
                if m.playlist_index.is_some() && !st.downloading {
                    // Listing a playlist: show its name and a running count.
                    st.entries.push((*m).clone());
                    let n = st.entries.len();
                    self.update(id, n == 1 || n.is_multiple_of(20), |d| {
                        d.stage = Some("listing".into());
                        if let Some(t) = &m.playlist_title {
                            d.title = Some(t.clone());
                        }
                        d.playlist = Some(PlaylistPreview {
                            title: d.title.clone().unwrap_or_default(),
                            count: n,
                            truncated: false,
                            nested: 0,
                        });
                    });
                } else {
                    if m.playlist_index.is_some() {
                        st.entries.push((*m).clone());
                    }
                    self.update(id, true, |d| {
                        let in_group = d.group_id.is_some();
                        if !in_group || d.title.is_none() {
                            d.title = m.title.clone().or(d.title.take());
                        }
                        d.thumbnail = m.thumbnail.clone().or(d.thumbnail.take());
                        d.duration = m.duration.or(d.duration);
                        d.site = m.extractor_key.clone().or(d.site.take());
                        d.playlist = None;
                    });
                    let snapshot = self.lock().active.get(id).cloned();
                    if let Some(d) = snapshot {
                        let _ = self.db.save(&d);
                    }
                }
            }
            Line::Sizes { first, second, thumb } => {
                st.tracker.on_sizes(first, second);
                if let Some(t) = thumb {
                    self.update(id, false, |d| {
                        if d.thumbnail.is_none() {
                            d.thumbnail = Some(t);
                        }
                    });
                }
            }
            Line::Progress(p) => {
                st.downloading = true;
                let stage = st.tracker.on_progress(&p);
                let progress = st.tracker.progress;
                let now = Instant::now();
                let due = p.finished || st.last_emit.is_none_or(|t| now - t >= EMIT_INTERVAL);
                if due {
                    st.last_emit = Some(now);
                }
                self.update(id, due, |d| {
                    d.status = "downloading".into();
                    d.stage = Some(stage.into());
                    d.progress = progress;
                    d.speed = p.speed;
                    d.eta = p.eta;
                });
            }
            Line::PostProcess { started, name } => {
                if !started || (!st.downloading && !st.entries.is_empty()) {
                    return;
                }
                if let Some(stage) = parse::postprocess_stage(&name) {
                    self.update(id, true, |d| {
                        d.status = "processing".into();
                        d.stage = Some(stage.into());
                        d.progress = d.progress.max(99.0);
                        d.speed = None;
                        d.eta = None;
                    });
                }
            }
            Line::File(f) => st.files.push(f),
            Line::Error(e) => st.errors.push(e),
            Line::Other => {
                let t = line.trim();
                if is_err && !t.is_empty() && !t.starts_with("LYNK_") {
                    st.last_stderr = Some(t.trim_start_matches("WARNING: ").to_string());
                }
            }
        }
    }

    /// Mutates an in-flight record; emits a snapshot when `emit` is set.
    fn update(&self, id: &str, emit: bool, f: impl FnOnce(&mut Download)) {
        let snapshot = {
            let mut inner = self.lock();
            let Some(d) = inner.active.get_mut(id) else { return };
            f(d);
            emit.then(|| d.clone())
        };
        if let Some(d) = snapshot {
            self.emit(&d);
            self.refresh_indicators(false);
        }
    }

    fn finish(&self, id: &str, outcome: Outcome, cancel: &AtomicBool, discard: &AtomicBool) {
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        let Some(mut d) = ({
            let mut inner = self.lock();
            inner.running.remove(id);
            inner.active.remove(id)
        }) else {
            return;
        };

        if matches!(outcome, Outcome::Cancelled) {
            discard_staging(&d);
        }
        if discard.load(Ordering::SeqCst) {
            let _ = self.db.delete(id);
            self.emit_removed(id);
            self.refresh_indicators(true);
            return;
        }

        match outcome {
            Outcome::Done { file } => {
                mark_finished(&mut d, "done");
                d.progress = 100.0;
                if let Some(f) = &file {
                    d.ext = f.extension().map(|e| e.to_string_lossy().to_lowercase()).or(d.ext.take());
                    d.size_bytes = std::fs::metadata(f).ok().map(|m| m.len() as i64);
                    d.save_path = f.parent().map(|p| p.to_string_lossy().to_string()).or(d.save_path.take());
                }
                d.file_path = file.map(|f| f.to_string_lossy().to_string());
            }
            Outcome::Playlist { title, entries, total, nested } => {
                // Listing never downloads anything; nothing to resume.
                discard_staging(&d);
                let problem = if d.group_id.is_some() {
                    Some("This item is a playlist itself. Paste its link on its own.")
                } else if entries.is_empty() && nested > 0 {
                    Some("This is a channel page. Open its Videos tab and paste that link instead.")
                } else if entries.is_empty() {
                    Some("This playlist is empty.")
                } else {
                    None
                };
                if let Some(msg) = problem {
                    mark_finished(&mut d, "error");
                    d.error_msg = Some(msg.into());
                } else {
                    d.status = "playlist".into();
                    d.stage = None;
                    d.title = Some(title.clone());
                    if d.thumbnail.is_none() {
                        d.thumbnail = entries.iter().find_map(|e| e.thumbnail.clone());
                    }
                    let truncated = match total {
                        Some(t) => t as usize > entries.len() + nested,
                        None => entries.len() + nested >= PLAYLIST_LIMIT,
                    };
                    d.playlist = Some(PlaylistPreview { title, count: entries.len(), truncated, nested });
                    let _ = self.db.save(&d);
                    {
                        let mut inner = self.lock();
                        inner.playlists.insert(d.id.clone(), entries);
                        inner.active.insert(d.id.clone(), d.clone());
                    }
                    self.emit(&d);
                    self.refresh_indicators(true);
                    return;
                }
            }
            Outcome::Failed { message, detail } => {
                // Keep partial files for a resuming retry, but not an empty folder.
                if let Some(dir) = &d.save_path {
                    staging::remove_if_empty(Path::new(dir), &d.id);
                }
                mark_finished(&mut d, "error");
                d.error_msg = Some(message);
                d.error_detail = detail;
            }
            Outcome::Cancelled => {
                mark_finished(&mut d, "cancelled");
            }
        }
        // A cancel that raced a natural finish still reads as cancelled.
        if cancel.load(Ordering::SeqCst) && d.status != "done" {
            d.status = "cancelled".into();
            d.error_msg = None;
        }

        let _ = self.db.save(&d);
        self.emit(&d);
        self.refresh_indicators(true);
        self.notify(&d);
    }

    // ── Feedback outside the window ──────────────────────────────────────────

    fn notify(&self, d: &Download) {
        if d.status != "done" && d.status != "error" {
            return;
        }
        if self.db.pref("notifications").as_deref() == Some("false") {
            return;
        }
        let focused = self
            .app
            .get_webview_window("main")
            .map(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false))
            .unwrap_or(false);
        if focused {
            return;
        }

        let (title, body) = if let Some(gid) = &d.group_id {
            // One notification per playlist, when its last item finishes.
            let still_running = self
                .lock()
                .active
                .values()
                .any(|o| o.group_id.as_deref() == Some(gid.as_str()));
            if still_running {
                return;
            }
            let (done, failed) = self.db.group_counts(gid);
            let name = d.group_title.clone().unwrap_or_else(|| "Playlist".into());
            let failed = if failed > 0 { format!(", {failed} failed") } else { String::new() };
            ("Playlist finished".to_string(), format!("{name} · {done} downloaded{failed}"))
        } else if d.status == "done" {
            ("Download complete".to_string(), d.title.clone().unwrap_or_else(|| d.url.clone()))
        } else {
            (
                "Download failed".to_string(),
                format!(
                    "{} — {}",
                    d.title.clone().unwrap_or_else(|| d.url.clone()),
                    d.error_msg.clone().unwrap_or_default()
                ),
            )
        };
        let _ = self.app.notification().builder().title(title).body(body).show();
    }

    /// Taskbar progress and tray tooltip.
    pub fn refresh_indicators(&self, force: bool) {
        {
            let mut last = self.last_indicator.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            if !force && last.is_some_and(|t| now - t < INDICATOR_INTERVAL) {
                return;
            }
            *last = Some(now);
        }
        let (count, sum, any_bytes) = {
            let inner = self.lock();
            inner
                .active
                .values()
                .filter(|d| d.is_active())
                .fold((0usize, 0f64, false), |(c, s, b), d| {
                    (c + 1, s + d.progress, b || d.status == "downloading" || d.status == "processing")
                })
        };

        if let Some(w) = self.app.get_webview_window("main") {
            let state = if count == 0 {
                ProgressBarState { status: Some(ProgressBarStatus::None), progress: None }
            } else if !any_bytes {
                ProgressBarState { status: Some(ProgressBarStatus::Indeterminate), progress: None }
            } else {
                ProgressBarState {
                    status: Some(ProgressBarStatus::Normal),
                    progress: Some((sum / count as f64).round().clamp(0.0, 100.0) as u64),
                }
            };
            let _ = w.set_progress_bar(state);
        }
        if let Some(tray) = self.app.tray_by_id("main") {
            let tip = if count == 0 {
                "lynk".to_string()
            } else {
                format!("lynk — {count} downloading · {}%", (sum / count as f64).round() as i64)
            };
            let _ = tray.set_tooltip(Some(tip));
        }
    }
}

fn mark_finished(d: &mut Download, status: &str) {
    d.status = status.into();
    d.stage = None;
    d.speed = None;
    d.eta = None;
    d.playlist = None;
    d.finished_at = Some(now_ms());
}

/// Where the finished file should be, from the filename yt-dlp announced
/// before downloading (its extension changes after audio extraction).
fn expected_outputs(rec: &Download, expected: &Option<String>) -> Vec<PathBuf> {
    let Some(e) = expected else { return Vec::new() };
    let p = PathBuf::from(e);
    let mut out = Vec::new();
    if rec.format == "audio" {
        out.push(p.with_extension(&rec.audio_format));
    } else {
        out.push(p.with_extension(&rec.video_format));
    }
    out.push(p);
    out
}

/// Moves the finished file out of its staging folder into the save folder.
fn deliver(file: Option<PathBuf>, save_dir: &Path, id: &str) -> Outcome {
    let Some(file) = file else {
        staging::remove(save_dir, id);
        return Outcome::Done { file: None };
    };
    match staging::move_into(&file, save_dir) {
        Ok(target) => {
            staging::remove(save_dir, id);
            Outcome::Done { file: Some(target) }
        }
        Err(e) => Outcome::Failed {
            message: "Downloaded, but couldn't move the file into the save folder.".into(),
            detail: Some(e.to_string()),
        },
    }
}

fn discard_staging(d: &Download) {
    if let Some(dir) = &d.save_path {
        staging::remove(Path::new(dir), &d.id);
    }
}
