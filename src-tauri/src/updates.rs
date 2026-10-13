//! Self-update from Cloudflare: the manifest and packages are served by
//! api.bloot.app from the R2 release bucket (see scripts/publish.mjs).
//!
//! Lynk checks quietly in the background and downloads a signed update, but
//! installs it only when the user clicks "Restart" — never in the middle of
//! their downloads.

use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager as _};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::downloader::Manager;

const FIRST_CHECK: Duration = Duration::from_secs(30);
const INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateStatus {
    /// `idle` | `checking` | `downloading` | `ready` | `current` | `error`
    pub state: String,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Updates {
    status: Mutex<UpdateStatus>,
    ready: Mutex<Option<(Update, Vec<u8>)>>,
}

impl Updates {
    pub fn new() -> Arc<Self> {
        Arc::new(Updates {
            status: Mutex::new(UpdateStatus { state: "idle".into(), ..Default::default() }),
            ready: Mutex::new(None),
        })
    }

    pub fn status(&self) -> UpdateStatus {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn set(app: &AppHandle, status: UpdateStatus) -> UpdateStatus {
    if let Some(u) = app.try_state::<Arc<Updates>>() {
        *u.status.lock().unwrap_or_else(|e| e.into_inner()) = status.clone();
    }
    let _ = app.emit("app:update", &status);
    status
}

/// Background schedule: shortly after launch, then every few hours.
pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(FIRST_CHECK);
        loop {
            tauri::async_runtime::block_on(check(&app));
            std::thread::sleep(INTERVAL);
        }
    });
}

/// Checks for an update and downloads it if there is one.
pub async fn check(app: &AppHandle) -> UpdateStatus {
    let Some(updates) = app.try_state::<Arc<Updates>>() else {
        return UpdateStatus::default();
    };
    let current = updates.status();
    if matches!(current.state.as_str(), "ready" | "checking" | "downloading") {
        return current;
    }
    set(app, UpdateStatus { state: "checking".into(), ..Default::default() });

    let fail = |e: String| UpdateStatus { state: "error".into(), error: Some(e), ..Default::default() };
    let update = match app.updater() {
        Ok(u) => u.check().await,
        Err(e) => return set(app, fail(e.to_string())),
    };
    let update = match update {
        Ok(Some(u)) => u,
        Ok(None) => return set(app, UpdateStatus { state: "current".into(), ..Default::default() }),
        Err(e) => return set(app, fail(friendly(&e.to_string()))),
    };

    let version = Some(update.version.clone());
    let notes = update.body.clone().filter(|b| !b.trim().is_empty());
    set(app, UpdateStatus { state: "downloading".into(), version: version.clone(), ..Default::default() });
    match update.download(|_, _| {}, || {}).await {
        Ok(bytes) => {
            *updates.ready.lock().unwrap_or_else(|e| e.into_inner()) = Some((update, bytes));
            set(app, UpdateStatus { state: "ready".into(), version, notes, error: None })
        }
        Err(e) => set(app, fail(friendly(&e.to_string()))),
    }
}

/// Installs the downloaded update and restarts. Running downloads are
/// stopped first and resume (from their partial files) after the restart.
pub fn install(app: &AppHandle) -> Result<(), String> {
    let updates = app.try_state::<Arc<Updates>>().ok_or("Updater isn't ready.")?;
    let (update, bytes) = updates
        .ready
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .ok_or("No update is ready yet.")?;
    if let Some(m) = app.try_state::<Arc<Manager>>() {
        m.shutdown();
    }
    // On Windows this hands over to the installer and exits the process.
    update
        .install(bytes)
        .map_err(|e| format!("The update couldn't be installed ({e}). Quit and reopen Lynk to try again."))?;
    app.restart()
}

fn friendly(raw: &str) -> String {
    let l = raw.to_lowercase();
    if l.contains("signature") {
        "The update wasn't signed by bloot, so it was skipped.".into()
    } else if l.contains("network") || l.contains("connect") || l.contains("dns") || l.contains("timed out") {
        "Couldn't reach the update server.".into()
    } else {
        "Couldn't check for updates.".into()
    }
}
