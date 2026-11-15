//! Locating and updating the yt-dlp / ffmpeg binaries.
//!
//! Downloads run Lynk's own copy of yt-dlp in the app data folder, which Lynk
//! keeps current: sites like YouTube break old yt-dlp versions within weeks,
//! and the install directory may not be writable.
//!
//! On Windows that copy is yt-dlp's unpacked build (`yt-dlp_win.zip`: the exe
//! plus its `_internal` folder). The single-file `yt-dlp.exe` unpacks ~40 MB
//! into a new temp folder on every run, which antivirus then scans again:
//! seconds per download on slower PCs, all spent before "Fetching info" can
//! end. Each version gets its own folder, `bin/yt-dlp/<version>`, and
//! `bin/yt-dlp/current` names the one to run. An update downloads and checks
//! a new folder, then switches that one line, so downloads never wait for it.
//!
//! On macOS it is the single-file build, updated in place with `yt-dlp -U`;
//! downloads wait while that replaces the file they run.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

use crate::db::{self, Db};
use crate::model::now_ms;
use crate::parse::version_newer;
use crate::process;

const CHECK_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000;
const RECHECK_MS: i64 = 60 * 60 * 1000;
const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

#[derive(Debug, Clone, Default, Serialize)]
pub struct EngineStatus {
    pub version: Option<String>,
    pub updating: bool,
    pub last_check: Option<i64>,
    pub error: Option<String>,
    /// Set on the status emitted right after a successful update.
    pub updated_to: Option<String>,
}

pub struct Engine {
    app: AppHandle,
    db: Arc<Db>,
    bundled_ytdlp: Option<PathBuf>,
    ffmpeg: Option<PathBuf>,
    /// Windows: the folder of versions (`bin/yt-dlp`); macOS: the binary.
    managed: PathBuf,
    /// Windows: the yt-dlp.exe downloads run, once `prepare` has chosen it.
    current: Mutex<Option<PathBuf>>,
    status: Mutex<EngineStatus>,
}

impl Engine {
    pub fn new(app: AppHandle, db: Arc<Db>) -> Self {
        let last_check = db.pref("ytdlp_last_check").and_then(|v| v.parse().ok());
        let version = db.pref("ytdlp_version").filter(|v| !v.is_empty());
        let bin = db::data_dir().join("bin");
        Engine {
            app,
            db,
            bundled_ytdlp: find_bundled_ytdlp(),
            ffmpeg: find_sidecar("ffmpeg"),
            managed: if cfg!(windows) { bin.join("yt-dlp") } else { bin.join(format!("yt-dlp{EXE}")) },
            current: Mutex::new(None),
            status: Mutex::new(EngineStatus { version, last_check, ..Default::default() }),
        }
    }

    /// The yt-dlp binary downloads should use.
    pub fn ytdlp(&self) -> Option<PathBuf> {
        if cfg!(windows) {
            let current = self.current.lock().unwrap_or_else(|e| e.into_inner()).clone();
            return current.filter(|p| p.is_file()).or_else(|| self.bundled_ytdlp.clone());
        }
        if self.managed.is_file() {
            Some(self.managed.clone())
        } else {
            self.bundled_ytdlp.clone()
        }
    }

    pub fn ffmpeg(&self) -> Option<PathBuf> {
        self.ffmpeg.clone()
    }

    pub fn is_updating(&self) -> bool {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).updating
    }

    /// Whether a download should wait for the running update: only where the
    /// update replaces the very file downloads run.
    pub fn blocks_downloads(&self) -> bool {
        !cfg!(windows) && self.is_updating()
    }

    pub fn status(&self) -> EngineStatus {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_status(&self, f: impl FnOnce(&mut EngineStatus)) {
        let snapshot = {
            let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut s);
            s.clone()
        };
        let _ = self.app.emit("engine:status", snapshot);
    }

    /// Startup work; runs on a background thread.
    pub fn prepare(&self) {
        let version = if cfg!(windows) { self.prepare_unpacked() } else { self.prepare_single_file() };
        if let Some(v) = &version {
            let _ = self.db.set_pref("ytdlp_version", v);
        }
        self.set_status(|s| s.version = version);
        self.update_if_due();
    }

    fn prepare_single_file(&self) -> Option<String> {
        let old = self.managed.with_file_name(format!("yt-dlp{EXE}.old"));
        let _ = std::fs::remove_file(old);

        let app_version = env!("CARGO_PKG_VERSION");
        let seeded_for = self.db.pref("ytdlp_seeded_for");
        if let Some(bundled) = &self.bundled_ytdlp {
            if !self.managed.is_file() {
                self.copy_bundled(bundled);
            } else if seeded_for.as_deref() != Some(app_version) {
                // A new Lynk build may ship a newer yt-dlp than the copy we
                // have been updating.
                if let (Some(b), Some(m)) = (read_version(bundled), read_version(&self.managed)) {
                    if version_newer(&b, &m) {
                        self.copy_bundled(bundled);
                    }
                }
            }
        }
        let _ = self.db.set_pref("ytdlp_seeded_for", app_version);
        self.ytdlp().and_then(|p| read_version(&p))
    }

    /// Picks the yt-dlp version to run: the newest of the installed one and
    /// the one this Lynk build ships, copied out of the install folder.
    fn prepare_unpacked(&self) -> Option<String> {
        let root = &self.managed;
        // Earlier builds kept the single-file yt-dlp.exe next to the folder.
        if let Some(bin) = root.parent() {
            for f in ["yt-dlp.exe", "yt-dlp.exe.old", "yt-dlp.exe.new", "yt-dlp.tmp"] {
                let _ = std::fs::remove_file(bin.join(f));
            }
        }

        let installed = unpacked::read_current(root).filter(|v| unpacked::exe(&root.join(v)).is_file());
        let bundled = self.bundled_ytdlp.as_deref();
        // Only an unpacked build can be copied as a folder (a development run
        // may have the single-file sidecar instead).
        let bundled_dir = bundled.and_then(Path::parent).filter(|d| d.join("_internal").is_dir());
        let bundled_version = bundled.and_then(read_version);

        let chosen = match (&installed, &bundled_version) {
            (Some(i), Some(b)) if version_newer(b, i) => None,
            (Some(i), _) => Some(i.clone()),
            (None, _) => None,
        };
        let chosen = chosen.or_else(|| {
            let (dir, version) = (bundled_dir?, bundled_version.clone()?);
            unpacked::install_folder(root, &version, |dest| unpacked::copy_dir(dir, dest).map_err(|e| e.to_string()))
                .ok()
                .map(|_| version)
        });

        match &chosen {
            Some(v) => {
                *self.current.lock().unwrap_or_else(|e| e.into_inner()) = Some(unpacked::exe(&root.join(v)));
                unpacked::prune(root, v);
                Some(v.clone())
            }
            None => bundled_version,
        }
    }

    /// The daily update check. Runs at launch and again while lynk stays
    /// open — it lives in the tray for days, and YouTube breaks old yt-dlp
    /// versions within weeks. Always on: up to 0.1.4 Settings could switch it
    /// off ("Update automatically"), which only ever made downloads fail.
    pub fn update_if_due(&self) {
        let due = self
            .status()
            .last_check
            .is_none_or(|t| now_ms() - t > CHECK_INTERVAL_MS);
        if due {
            self.update();
        }
    }

    /// After a failure that a newer yt-dlp often fixes: checks now instead of
    /// at the next daily check, at most once an hour. Doesn't block.
    pub fn update_soon(self: &Arc<Self>) {
        let recent = self.status().last_check.is_some_and(|t| now_ms() - t < RECHECK_MS);
        if recent || self.is_updating() {
            return;
        }
        let engine = Arc::clone(self);
        std::thread::spawn(move || engine.update());
    }

    fn copy_bundled(&self, bundled: &Path) {
        if let Some(dir) = self.managed.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = self.managed.with_extension("tmp");
        if std::fs::copy(bundled, &tmp).is_ok() && std::fs::rename(&tmp, &self.managed).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        // A copy out of a downloaded .app keeps its quarantine flag, and
        // Gatekeeper would refuse to run it outside the approved bundle.
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("xattr")
            .args(["-d", "com.apple.quarantine"])
            .arg(&self.managed)
            .output();
    }

    /// Checks for (and installs) a newer yt-dlp. Blocking.
    pub fn update(&self) {
        {
            let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
            if s.updating {
                return;
            }
            s.updating = true;
            s.error = None;
            s.updated_to = None;
        }
        self.set_status(|_| {});

        let before = self.status().version;
        let result = if cfg!(windows) { self.update_unpacked(before.as_deref()) } else { self.update_single_file() };

        let now = now_ms();
        let _ = self.db.set_pref("ytdlp_last_check", &now.to_string());
        let after = match &result {
            Ok(Some(v)) => Some(v.clone()),
            _ if cfg!(windows) => before.clone(),
            _ => read_version(&self.managed).or(before.clone()),
        };
        if let Some(v) = &after {
            let _ = self.db.set_pref("ytdlp_version", v);
        }
        let changed = after.is_some() && after != before;
        self.set_status(|s| {
            s.updating = false;
            s.last_check = Some(now);
            s.version = after.clone();
            s.error = result.err();
            s.updated_to = if changed { after } else { None };
        });
    }

    /// `yt-dlp -U` on the managed single-file copy.
    fn update_single_file(&self) -> Result<Option<String>, String> {
        if !self.managed.is_file() {
            if let Some(b) = &self.bundled_ytdlp {
                self.copy_bundled(b);
            }
        }
        if !self.managed.is_file() {
            return Err("The download engine is missing. Reinstall Lynk.".into());
        }
        run_update(&self.managed).map(|_| None)
    }

    /// Installs a newer unpacked build, if there is one, and switches to it.
    fn update_unpacked(&self, current: Option<&str>) -> Result<Option<String>, String> {
        let installed = unpacked::install_latest(&self.managed, current)?;
        if let Some(v) = &installed {
            // Downloads started from now on run the new folder. The old one
            // stays until the next launch: a running download may still load
            // modules from it.
            *self.current.lock().unwrap_or_else(|e| e.into_inner()) = Some(unpacked::exe(&self.managed.join(v)));
        }
        Ok(installed)
    }
}

fn run_update(bin: &Path) -> Result<(), String> {
    let out = process::command(bin)
        .args(["-U", "--encoding", "utf-8"])
        .output()
        .map_err(|e| format!("Couldn't run the updater: {e}"))?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if let Some(err) = text.lines().find_map(|l| l.strip_prefix("ERROR: ")) {
        let msg = if err.to_lowercase().contains("unable to") || err.contains("Errno") {
            "Couldn't reach GitHub to check for updates.".to_string()
        } else {
            crate::parse::clean_error(err)
        };
        return Err(msg);
    }
    Ok(())
}

fn read_version(bin: &Path) -> Option<String> {
    let out = process::command(bin).arg("--version").output().ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !v.is_empty() && v.len() < 40).then_some(v)
}

/// yt-dlp as shipped with this build. On Windows it's the unpacked build, a
/// resource folder next to the app (`yt-dlp/yt-dlp.exe`); elsewhere, and in
/// development runs that only have it, the single-file sidecar.
fn find_bundled_ytdlp() -> Option<PathBuf> {
    if cfg!(windows) {
        let mut candidates = Vec::new();
        if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
            candidates.push(dir.join("yt-dlp").join("yt-dlp.exe"));
        }
        if cfg!(debug_assertions) {
            candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("yt-dlp").join("yt-dlp.exe"));
        }
        if let Some(p) = candidates.into_iter().find(|p| p.is_file()) {
            return Some(p);
        }
    }
    find_sidecar("yt-dlp")
}

/// Bundled binaries live next to the app executable. The bundler strips the
/// target-triple suffix (`yt-dlp.exe`); fall back to the suffixed name and,
/// in debug builds, to `src-tauri/binaries`.
fn find_sidecar(name: &str) -> Option<PathBuf> {
    let triple = option_env!("TAURI_ENV_TARGET_TRIPLE").unwrap_or("");
    let plain = format!("{name}{EXE}");
    let suffixed = format!("{name}-{triple}{EXE}");

    let mut candidates = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        candidates.push(dir.join(&plain));
        candidates.push(dir.join(&suffixed));
        #[cfg(target_os = "macos")]
        candidates.push(dir.join("../Resources").join(&plain));
    }
    if cfg!(debug_assertions) {
        candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries").join(&suffixed));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// The unpacked Windows build: one folder per version under `root`.
mod unpacked {
    use std::fs::File;
    use std::path::{Path, PathBuf};

    pub const ASSET: &str = "yt-dlp_win.zip";
    const CURRENT: &str = "current";

    pub fn exe(dir: &Path) -> PathBuf {
        dir.join("yt-dlp.exe")
    }

    pub fn read_current(root: &Path) -> Option<String> {
        let v = std::fs::read_to_string(root.join(CURRENT)).ok()?;
        let v = v.trim();
        valid_version(v).then(|| v.to_string())
    }

    /// Version folder names come from GitHub tags: dot-separated numbers
    /// only, so a tag can never name `root` itself or anything outside it.
    pub fn valid_version(v: &str) -> bool {
        v.len() < 40 && v.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    }

    /// Fills `root/<version>` with `fill` (into a `.part` folder first) and
    /// makes it the current one. An existing complete folder is reused.
    pub fn install_folder(
        root: &Path,
        version: &str,
        fill: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<(), String> {
        if !valid_version(version) {
            return Err(format!("unexpected version {version:?}"));
        }
        let dest = root.join(version);
        if !exe(&dest).is_file() {
            let part = root.join(format!("{version}.part"));
            let _ = std::fs::remove_dir_all(&part);
            std::fs::create_dir_all(&part).map_err(|e| e.to_string())?;
            if let Err(e) = fill(&part) {
                let _ = std::fs::remove_dir_all(&part);
                return Err(e);
            }
            let _ = std::fs::remove_dir_all(&dest);
            std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
        }
        let tmp = root.join(format!("{CURRENT}.tmp"));
        std::fs::write(&tmp, version).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, root.join(CURRENT)).map_err(|e| e.to_string())
    }

    /// Removes every other version (and leftovers of interrupted updates).
    /// Only called at launch, before any download runs one of them.
    pub fn prune(root: &Path, keep: &str) {
        let Ok(entries) = std::fs::read_dir(root) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == keep || name == CURRENT {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    pub fn copy_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dest)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let target = dest.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_dir(&entry.path(), &target)?;
            } else {
                std::fs::copy(entry.path(), &target)?;
            }
        }
        Ok(())
    }

    pub fn extract_zip(zip_path: &Path, dest: &Path) -> Result<(), String> {
        let file = File::open(zip_path).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            // Entries that would land outside `dest` are skipped.
            let Some(rel) = entry.enclosed_name() else { continue };
            let out = dest.join(rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
                continue;
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut f = File::create(&out).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Downloads the newest release into a folder of its own, checks it
    /// (SHA-256 from the release's own list, as `yt-dlp -U` does, and that it
    /// runs) and makes it current, when it's newer than `current`. Returns
    /// the version it installed.
    pub fn install_latest(root: &Path, current: Option<&str>) -> Result<Option<String>, String> {
        let unreachable = |_| "Couldn't reach GitHub to check for updates.".to_string();
        let latest = latest_tag().map_err(unreachable)?;
        if current.is_some_and(|c| !crate::parse::version_newer(&latest, c)) {
            return Ok(None);
        }
        let base = format!("https://github.com/yt-dlp/yt-dlp/releases/download/{latest}");
        let sums = crate::net::get_text(&format!("{base}/SHA2-256SUMS"), 64 * 1024).map_err(unreachable)?;
        let sha = checksum_for(&sums, ASSET).ok_or_else(|| format!("yt-dlp {latest} has no {ASSET} to update to."))?;

        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        let zip = root.join(format!("{latest}.zip.part"));
        let never = std::sync::atomic::AtomicBool::new(false);
        let fetched = crate::net::fetch_verified(&[format!("{base}/{ASSET}")], &sha, &zip, &never);
        let installed = fetched.and_then(|_| {
            install_folder(root, &latest, |dest| {
                extract_zip(&zip, dest)?;
                // The new folder must actually run before anything uses it.
                match super::read_version(&exe(dest)) {
                    Some(v) if v == latest => Ok(()),
                    Some(v) => Err(format!("it says it is {v}")),
                    None => Err("it doesn't start".into()),
                }
            })
        });
        let _ = std::fs::remove_file(&zip);
        installed.map_err(|e| format!("Couldn't install yt-dlp {latest}: {e}"))?;
        Ok(Some(latest))
    }

    /// The newest stable release, from where `releases/latest` redirects.
    pub fn latest_tag() -> Result<String, String> {
        let target = crate::net::redirect_target("https://github.com/yt-dlp/yt-dlp/releases/latest")?;
        tag_from_url(&target).ok_or_else(|| format!("unexpected redirect to {target}"))
    }

    pub fn tag_from_url(url: &str) -> Option<String> {
        let tag = url.split("/releases/tag/").nth(1)?.split(['/', '?', '#']).next()?.to_string();
        valid_version(&tag).then_some(tag)
    }

    /// The SHA-256 a `SHA2-256SUMS` file lists for `name`.
    pub fn checksum_for(sums: &str, name: &str) -> Option<String> {
        sums.lines().find_map(|line| {
            let (hash, file) = line.trim().split_once(char::is_whitespace)?;
            let file = file.trim().trim_start_matches('*');
            (file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| hash.to_ascii_lowercase())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::unpacked::*;

    #[test]
    fn reads_release_tags_and_checksums() {
        assert_eq!(
            tag_from_url("https://github.com/yt-dlp/yt-dlp/releases/tag/2026.10.26").as_deref(),
            Some("2026.10.26")
        );
        assert_eq!(tag_from_url("https://github.com/yt-dlp/yt-dlp/releases/tag/../evil"), None);
        assert_eq!(tag_from_url("https://github.com/yt-dlp/yt-dlp/releases"), None);
        let sums = "0f192b7ec147ab6288885d6351d9ab67367640029b4377576ef46dd79cf7b202  yt-dlp_macos\n\
                    FD51B212CF8380C7A52EE08C0D6BE7F3C59B2F2C832EAF7B45ACFAE6EAB57419  yt-dlp_win.zip\n";
        assert_eq!(
            checksum_for(sums, "yt-dlp_win.zip").as_deref(),
            Some("fd51b212cf8380c7a52ee08c0d6be7f3c59b2f2c832eaf7b45acfae6eab57419")
        );
        assert_eq!(checksum_for(sums, "yt-dlp_win_x86.zip"), None);
        assert!(valid_version("2026.10.26.1"));
        for bad in ["", ".", "..", "2026..10", ".2026", "2026/10", "2026.10.26 "] {
            assert!(!valid_version(bad), "{bad:?}");
        }
    }

    #[test]
    fn installs_switches_and_prunes_versions() {
        let root = std::env::temp_dir().join(format!("lynk-engine-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fill = |dest: &std::path::Path| std::fs::write(exe(dest), b"x").map_err(|e| e.to_string());
        install_folder(&root, "2026.08.19", fill).unwrap();
        assert_eq!(read_current(&root).as_deref(), Some("2026.08.19"));
        // A failed fill leaves the current version and no half folder.
        assert!(install_folder(&root, "2026.10.26", |_| Err("broken".into())).is_err());
        assert_eq!(read_current(&root).as_deref(), Some("2026.08.19"));
        assert!(!root.join("2026.10.26.part").exists() && !root.join("2026.10.26").exists());
        install_folder(&root, "2026.10.26", fill).unwrap();
        prune(&root, "2026.10.26");
        assert_eq!(read_current(&root).as_deref(), Some("2026.10.26"));
        assert!(exe(&root.join("2026.10.26")).is_file() && !root.join("2026.08.19").exists());
        assert!(install_folder(&root, "..", fill).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Real download from GitHub (~24 MB):
    /// `cargo test -- --ignored installs_the_latest_release`
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn installs_the_latest_release() {
        let root = std::env::temp_dir().join(format!("lynk-engine-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let v = install_latest(&root, Some("2020.01.01")).unwrap().expect("an update from 2020");
        assert_eq!(read_current(&root).as_deref(), Some(v.as_str()));
        assert_eq!(super::read_version(&exe(&root.join(&v))).as_deref(), Some(v.as_str()));
        assert!(root.join(&v).join("_internal").is_dir());
        assert_eq!(install_latest(&root, Some(&v)).unwrap(), None, "already current");
        let leftovers: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(leftovers.len(), 2, "only the version folder and `current`: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
