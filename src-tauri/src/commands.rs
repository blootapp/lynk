use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Manager as _, State, WebviewWindow};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::db::Db;
use crate::downloader::Manager;
use crate::engine::{Engine, EngineStatus};
use crate::model::{Download, NewDownload};
use crate::system;
use crate::updates::{self, UpdateStatus, Updates};

type DbState<'a> = State<'a, Arc<Db>>;
type ManagerState<'a> = State<'a, Arc<Manager>>;
type EngineState<'a> = State<'a, Arc<Engine>>;

/// Preference keys the UI may write.
const PREF_KEYS: &[&str] = &[
    "default_quality",
    "default_format",
    "audio_format",
    "video_format",
    "ask_save_path",
    "prefer_h264",
    "max_concurrent",
    "notifications",
    "ytdlp_auto_update",
    "cookies_browser",
    "save_path",
];

// ── App & window ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct AppInfo {
    platform: String,
    version: String,
    app_id: String,
}

#[tauri::command]
pub fn get_app_info() -> AppInfo {
    AppInfo {
        platform: if cfg!(target_os = "windows") { "windows" } else { "macos" }.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        app_id: "lynk".into(),
    }
}

#[tauri::command]
pub fn window_minimize(window: WebviewWindow) {
    let _ = window.minimize();
}

/// The close button hides to the tray (Bloot §7.4); downloads keep going.
#[tauri::command]
pub fn window_hide(window: WebviewWindow) {
    let _ = window.hide();
}

#[tauri::command]
pub fn app_quit(app: AppHandle, manager: ManagerState) {
    manager.shutdown();
    app.exit(0);
}

// ── Preferences ──────────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_preferences(db: DbState) -> Result<HashMap<String, String>, String> {
    db.prefs().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_preference(
    key: String,
    value: String,
    db: DbState,
    manager: ManagerState,
) -> Result<(), String> {
    if !PREF_KEYS.contains(&key.as_str()) {
        return Err(format!("Unknown preference: {key}"));
    }
    db.set_pref(&key, &value).map_err(|e| e.to_string())?;
    if key == "max_concurrent" {
        manager.set_max_concurrent(value.parse().unwrap_or(3));
    }
    Ok(())
}

// ── Files ────────────────────────────────────────────────────────────────────

pub fn default_save_path(app: &AppHandle) -> PathBuf {
    let base = app.path().download_dir().unwrap_or_else(|_| {
        PathBuf::from(std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default())
            .join("Downloads")
    });
    base.join("Lynk")
}

#[tauri::command]
pub fn get_default_save_path(app: AppHandle) -> String {
    default_save_path(&app).to_string_lossy().to_string()
}

/// Opens a file with its default app.
#[tauri::command(async)]
pub fn open_path(path: String) -> Result<(), String> {
    if !Path::new(&path).is_file() {
        return Err("The file was moved or deleted.".into());
    }
    tauri_plugin_opener::open_path(&path, None::<&str>).map_err(|e| e.to_string())
}

/// Shows a file selected in Explorer/Finder; falls back to its folder.
#[tauri::command(async)]
pub fn reveal_path(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if p.exists() {
        return tauri_plugin_opener::reveal_item_in_dir(p).map_err(|e| e.to_string());
    }
    match p.parent().filter(|d| d.is_dir()) {
        Some(dir) => tauri_plugin_opener::open_path(dir, None::<&str>).map_err(|e| e.to_string()),
        None => Err("The file was moved or deleted.".into()),
    }
}

/// Opens a folder, creating it first if needed.
#[tauri::command(async)]
pub fn open_dir(path: String) -> Result<(), String> {
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    tauri_plugin_opener::open_path(&path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn read_clipboard(app: AppHandle) -> Option<String> {
    app.clipboard()
        .read_text()
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty() && t.len() < 8192)
}

// ── Downloads ────────────────────────────────────────────────────────────────

/// History from the database, overlaid with live state for anything running.
#[tauri::command(async)]
pub fn list_downloads(db: DbState, manager: ManagerState) -> Result<Vec<Download>, String> {
    let mut rows = db.list().map_err(|e| e.to_string())?;
    let mut live: HashMap<String, Download> =
        manager.snapshot().into_iter().map(|d| (d.id.clone(), d)).collect();
    for row in rows.iter_mut() {
        if let Some(d) = live.remove(&row.id) {
            *row = d;
        } else if row.status == "done" {
            match row.file_path.as_deref().map(std::fs::metadata) {
                Some(Ok(meta)) => {
                    if row.size_bytes.is_none() {
                        row.size_bytes = Some(meta.len() as i64);
                    }
                }
                _ => row.file_missing = true,
            }
        }
    }
    // Live records not in the list yet (pending playlist prompts).
    rows.extend(live.into_values());
    rows.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(rows)
}

#[tauri::command]
pub fn enqueue_downloads(
    items: Vec<NewDownload>,
    manager: ManagerState,
) -> Result<Vec<Download>, String> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    for it in &items {
        let scheme_ok = it.url.starts_with("https://") || it.url.starts_with("http://");
        if !scheme_ok || it.url.len() > 4096 || it.url.chars().any(char::is_whitespace) {
            return Err(format!("Not a valid link: {}", it.url));
        }
        if it.format != "video" && it.format != "audio" {
            return Err("Unknown format".into());
        }
        if it.audio_format != "mp3" && it.audio_format != "m4a" {
            return Err("Unknown audio format".into());
        }
        if it.video_format != "mp4" && it.video_format != "mkv" {
            return Err("Unknown video format".into());
        }
        if it.save_path.trim().is_empty() {
            return Err("No save folder is set.".into());
        }
    }
    manager.enqueue(items)
}

#[tauri::command]
pub fn cancel_download(id: String, manager: ManagerState) {
    manager.cancel(&id);
}

#[tauri::command]
pub fn cancel_group(group_id: String, manager: ManagerState) {
    manager.cancel_group(&group_id);
}

#[tauri::command]
pub fn retry_download(id: String, manager: ManagerState) -> Result<Download, String> {
    manager.retry(&id)
}

#[tauri::command]
pub fn remove_download(id: String, manager: ManagerState) {
    manager.remove(&id);
}

#[tauri::command]
pub fn clear_history(manager: ManagerState) -> Result<Vec<String>, String> {
    manager.clear_history()
}

#[tauri::command]
pub fn confirm_playlist(id: String, manager: ManagerState) -> Result<usize, String> {
    manager.confirm_playlist(&id)
}

// ── Engine ───────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn engine_status(engine: EngineState) -> EngineStatus {
    engine.status()
}

#[tauri::command]
pub fn engine_update(engine: EngineState) {
    let engine = engine.inner().clone();
    std::thread::spawn(move || engine.update());
}

// ── App updates ──────────────────────────────────────────────────────────────

#[tauri::command]
pub fn update_status(updates: State<'_, Arc<Updates>>) -> UpdateStatus {
    updates.status()
}

#[tauri::command]
pub async fn check_update(app: AppHandle) -> UpdateStatus {
    updates::check(&app).await
}

#[tauri::command]
pub fn install_update(app: AppHandle) -> Result<(), String> {
    updates::install(&app)
}

// ── Launch at login ──────────────────────────────────────────────────────────

#[tauri::command(async)]
pub fn get_autostart() -> bool {
    system::autostart_enabled()
}

#[tauri::command(async)]
pub fn set_autostart(enabled: bool, db: DbState) -> Result<(), String> {
    system::set_autostart(enabled)?;
    let _ = db.set_pref("autostart_initialized", "true");
    Ok(())
}
