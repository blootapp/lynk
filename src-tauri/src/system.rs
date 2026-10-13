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
