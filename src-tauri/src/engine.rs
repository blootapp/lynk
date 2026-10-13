//! Locating and updating the yt-dlp / ffmpeg binaries.
//!
//! The bundled yt-dlp is copied into the app data folder on first run and
//! that copy is what downloads use, because it can be updated in place
//! (`yt-dlp -U`, which verifies the release signature itself). The install
//! directory may not be writable, and sites like YouTube break old yt-dlp
//! versions within weeks.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

use crate::db::{self, Db};
use crate::model::now_ms;
use crate::parse::version_newer;
use crate::process;

const CHECK_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000;
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
    managed: PathBuf,
    status: Mutex<EngineStatus>,
}

impl Engine {
    pub fn new(app: AppHandle, db: Arc<Db>) -> Self {
        let last_check = db.pref("ytdlp_last_check").and_then(|v| v.parse().ok());
        let version = db.pref("ytdlp_version").filter(|v| !v.is_empty());
        Engine {
            app,
            db,
            bundled_ytdlp: find_sidecar("yt-dlp"),
            ffmpeg: find_sidecar("ffmpeg"),
            managed: db::data_dir().join("bin").join(format!("yt-dlp{EXE}")),
            status: Mutex::new(EngineStatus { version, last_check, ..Default::default() }),
        }
    }

    /// The yt-dlp binary downloads should use.
    pub fn ytdlp(&self) -> Option<PathBuf> {
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

        let version = self.ytdlp().and_then(|p| read_version(&p));
        if let Some(v) = &version {
            let _ = self.db.set_pref("ytdlp_version", v);
        }
        self.set_status(|s| s.version = version);

        let auto = self.db.pref("ytdlp_auto_update").as_deref() != Some("false");
        let due = self
            .status()
            .last_check
            .is_none_or(|t| now_ms() - t > CHECK_INTERVAL_MS);
        if auto && due {
            self.update();
        }
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

    /// Runs `yt-dlp -U` on the managed copy. Blocking.
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

        if !self.managed.is_file() {
            if let Some(b) = &self.bundled_ytdlp {
                self.copy_bundled(b);
            }
        }
        let before = self.status().version;
        let result = if self.managed.is_file() {
            run_update(&self.managed)
        } else {
            Err("The download engine is missing. Reinstall Lynk.".into())
        };

        let now = now_ms();
        let _ = self.db.set_pref("ytdlp_last_check", &now.to_string());
        let after = read_version(&self.managed).or(before.clone());
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
