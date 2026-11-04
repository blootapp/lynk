//! Everything that knows about yt-dlp's command line and output format.
//!
//! yt-dlp is driven with `--print` / `--progress-template` so that every line
//! we care about starts with a `LYNK_*` marker and has a fixed shape — no
//! scraping of human-readable log output.

use serde::{Deserialize, Deserializer};
use std::path::Path;

/// Max entries listed for a playlist/channel link.
pub const PLAYLIST_LIMIT: usize = 500;

// ── Command line ─────────────────────────────────────────────────────────────

pub struct ArgSpec<'a> {
    pub url: &'a str,
    pub format: &'a str,
    pub quality: &'a str,
    pub audio_format: &'a str,
    pub video_format: &'a str,
    pub prefer_h264: bool,
    pub dir: &'a str,
    pub name_prefix: Option<&'a str>,
    pub ffmpeg: Option<&'a Path>,
    pub cookies_browser: Option<&'a str>,
    /// deno for YouTube's JS challenges.
    pub js_runtime: Option<&'a Path>,
    /// Plugin folder to load (the PO-token plugin); none on normal runs.
    /// YouTube then also switches to the clients that use its tokens.
    pub plugin_dir: Option<&'a Path>,
}

pub fn build_args(s: &ArgSpec) -> Vec<String> {
    let youtube = crate::youtube::is_youtube(s.url);
    let raw_prefix = s.name_prefix.unwrap_or("");
    let prefix = raw_prefix.replace('%', "%%");
    let mut a: Vec<String> = [
        "--encoding", "utf-8",
        "--no-update",
        // A yt-dlp config or plugin the user set up for their own use must
        // not change what Lynk asks for.
        "--ignore-config",
        "--no-plugin-dirs",
        "--newline",
        "--progress",
        "--no-simulate",
        "--no-mtime",
        "--concurrent-fragments", "4",
        // A video link that merely carries a playlist (watch?v=…&list=…) is
        // treated as the video. A pure playlist link is listed, not
        // downloaded, so the user can confirm it first.
        "--no-playlist",
        "--flat-playlist",
        "--embed-metadata",
        "--print", "video:LYNK_META %(.{title,thumbnail,duration,extractor_key,ie_key,url,webpage_url,playlist_index,playlist_count,playlist_title,format_id,filename})j",
        "--print", "video:LYNK_SIZES %(requested_formats.0.filesize,requested_formats.0.filesize_approx|0)s %(requested_formats.1.filesize,requested_formats.1.filesize_approx|0)s %(thumbnails.-1.url|NA)s",
        "--print", "after_move:LYNK_FILE %(filepath)s",
        "--progress-template", "download:LYNK_P %(progress.status)s %(progress.downloaded_bytes|0)s %(progress.total_bytes|0)s %(progress.total_bytes_estimate|0)s %(progress.speed|0)s %(progress.eta|NA)s %(progress.fragment_index|NA)s %(progress.fragment_count|NA)s %(info.vcodec|NA)s %(info.acodec|NA)s %(info.format_id|NA)s",
        "--progress-template", "postprocess:LYNK_PP %(progress.status)s %(progress.postprocessor)s",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    a.extend(["--playlist-end".into(), PLAYLIST_LIMIT.to_string()]);
    a.extend(["-P".into(), s.dir.to_string()]);
    let title = title_bytes(s.dir, raw_prefix, cfg!(windows));
    a.extend(["-o".into(), format!("{prefix}%(title).{title}B.%(ext)s")]);
    if let Some(ff) = s.ffmpeg {
        a.extend(["--ffmpeg-location".into(), ff.to_string_lossy().to_string()]);
    }
    if let Some(b) = s.cookies_browser.filter(|b| !b.is_empty()) {
        a.extend(["--cookies-from-browser".into(), b.to_string()]);
    }
    if let Some(deno) = s.js_runtime {
        a.extend(["--js-runtimes".into(), format!("deno:{}", deno.to_string_lossy())]);
    }
    // A Node.js the user already has is a fallback for a missing deno.
    a.extend(["--js-runtimes".into(), "node".into()]);
    if let Some(dir) = s.plugin_dir {
        a.extend(["--plugin-dirs".into(), dir.to_string_lossy().to_string()]);
    }
    if youtube {
        // One combined value: a second `--extractor-args youtube:…` would
        // replace this one, not add to it.
        let clients = if s.plugin_dir.is_some() { crate::youtube::TOKEN_CLIENTS } else { crate::youtube::CLIENTS };
        a.extend(["--extractor-args".into(), format!("youtube:player_client={clients}")]);
    }

    if s.format == "audio" {
        let selector = if s.audio_format == "m4a" { "ba[ext=m4a]/ba/b" } else { "ba/b" };
        a.extend(
            ["-f", selector, "-x", "--audio-format", s.audio_format, "--audio-quality", "0", "--embed-thumbnail"]
                .iter()
                .map(|s| s.to_string()),
        );
    } else {
        let mkv = s.video_format == "mkv";
        a.extend(["-f".into(), "bv*+ba/b".into(), "-S".into(), video_sort(s.quality, s.prefer_h264, mkv, youtube)]);
        // MKV holds any codec, so everything can be remuxed into it losslessly.
        let (container, remux) = if mkv { ("mkv", "mkv") } else { ("mp4", "mkv>mp4/webm>mp4/mov>mp4") };
        a.extend(
            ["--merge-output-format", container, "--remux-video", remux]
                .iter()
                .map(|s| s.to_string()),
        );
    }

    // `--` stops option parsing: a "URL" can never be read as a yt-dlp flag.
    a.push("--".into());
    a.push(s.url.to_string());
    a
}

/// Format-sort string. An explicit resolution wins over the codec preference
/// (picking "4K" should give 4K even though YouTube has no 4K H.264); with
/// "best", H.264 preference caps at the best H.264 stream — on YouTube at
/// 1080p outright, so a video whose H.264 stops lower still comes in its
/// best stream up to 1080p.
///
/// The picture decides before any audio field. Ranked ahead of resolution,
/// the AAC preference made YouTube's one combined stream (360p, sometimes
/// with a dubbed audio track) beat every 1080p one. At equal picture, direct
/// https beats HLS and a video-only stream (merged with the best audio)
/// beats a combined one. The audio fields then pick the audio stream: the
/// original language, and for MP4 AAC and MP4/M4A sources; MKV takes the
/// best audio as-is.
pub fn video_sort(quality: &str, prefer_h264: bool, mkv: bool, youtube: bool) -> String {
    let mut sort: Vec<String> = Vec::new();
    match quality_height(quality) {
        Some(h) => sort.push(format!("res:{h}")),
        None if prefer_h264 && youtube => sort.push("res:1080".into()),
        None => {}
    }
    if prefer_h264 {
        sort.push("vcodec:h264".into());
    }
    if !sort.iter().any(|f| f.starts_with("res")) {
        sort.push("res".into());
    }
    sort.extend(["fps", "proto", "+hasaud", "lang"].map(String::from));
    if !mkv {
        // AAC audio is native to MP4 — no transcode, plays everywhere.
        sort.push("acodec:aac".into());
        sort.push("ext:mp4:m4a".into());
    }
    sort.join(",")
}

/// Longest title, in UTF-8 bytes, that keeps every file yt-dlp writes into
/// `dir` within Windows' 260-character path limit, which applies unless long
/// paths are switched on system-wide. A UTF-8 byte count never undercounts
/// UTF-16 units, so this holds for any script.
fn title_bytes(dir: &str, prefix: &str, windows: bool) -> usize {
    const MAX: usize = 150;
    if !windows {
        return MAX;
    }
    // "<dir>\" + prefix + title + the longest suffix yt-dlp adds while it
    // works (".f140-27.m4a.part-Frag1234").
    let used = dir.encode_utf16().count() + 1 + prefix.encode_utf16().count() + 30;
    250usize.saturating_sub(used).clamp(40, MAX)
}

fn quality_height(q: &str) -> Option<u32> {
    match q {
        "2160p" => Some(2160),
        "1440p" => Some(1440),
        "1080p" => Some(1080),
        "720p" => Some(720),
        "480p" => Some(480),
        "360p" => Some(360),
        _ => None,
    }
}

// ── Output lines ─────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, Deserialize)]
pub struct Meta {
    #[serde(default, deserialize_with = "any_string")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "any_string")]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default, deserialize_with = "any_string")]
    pub extractor_key: Option<String>,
    #[serde(default, deserialize_with = "any_string")]
    pub ie_key: Option<String>,
    #[serde(default, deserialize_with = "any_string")]
    pub url: Option<String>,
    #[serde(default, deserialize_with = "any_string")]
    pub webpage_url: Option<String>,
    #[serde(default)]
    pub playlist_index: Option<i64>,
    #[serde(default)]
    pub playlist_count: Option<i64>,
    #[serde(default, deserialize_with = "any_string")]
    pub playlist_title: Option<String>,
    #[serde(default, deserialize_with = "any_string")]
    pub format_id: Option<String>,
    #[serde(default, deserialize_with = "any_string")]
    pub filename: Option<String>,
}

impl Meta {
    /// Flat playlist entries that are themselves playlists (a channel's
    /// "Videos"/"Shorts" tabs, nested playlists, …).
    pub fn is_nested_playlist(&self) -> bool {
        let k = self.ie_key.as_deref().unwrap_or("");
        k.ends_with("Tab")
            || k.contains("Playlist")
            || k.ends_with("Channel")
            || k.ends_with("User")
    }
}

/// Some extractors emit numeric ids; accept any scalar as a string.
fn any_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(match v {
        Some(serde_json::Value::String(s)) => Some(s),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        Some(serde_json::Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub finished: bool,
    pub downloaded: f64,
    pub total: f64,
    pub estimate: f64,
    pub speed: Option<f64>,
    pub eta: Option<f64>,
    pub frag_index: Option<f64>,
    pub frag_count: Option<f64>,
    pub vcodec: String,
    pub acodec: String,
    pub format_id: String,
}

#[derive(Debug)]
pub enum Line {
    Meta(Box<Meta>),
    Sizes { first: f64, second: f64, thumb: Option<String> },
    Progress(Progress),
    PostProcess { started: bool, name: String },
    File(String),
    Error(String),
    Other,
}

pub fn parse_line(line: &str) -> Line {
    let line = line.trim_end();
    if let Some(rest) = line.strip_prefix("LYNK_P ") {
        return parse_progress(rest).map(Line::Progress).unwrap_or(Line::Other);
    }
    if let Some(rest) = line.strip_prefix("LYNK_META ") {
        return serde_json::from_str::<Meta>(rest)
            .map(|m| Line::Meta(Box::new(m)))
            .unwrap_or(Line::Other);
    }
    if let Some(rest) = line.strip_prefix("LYNK_SIZES ") {
        let mut it = rest.splitn(3, ' ');
        let first = num(it.next()).unwrap_or(0.0);
        let second = num(it.next()).unwrap_or(0.0);
        let thumb = it
            .next()
            .map(str::trim)
            .filter(|t| t.starts_with("http"))
            .map(String::from);
        return Line::Sizes { first, second, thumb };
    }
    if let Some(rest) = line.strip_prefix("LYNK_PP ") {
        let mut it = rest.splitn(2, ' ');
        let status = it.next().unwrap_or("");
        let name = it.next().unwrap_or("").trim().to_string();
        return Line::PostProcess { started: status == "started", name };
    }
    if let Some(rest) = line.strip_prefix("LYNK_FILE ") {
        let p = rest.trim();
        return if p.is_empty() || p == "NA" { Line::Other } else { Line::File(p.to_string()) };
    }
    if let Some(rest) = line.strip_prefix("ERROR: ") {
        return Line::Error(rest.trim().to_string());
    }
    Line::Other
}

fn parse_progress(rest: &str) -> Option<Progress> {
    let f: Vec<&str> = rest.splitn(11, ' ').collect();
    if f.len() < 11 {
        return None;
    }
    Some(Progress {
        finished: f[0] == "finished",
        downloaded: num(Some(f[1])).unwrap_or(0.0),
        total: num(Some(f[2])).unwrap_or(0.0),
        estimate: num(Some(f[3])).unwrap_or(0.0),
        speed: num(Some(f[4])).filter(|v| *v > 0.0),
        eta: num(Some(f[5])),
        frag_index: num(Some(f[6])),
        frag_count: num(Some(f[7])),
        vcodec: f[8].to_string(),
        acodec: f[9].to_string(),
        format_id: f[10].trim().to_string(),
    })
}

fn num(s: Option<&str>) -> Option<f64> {
    s.and_then(|s| s.trim().parse::<f64>().ok()).filter(|v| v.is_finite())
}

// ── Progress across streams ──────────────────────────────────────────────────

/// Turns per-stream progress into one monotonic 0–100 value. A video+audio
/// download is two sequential streams; without this the bar fills twice.
#[derive(Debug, Default)]
pub struct Tracker {
    two_streams: bool,
    sizes: (f64, f64),
    first_is_video: Option<bool>,
    stream: usize,
    current_format: Option<String>,
    /// Multi-entry posts (e.g. a tweet with two videos) download several
    /// items in one run.
    entry: Option<(i64, i64)>,
    pub progress: f64,
}

impl Tracker {
    pub fn on_meta(&mut self, m: &Meta) {
        self.two_streams = m.format_id.as_deref().is_some_and(|f| f.contains('+'));
        self.sizes = (0.0, 0.0);
        self.first_is_video = None;
        self.stream = 0;
        self.current_format = None;
        self.entry = match (m.playlist_index, m.playlist_count) {
            (Some(i), Some(n)) if n > 1 && i >= 1 => Some((i, n)),
            _ => None,
        };
    }

    pub fn on_sizes(&mut self, first: f64, second: f64) {
        self.sizes = (first, second);
    }

    /// Returns the stage label for this stream.
    pub fn on_progress(&mut self, p: &Progress) -> &'static str {
        let is_video = p.vcodec != "none" && p.vcodec != "NA";
        if self.first_is_video.is_none() {
            self.first_is_video = Some(is_video);
        }
        match &self.current_format {
            Some(f) if *f != p.format_id => self.stream = (self.stream + 1).min(1),
            _ => {}
        }
        self.current_format = Some(p.format_id.clone());

        let total = if p.total > 0.0 { p.total } else { p.estimate };
        let frac = if p.finished {
            1.0
        } else if total > 0.0 {
            p.downloaded / total
        } else if let (Some(i), Some(n)) = (p.frag_index, p.frag_count) {
            if n > 0.0 { i / n } else { 0.0 }
        } else {
            0.0
        }
        .clamp(0.0, 1.0);

        let item = if self.two_streams {
            let w0 = if self.sizes.0 > 0.0 && self.sizes.1 > 0.0 {
                self.sizes.0 / (self.sizes.0 + self.sizes.1)
            } else if self.first_is_video.unwrap_or(true) {
                0.85
            } else {
                0.15
            };
            if self.stream == 0 { frac * w0 } else { w0 + frac * (1.0 - w0) }
        } else {
            frac
        };

        let overall = match self.entry {
            Some((i, n)) => ((i - 1) as f64 + item) / n as f64,
            None => item,
        };
        self.progress = self.progress.max(overall * 100.0).min(100.0);

        if !self.two_streams {
            "downloading"
        } else if is_video {
            "video"
        } else {
            "audio"
        }
    }
}

/// Stage label for a postprocessor name, or None to ignore it.
pub fn postprocess_stage(name: &str) -> Option<&'static str> {
    match name {
        "Merger" => Some("merging"),
        "ExtractAudio" | "VideoConvertor" | "VideoRemuxer" => Some("converting"),
        "Concat" | "" => None,
        _ => Some("finishing"),
    }
}

// ── Errors ───────────────────────────────────────────────────────────────────

/// Strips yt-dlp's "[extractor] id: " prefix and the long help suffixes.
pub fn clean_error(raw: &str) -> String {
    let mut s = raw.trim().trim_start_matches("ERROR:").trim();
    if s.starts_with('[') {
        if let Some(end) = s.find("] ") {
            s = &s[end + 2..];
            // "<id>: message" — ids never contain spaces
            if let Some(colon) = s.find(": ") {
                if !s[..colon].contains(' ') {
                    s = &s[colon + 2..];
                }
            }
        }
    }
    let cut = [" Use --", " See  http", " See http", " Also see", "; please report this issue"]
        .iter()
        .filter_map(|m| s.find(m))
        .min()
        .unwrap_or(s.len());
    let s = s[..cut].trim().trim_end_matches(['.', ',']);
    let mut out: String = s.chars().take(220).collect();
    if s.chars().count() > 220 {
        out.push('…');
    }
    out
}

/// Maps a yt-dlp error to something a person can act on.
pub fn humanize_error(raw: &str) -> String {
    let l = raw.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| l.contains(n));

    if has(&["not a bot", "confirm you’re not", "confirm you're not"]) {
        return "YouTube asked to verify you're not a bot. Wait a bit and retry, or pick your browser under Settings › Sign-in cookies.".into();
    }
    // Chrome and other Chromium browsers seal their cookies to themselves on
    // Windows; closing the browser doesn't help.
    if has(&["failed to decrypt with dpapi", "app-bound", "app bound"]) {
        return "This browser locks its cookies on Windows. Choose Firefox under Settings › Sign-in cookies.".into();
    }
    if has(&["could not copy chrome cookie", "could not copy", "failed to decrypt", "cookie database", "could not find"]) && l.contains("cookie") {
        return "Couldn't read cookies from your browser. Close the browser and retry, or choose another browser in Settings.".into();
    }
    if has(&["unsupported url"]) {
        return "This link isn't supported.".into();
    }
    if has(&["is not a valid url"]) {
        return "That doesn't look like a valid link.".into();
    }
    if has(&["requested format is not available", "requested format not available"]) {
        return "That quality isn't available for this video. Try a different quality.".into();
    }
    if has(&["confirm your age", "age-restricted", "age restricted", "inappropriate for some users"]) {
        return "This video is age-restricted. Pick your browser under Settings › Sign-in cookies to use your account.".into();
    }
    if has(&["private video", "video is private", "this video is private"]) {
        return "This video is private.".into();
    }
    if has(&["members-only", "members only", "join this channel"]) {
        return "This video is for channel members only.".into();
    }
    if has(&["premieres in", "live event will begin", "this live event", "is upcoming"]) {
        return "This video hasn't started yet.".into();
    }
    if has(&["copyright"]) {
        return "This video was removed for copyright reasons.".into();
    }
    if has(&["not available in your country", "geo restrict", "geo-restrict", "not available from your location"]) {
        return "This content isn't available in your region.".into();
    }
    if has(&["http error 429", "too many requests"]) {
        return "The site is rate-limiting requests. Wait a few minutes and retry.".into();
    }
    if has(&["http error 403", "forbidden"]) {
        return "The site refused the download (403). Updating the download engine in Settings usually fixes this.".into();
    }
    if has(&["http error 404", "not found (404)"]) {
        return "Not found (404). The link may be broken or the video removed.".into();
    }
    if has(&["video unavailable", "this video is unavailable", "no longer available", "has been removed", "been terminated"]) {
        return "This video is unavailable.".into();
    }
    if has(&["login required", "logged-in", "log in", "sign in", "--username", "requires authentication", "account credentials"]) {
        return "This content requires signing in. Pick your browser under Settings › Sign-in cookies.".into();
    }
    if has(&["no space left", "errno 28", "not enough space", "disk full"]) {
        return "Not enough disk space.".into();
    }
    if has(&["permission denied", "errno 13", "access is denied"]) {
        return "Lynk can't write to the save folder. Choose another folder in Settings.".into();
    }
    if has(&["ffmpeg not found", "ffmpeg is not installed", "ffprobe and ffmpeg not found"]) {
        return "ffmpeg is missing, so the file couldn't be processed. Reinstall Lynk.".into();
    }
    if has(&[
        "getaddrinfo", "name or service not known", "nodename nor servname", "timed out",
        "connection reset", "forcibly closed", "network is unreachable", "connection refused",
        "connection aborted", "remote end closed", "unable to download webpage", "ssl", "eof occurred",
    ]) {
        return "Network problem. Check your connection and retry.".into();
    }
    if has(&["no video formats found", "no formats found"]) {
        return "No downloadable video was found at this link.".into();
    }

    let cleaned = clean_error(raw);
    if cleaned.is_empty() { "Download failed.".into() } else { cleaned }
}

// ── Files ────────────────────────────────────────────────────────────────────

/// Makes a string safe to use as one Windows/macOS path component.
pub fn sanitize_component(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut s: String = s.chars().take(80).collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if reserved.iter().any(|r| r.eq_ignore_ascii_case(&s)) {
        s.insert(0, '_');
    }
    if s.is_empty() { "Playlist".into() } else { s }
}

/// Compares yt-dlp versions like "2026.08.19" or "2026.08.19.232000".
pub fn version_newer(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim().split(['.', '-']).map(|p| p.parse().unwrap_or(0)).collect()
    };
    parse(a) > parse(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_progress_line() {
        let l = "LYNK_P downloading 11720672 0 11850720.0 1107220.1742429116 1.2352667875685137 61 62 avc1.42001e mp4a.40.2 hls-380";
        let Line::Progress(p) = parse_line(l) else { panic!("not progress") };
        assert!(!p.finished);
        assert_eq!(p.downloaded, 11720672.0);
        assert_eq!(p.estimate, 11850720.0);
        assert_eq!(p.frag_index, Some(61.0));
        assert_eq!(p.vcodec, "avc1.42001e");
        assert_eq!(p.format_id, "hls-380");
    }

    #[test]
    fn parses_na_progress_fields() {
        let l = "LYNK_P finished 158405 158405 0 198274.23 NA NA NA none mp3 mp3-128";
        let Line::Progress(p) = parse_line(l) else { panic!() };
        assert!(p.finished);
        assert_eq!(p.eta, None);
        assert_eq!(p.vcodec, "none");
    }

    #[test]
    fn parses_meta_with_unicode_and_numeric_ids() {
        let l = r#"LYNK_META {"id": 1812978515, "playlist_title": 2024, "title": "Review \u2013 Bateman", "duration": 9.8, "playlist_index": 2, "playlist_count": 22, "ie_key": "YoutubeTab"}"#;
        let Line::Meta(m) = parse_line(l) else { panic!() };
        assert_eq!(m.playlist_title.as_deref(), Some("2024"));
        assert_eq!(m.title.as_deref(), Some("Review – Bateman"));
        assert_eq!(m.playlist_index, Some(2));
        assert!(m.is_nested_playlist());
    }

    #[test]
    fn parses_file_with_spaces_and_commas() {
        let l = r"LYNK_FILE C:\Users\me\Downloads\Lynk\Party Review –  Bateman, Munn.mp4";
        let Line::File(f) = parse_line(l) else { panic!() };
        assert!(f.ends_with("Bateman, Munn.mp4"));
    }

    #[test]
    fn parses_sizes_and_postprocess() {
        let Line::Sizes { first, second, thumb } = parse_line("LYNK_SIZES 100 50 https://x/y.jpg") else { panic!() };
        assert_eq!((first, second), (100.0, 50.0));
        assert_eq!(thumb.as_deref(), Some("https://x/y.jpg"));
        let Line::Sizes { thumb, .. } = parse_line("LYNK_SIZES 0 0 NA") else { panic!() };
        assert!(thumb.is_none());
        let Line::PostProcess { started, name } = parse_line("LYNK_PP started Merger") else { panic!() };
        assert!(started);
        assert_eq!(postprocess_stage(&name), Some("merging"));
        assert_eq!(postprocess_stage("Concat"), None);
    }

    #[test]
    fn two_stream_progress_is_monotonic_and_weighted() {
        let mut t = Tracker::default();
        t.on_meta(&Meta { format_id: Some("137+140".into()), ..Default::default() });
        t.on_sizes(900.0, 100.0);
        let p = |fmt: &str, v: &str, dl: f64, tot: f64, fin: bool| Progress {
            finished: fin, downloaded: dl, total: tot, estimate: 0.0, speed: None, eta: None,
            frag_index: None, frag_count: None, vcodec: v.into(), acodec: "x".into(), format_id: fmt.into(),
        };
        assert_eq!(t.on_progress(&p("137", "avc1", 450.0, 900.0, false)), "video");
        assert!((t.progress - 45.0).abs() < 0.01);
        t.on_progress(&p("137", "avc1", 900.0, 900.0, true));
        assert!((t.progress - 90.0).abs() < 0.01);
        assert_eq!(t.on_progress(&p("140", "none", 50.0, 100.0, false)), "audio");
        assert!((t.progress - 95.0).abs() < 0.01);
    }

    #[test]
    fn fragment_progress_used_without_sizes() {
        let mut t = Tracker::default();
        t.on_meta(&Meta { format_id: Some("hls-1".into()), ..Default::default() });
        let p = Progress {
            finished: false, downloaded: 10.0, total: 0.0, estimate: 0.0, speed: None, eta: None,
            frag_index: Some(5.0), frag_count: Some(10.0), vcodec: "avc1".into(), acodec: "mp4a".into(),
            format_id: "hls-1".into(),
        };
        t.on_progress(&p);
        assert!((t.progress - 50.0).abs() < 0.01);
    }

    #[test]
    fn humanizes_common_errors() {
        let bot = "[youtube] jNQXAC9IVRw: Sign in to confirm you’re not a bot. Use --cookies-from-browser or --cookies for the authentication.";
        assert!(humanize_error(bot).contains("not a bot"));
        assert!(humanize_error("unable to download video data: HTTP Error 403: Forbidden").contains("403"));
        assert!(humanize_error("[generic] Unsupported URL: https://x.y").contains("isn't supported"));
        assert!(humanize_error("Failed to decrypt with DPAPI. See  https://github.com/yt-dlp/yt-dlp/issues/10927  for more info").contains("Firefox"));
        assert!(humanize_error("Could not copy Chrome cookie database. See  https://github.com/yt-dlp/yt-dlp/issues/7271  for more info").contains("Close the browser"));
        assert!(humanize_error("[soundcloud] Unable to download webpage: ('Connection aborted.', ConnectionResetError(10054, 'An existing connection was forcibly closed'))").contains("Network"));
        assert_eq!(
            humanize_error("[vimeo] 123: Something odd happened. See  https://github.com/x for help"),
            "Something odd happened"
        );
    }

    #[test]
    fn args_end_with_separator_and_url() {
        let spec = ArgSpec {
            url: "--exec=calc", format: "video", quality: "1080p", audio_format: "mp3", video_format: "mp4",
            prefer_h264: true, dir: "C:\\x", name_prefix: Some("05 - 100% "), ffmpeg: None,
            cookies_browser: Some(""), js_runtime: None, plugin_dir: None,
        };
        let a = build_args(&spec);
        assert_eq!(a[a.len() - 2], "--");
        assert_eq!(a[a.len() - 1], "--exec=calc");
        assert!(a.contains(&"05 - 100%% %(title).150B.%(ext)s".to_string()));
        assert!(!a.contains(&"--cookies-from-browser".to_string()));
        assert!(a.contains(&"res:1080,vcodec:h264,fps,proto,+hasaud,lang,acodec:aac,ext:mp4:m4a".to_string()));
        assert!(a.contains(&"--no-plugin-dirs".to_string()));
        assert!(!a.contains(&"--plugin-dirs".to_string()));
        assert!(!a.contains(&"--extractor-args".to_string()));
    }

    /// Sorted ahead of resolution, an audio preference let YouTube's combined
    /// 360p stream ("18", AAC) beat every 1080p video-only stream.
    #[test]
    fn picture_is_ranked_before_audio() {
        assert_eq!(
            video_sort("best", true, false, true),
            "res:1080,vcodec:h264,fps,proto,+hasaud,lang,acodec:aac,ext:mp4:m4a"
        );
        assert_eq!(
            video_sort("best", true, false, false),
            "vcodec:h264,res,fps,proto,+hasaud,lang,acodec:aac,ext:mp4:m4a"
        );
        assert_eq!(video_sort("best", false, false, true), "res,fps,proto,+hasaud,lang,acodec:aac,ext:mp4:m4a");
        assert_eq!(video_sort("2160p", true, false, true), "res:2160,vcodec:h264,fps,proto,+hasaud,lang,acodec:aac,ext:mp4:m4a");
        for (q, h264, mkv, yt) in [("best", true, false, true), ("best", false, false, false), ("360p", true, false, true)] {
            let s = video_sort(q, h264, mkv, yt);
            let at = |f: &str| s.find(f).unwrap();
            assert!(at("res") < at("acodec") && at("fps") < at("acodec") && at("+hasaud") < at("lang"), "{s}");
        }
    }

    #[test]
    fn youtube_player_clients() {
        let deno = Path::new("C:\\Lynk\\bin\\deno.exe");
        let plugins = Path::new("C:\\Lynk\\yt-dlp-plugins");
        let spec = |url, plugin_dir| ArgSpec {
            url, format: "video", quality: "best", audio_format: "mp3", video_format: "mp4",
            prefer_h264: true, dir: "C:\\x", name_prefix: None, ffmpeg: None,
            cookies_browser: None, js_runtime: Some(deno), plugin_dir,
        };
        let value = |a: &[String]| {
            let i = a.iter().position(|x| x == "--extractor-args")?;
            (a.iter().filter(|x| *x == "--extractor-args").count() == 1).then(|| a[i + 1].clone())
        };
        assert_eq!(
            value(&build_args(&spec("https://youtu.be/x", None))).as_deref(),
            Some("youtube:player_client=default,web_embedded")
        );
        assert_eq!(
            value(&build_args(&spec("https://www.youtube.com/watch?v=x", Some(plugins)))).as_deref(),
            Some("youtube:player_client=mweb,web,web_embedded")
        );
        assert_eq!(value(&build_args(&spec("https://vimeo.com/1", None))), None);
    }

    #[test]
    fn titles_fit_windows_paths() {
        let id = "\\.lynk-partial\\0f8fad5b-d9cb-469f-a165-70867728950e";
        let short = format!("C:\\Users\\me\\Downloads\\Lynk{id}");
        assert_eq!(title_bytes(&short, "", true), 142);
        assert_eq!(title_bytes("C:\\x", "", true), 150);
        let deep = format!("C:\\Users\\Windows 11\\Downloads\\Lynk\\{}{id}", "P".repeat(80));
        let n = title_bytes(&deep, "001 - ", true);
        assert!(n < 100 && deep.len() + 1 + 6 + n + 30 <= 250, "{n}");
        assert_eq!(title_bytes(&"D".repeat(300), "", true), 40);
        assert_eq!(title_bytes(&deep, "001 - ", false), 150);
    }

    #[test]
    fn youtube_runtime_and_plugin_args() {
        let deno = Path::new("C:\\Lynk\\bin\\deno.exe");
        let plugins = Path::new("C:\\Lynk\\yt-dlp-plugins");
        let spec = ArgSpec {
            url: "https://youtu.be/x", format: "audio", quality: "best", audio_format: "mp3",
            video_format: "mp4", prefer_h264: false, dir: "C:\\x", name_prefix: None,
            ffmpeg: None, cookies_browser: None, js_runtime: Some(deno), plugin_dir: Some(plugins),
        };
        let a = build_args(&spec);
        let at = |flag: &str| a.iter().position(|x| x == flag).unwrap();
        assert_eq!(a[at("--js-runtimes") + 1], "deno:C:\\Lynk\\bin\\deno.exe");
        // --plugin-dirs must come after --no-plugin-dirs, which resets the list.
        assert!(at("--plugin-dirs") > at("--no-plugin-dirs"));
        assert_eq!(a[at("--plugin-dirs") + 1], "C:\\Lynk\\yt-dlp-plugins");
    }

    #[test]
    fn mkv_takes_best_audio_and_remuxes_everything() {
        assert_eq!(video_sort("best", false, true, true), "res,fps,proto,+hasaud,lang");
        assert_eq!(video_sort("720p", true, true, false), "res:720,vcodec:h264,fps,proto,+hasaud,lang");
        let spec = ArgSpec {
            url: "https://x.y/v", format: "video", quality: "best", audio_format: "mp3",
            video_format: "mkv", prefer_h264: false, dir: "C:\\x", name_prefix: None,
            ffmpeg: None, cookies_browser: None, js_runtime: None, plugin_dir: None,
        };
        let a = build_args(&spec);
        let pos = a.iter().position(|x| x == "--merge-output-format").unwrap();
        assert_eq!(a[pos + 1], "mkv");
        let pos = a.iter().position(|x| x == "--remux-video").unwrap();
        assert_eq!(a[pos + 1], "mkv");
    }

    #[test]
    fn sanitizes_folder_names() {
        assert_eq!(sanitize_component("Mix: Best of 2024 / Vol. 1?"), "Mix Best of 2024 Vol. 1");
        assert_eq!(sanitize_component("CON"), "_CON");
        assert_eq!(sanitize_component("..."), "Playlist");
    }

    #[test]
    fn compares_versions() {
        assert!(version_newer("2026.08.19", "2026.06.09"));
        assert!(!version_newer("2026.06.09", "2026.06.09"));
        assert!(version_newer("2026.06.09.1", "2026.06.09"));
    }
}
