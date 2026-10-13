use serde::{Deserialize, Serialize};

/// One download, as stored in SQLite and sent to the frontend.
///
/// `status` is one of:
/// - `queued`      waiting for a free slot
/// - `fetching`    yt-dlp is running but no bytes have arrived yet
/// - `downloading` bytes are flowing
/// - `processing`  merging / converting / tagging
/// - `done` | `error` | `cancelled`
/// - `playlist`    the link turned out to be a playlist; waiting for the user
///   to confirm (see `playlist`)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Download {
    pub id: String,
    pub url: String,
    pub title: Option<String>,
    pub thumbnail: Option<String>,
    pub duration: Option<f64>,
    pub site: Option<String>,
    pub format: String,
    pub quality: String,
    pub audio_format: String,
    /// Container for video downloads: `mp4` | `mkv`.
    pub video_format: String,
    pub prefer_h264: bool,
    pub ext: Option<String>,
    /// Directory the file is saved into.
    pub save_path: Option<String>,
    /// Final file on disk, once known.
    pub file_path: Option<String>,
    pub status: String,
    /// Overall progress, 0–100.
    pub progress: f64,
    pub size_bytes: Option<i64>,
    pub error_msg: Option<String>,
    pub error_detail: Option<String>,
    /// Unix milliseconds.
    pub created_at: i64,
    pub finished_at: Option<i64>,
    /// Playlist grouping — every item of a confirmed playlist shares a group.
    pub group_id: Option<String>,
    pub group_title: Option<String>,
    pub group_index: Option<i64>,
    pub group_count: Option<i64>,
    /// Literal filename prefix, e.g. "03 - " for playlist items.
    pub name_prefix: Option<String>,

    // ── Transient (never persisted) ──
    /// `fetching` | `downloading` | `video` | `audio` | `merging` | `converting` | `finishing`
    #[serde(skip_deserializing)]
    pub stage: Option<String>,
    #[serde(skip_deserializing)]
    pub speed: Option<f64>,
    #[serde(skip_deserializing)]
    pub eta: Option<f64>,
    /// Set by `list_downloads` for finished rows whose file is gone.
    #[serde(skip_deserializing)]
    pub file_missing: bool,
    #[serde(skip_deserializing)]
    pub playlist: Option<PlaylistPreview>,
}

impl Download {
    pub fn is_active(&self) -> bool {
        matches!(
            self.status.as_str(),
            "queued" | "fetching" | "downloading" | "processing"
        )
    }

    /// Clears per-run state so the record can be run again.
    pub fn reset_for_run(&mut self) {
        self.status = "queued".into();
        self.progress = 0.0;
        self.stage = None;
        self.speed = None;
        self.eta = None;
        self.file_missing = false;
        self.error_msg = None;
        self.error_detail = None;
        self.finished_at = None;
        self.size_bytes = None;
        self.file_path = None;
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PlaylistPreview {
    pub title: String,
    pub count: usize,
    /// More entries exist than were listed (we cap the listing).
    pub truncated: bool,
    /// Entries skipped because they are playlists themselves (channel tabs).
    pub nested: usize,
}

/// One entry of a detected playlist (from yt-dlp's flat listing).
#[derive(Debug, Clone)]
pub struct PlaylistEntry {
    pub url: String,
    pub title: Option<String>,
    pub thumbnail: Option<String>,
    pub duration: Option<f64>,
    pub index: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewDownload {
    pub url: String,
    pub format: String,
    pub quality: String,
    pub audio_format: String,
    #[serde(default = "default_video_format")]
    pub video_format: String,
    pub prefer_h264: bool,
    pub save_path: String,
}

fn default_video_format() -> String {
    "mp4".into()
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}
