mod commands;
mod db;
mod downloader;
mod engine;
mod model;
mod parse;
mod process;
mod staging;
mod system;
mod updates;

use std::sync::Arc;
use std::time::Duration;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager as _, RunEvent, WindowEvent};

use db::Db;
use downloader::Manager;
use engine::Engine;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Launched at login: start in the tray (see system.rs).
    let start_hidden = std::env::args().any(|a| a == "--minimized");

    let app = tauri::Builder::default()
        // Must be first: a second launch focuses the running instance.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            let db = Arc::new(Db::open()?);
            db.recover_interrupted()?;
            db::seed_preferences(&db, &commands::default_save_path(app.handle()))?;

            // Launch at login is on by default on first run (Bloot §12) —
            // except for dev builds, which shouldn't register themselves.
            if db.pref("autostart_initialized").as_deref() != Some("true") {
                if !cfg!(debug_assertions) {
                    let _ = system::set_autostart(true);
                }
                let _ = db.set_pref("autostart_initialized", "true");
            } else {
                system::sync_autostart_path();
            }

            let engine = Arc::new(Engine::new(app.handle().clone(), db.clone()));
            let manager = Manager::new(app.handle().clone(), db.clone(), engine.clone());
            let sweeper = manager.clone();
            std::thread::spawn(move || sweeper.sweep_staging());
            app.manage(db);
            app.manage(engine.clone());
            app.manage(manager);
            app.manage(updates::Updates::new());
            if !cfg!(debug_assertions) {
                updates::start(app.handle().clone());
            }

            build_tray(app)?;

            if let Some(w) = app.get_webview_window("main") {
                let hide = w.clone();
                w.on_window_event(move |e| {
                    if let WindowEvent::CloseRequested { api, .. } = e {
                        api.prevent_close();
                        let _ = hide.hide();
                    }
                });
                if !start_hidden {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }

            // Engine check/update in the background; wait longer at login so
            // we don't compete with everything else starting up.
            let delay = if start_hidden { 20 } else { 1 };
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(delay));
                engine.prepare();
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_info,
            commands::window_minimize,
            commands::window_hide,
            commands::app_quit,
            commands::get_preferences,
            commands::set_preference,
            commands::get_default_save_path,
            commands::open_path,
            commands::reveal_path,
            commands::open_dir,
            commands::read_clipboard,
            commands::list_downloads,
            commands::enqueue_downloads,
            commands::cancel_download,
            commands::cancel_group,
            commands::retry_download,
            commands::remove_download,
            commands::clear_history,
            commands::confirm_playlist,
            commands::engine_status,
            commands::engine_update,
            commands::get_autostart,
            commands::set_autostart,
            commands::update_status,
            commands::check_update,
            commands::install_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app, event| match event {
        RunEvent::Exit => {
            if let Some(m) = app.try_state::<Arc<Manager>>() {
                m.shutdown();
            }
        }
        // macOS: clicking the Dock icon brings the hidden window back.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { has_visible_windows, .. } => {
            if !has_visible_windows {
                show_main(app);
            }
        }
        _ => {}
    });
}

fn tray_template() -> tauri::image::Image<'static> {
    tauri::include_image!("icons/tray-macos.png")
}

#[cfg(test)]
mod tests {
    #[test]
    fn tray_template_embeds() {
        let img = super::tray_template();
        assert_eq!((img.width(), img.height()), (64, 64));
    }
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show lynk", true, None::<&str>)?;
    let folder = MenuItem::with_id(app, "folder", "Open downloads folder", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit lynk", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &folder, &separator, &quit])?;

    let mut tray = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .tooltip("lynk")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "folder" => {
                let dir = app
                    .try_state::<Arc<Db>>()
                    .and_then(|db| db.pref("save_path"))
                    .filter(|p| !p.is_empty())
                    .unwrap_or_else(|| commands::default_save_path(app).to_string_lossy().to_string());
                let _ = commands::open_dir(dir);
            }
            "quit" => {
                if let Some(m) = app.try_state::<Arc<Manager>>() {
                    m.shutdown();
                }
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if cfg!(target_os = "macos") {
        // Menu-bar icons are monochrome templates that follow the bar's colour.
        tray = tray.icon(tray_template()).icon_as_template(true);
    } else if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}
