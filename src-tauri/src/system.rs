//! Launch at login (Bloot §12). The registry value is the source of truth;
//! there is no separate "enabled" preference that could drift from it.

#[cfg(windows)]
mod imp {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    const RUN_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "Lynk";

    /// Starts in the tray instead of opening the window.
    fn command_line() -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        Ok(format!("\"{}\" --minimized", exe.display()))
    }

    fn run_key() -> Result<RegKey, String> {
        RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(RUN_KEY)
            .map(|(k, _)| k)
            .map_err(|e| e.to_string())
    }

    pub fn set(enabled: bool) -> Result<(), String> {
        let key = run_key()?;
        if enabled {
            key.set_value(VALUE, &command_line()?).map_err(|e| e.to_string())
        } else {
            match key.delete_value(VALUE) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            }
        }
    }

    pub fn get() -> bool {
        run_key()
            .and_then(|k| k.get_value::<String, _>(VALUE).map_err(|e| e.to_string()))
            .is_ok()
    }

    /// Keeps the entry pointing at the current executable after an update
    /// or reinstall to a different folder.
    pub fn sync_path() {
        let Ok(key) = run_key() else { return };
        let (Ok(current), Ok(wanted)) = (key.get_value::<String, _>(VALUE), command_line()) else {
            return;
        };
        if current != wanted {
            let _ = key.set_value(VALUE, &wanted);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn set(_enabled: bool) -> Result<(), String> {
        Ok(())
    }
    pub fn get() -> bool {
        false
    }
    pub fn sync_path() {}
}

pub use imp::{get as autostart_enabled, set as set_autostart, sync_path as sync_autostart_path};

// ── Browsers ─────────────────────────────────────────────────────────────────

/// Browsers whose sign-in yt-dlp can use (`--cookies-from-browser`) that are
/// installed here, in the order lynk lists them. "Installed" means the
/// profile folder yt-dlp reads exists; nothing inside is opened until a
/// download uses that browser's sign-in.
pub fn installed_browsers() -> Vec<&'static str> {
    use std::path::PathBuf;
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    #[cfg(windows)]
    let places: Vec<(&'static str, Option<PathBuf>)> = {
        let local = env("LOCALAPPDATA");
        let roaming = env("APPDATA");
        let l = |p: &str| local.as_ref().map(|d| d.join(p));
        let r = |p: &str| roaming.as_ref().map(|d| d.join(p));
        vec![
            ("firefox", r(r"Mozilla\Firefox\Profiles")),
            ("chrome", l(r"Google\Chrome\User Data")),
            ("edge", l(r"Microsoft\Edge\User Data")),
            ("brave", l(r"BraveSoftware\Brave-Browser\User Data")),
            ("opera", r(r"Opera Software\Opera Stable")),
            ("vivaldi", l(r"Vivaldi\User Data")),
            ("chromium", l(r"Chromium\User Data")),
        ]
    };
    #[cfg(not(windows))]
    let places: Vec<(&'static str, Option<PathBuf>)> = {
        let home = env("HOME");
        let s = |p: &str| home.as_ref().map(|h| h.join("Library/Application Support").join(p));
        vec![
            // Every Mac has Safari.
            ("safari", home.clone()),
            ("firefox", s("Firefox/Profiles")),
            ("chrome", s("Google/Chrome")),
            ("edge", s("Microsoft Edge")),
            ("brave", s("BraveSoftware/Brave-Browser")),
            ("opera", s("com.operasoftware.Opera")),
            ("vivaldi", s("Vivaldi")),
            ("chromium", s("Chromium")),
        ]
    };
    places
        .into_iter()
        .filter(|(_, dir)| dir.as_ref().is_some_and(|d| d.is_dir()))
        .map(|(browser, _)| browser)
        .collect()
}
