//! Every download runs in its own hidden folder inside the destination
//! (`<save dir>/.lynk-partial/<id>`), and the finished file is moved into
//! place by Lynk.
//!
//! yt-dlp treats any existing file with the name it wants as its own earlier
//! download: it skips re-downloading, converts that file, and then deletes
//! the "original". Pointed at the user's folder, that silently deleted e.g. an
//! earlier MP3 when the same track was fetched again as M4A. In a private
//! folder it can only ever see its own files, cancelling is a plain folder
//! delete, and a retry still resumes from the partial files.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const DIR: &str = ".lynk-partial";

/// Serializes "pick a free name + rename" so two downloads finishing with
/// the same title can't claim the same file.
static MOVE_LOCK: Mutex<()> = Mutex::new(());

pub fn create(save_dir: &Path, id: &str) -> std::io::Result<PathBuf> {
    let root = save_dir.join(DIR);
    let fresh = !root.exists();
    let dir = root.join(id);
    std::fs::create_dir_all(&dir)?;
    if fresh {
        hide(&root);
    }
    Ok(dir)
}

/// Deletes a download's staging folder, and the parent once it's empty.
pub fn remove(save_dir: &Path, id: &str) {
    let root = save_dir.join(DIR);
    let _ = std::fs::remove_dir_all(root.join(id));
    let _ = std::fs::remove_dir(&root); // fails (harmlessly) unless empty
}

pub fn remove_if_empty(save_dir: &Path, id: &str) {
    let root = save_dir.join(DIR);
    let _ = std::fs::remove_dir(root.join(id));
    let _ = std::fs::remove_dir(&root);
}

/// Moves a finished file into `dest_dir`, never replacing an existing file:
/// a taken name becomes "Name (2).ext", "Name (3).ext", …
pub fn move_into(file: &Path, dest_dir: &Path) -> std::io::Result<PathBuf> {
    let name = file
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?;
    let _guard = MOVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let target = unique_path(dest_dir, name);
    if std::fs::rename(file, &target).is_err() {
        std::fs::copy(file, &target)?;
        let _ = std::fs::remove_file(file);
    }
    Ok(target)
}

pub fn unique_path(dir: &Path, name: &OsStr) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p.file_stem().unwrap_or(name).to_string_lossy();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|c| !c.exists())
        .expect("unbounded range")
}

/// Removes staging folders no download needs any more (`keep` = ids whose
/// partial files a retry could still resume).
pub fn sweep(save_dirs: impl IntoIterator<Item = PathBuf>, keep: &HashSet<String>) {
    for dir in save_dirs {
        let root = dir.join(DIR);
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !keep.contains(&name) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        let _ = std::fs::remove_dir(&root);
    }
}

#[cfg(windows)]
fn hide(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the call.
    unsafe { SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_HIDDEN) };
}

#[cfg(not(windows))]
fn hide(_path: &Path) {
    // The leading dot already hides it on macOS/Linux.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("lynk-staging-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Song.mp3"), b"original").unwrap();
        std::fs::write(dir.join("Song (2).mp3"), b"second").unwrap();

        let staged = create(&dir, "abc").unwrap().join("Song.mp3");
        std::fs::write(&staged, b"new").unwrap();
        let moved = move_into(&staged, &dir).unwrap();
        assert_eq!(moved, dir.join("Song (3).mp3"));
        assert_eq!(std::fs::read(dir.join("Song.mp3")).unwrap(), b"original");
        assert_eq!(std::fs::read(&moved).unwrap(), b"new");

        remove(&dir, "abc");
        assert!(!dir.join(DIR).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweep_keeps_resumable() {
        let dir = std::env::temp_dir().join(format!("lynk-sweep-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create(&dir, "keep-me").unwrap();
        create(&dir, "stale").unwrap();
        sweep([dir.clone()], &HashSet::from(["keep-me".to_string()]));
        assert!(dir.join(DIR).join("keep-me").exists());
        assert!(!dir.join(DIR).join("stale").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
