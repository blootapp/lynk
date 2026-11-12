//! Small blocking HTTP helpers for Lynk's own downloads (deno, the PO-token
//! provider, yt-dlp updates). Everything that runs code checks a SHA-256.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

fn client(follow_redirects: bool) -> Result<reqwest::Client, String> {
    // reqwest is built on rustls without a default crypto provider (the
    // updater plugin does the same).
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let policy = if follow_redirects { reqwest::redirect::Policy::limited(10) } else { reqwest::redirect::Policy::none() };
    reqwest::Client::builder()
        .user_agent(concat!("lynk/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .redirect(policy)
        .build()
        .map_err(|e| e.to_string())
}

/// Downloads the first URL that serves a file with the expected SHA-256.
pub fn fetch_verified(urls: &[String], sha256: &str, dest: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let mut last = String::from("no source");
    for url in urls {
        match download(url, dest, cancel) {
            Ok(hash) if hash.eq_ignore_ascii_case(sha256) => return Ok(()),
            Ok(_) => last = format!("{url}: checksum mismatch"),
            Err(e) => last = format!("{url}: {e}"),
        }
        let _ = std::fs::remove_file(dest);
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
    }
    Err(last)
}

/// Streams `url` into `dest`; returns the file's SHA-256 (hex).
pub fn download(url: &str, dest: &Path, cancel: &AtomicBool) -> Result<String, String> {
    let client = client(true)?;
    tauri::async_runtime::block_on(async {
        let mut res = client.get(url).send().await.map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("HTTP {}", res.status().as_u16()));
        }
        let mut file = File::create(dest).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        while let Some(chunk) = res.chunk().await.map_err(|e| e.to_string())? {
            if cancel.load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            hasher.update(&chunk);
            file.write_all(&chunk).map_err(|e| e.to_string())?;
        }
        file.sync_all().map_err(|e| e.to_string())?;
        Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
    })
}

/// A small text file (checksum lists and the like), at most `limit` bytes.
pub fn get_text(url: &str, limit: usize) -> Result<String, String> {
    let client = client(true)?;
    tauri::async_runtime::block_on(async {
        let mut res = client.get(url).send().await.map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("HTTP {}", res.status().as_u16()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = res.chunk().await.map_err(|e| e.to_string())? {
            body.extend_from_slice(&chunk);
            if body.len() > limit {
                return Err("response too large".into());
            }
        }
        String::from_utf8(body).map_err(|e| e.to_string())
    })
}

/// Where `url` redirects to, without following it. GitHub's
/// `/releases/latest` answers with the newest release's own page, which
/// names its tag, and isn't subject to the API's per-address rate limit.
pub fn redirect_target(url: &str) -> Result<String, String> {
    let client = client(false)?;
    tauri::async_runtime::block_on(async {
        let res = client.get(url).send().await.map_err(|e| e.to_string())?;
        if !res.status().is_redirection() {
            return Err(format!("HTTP {} instead of a redirect", res.status().as_u16()));
        }
        res.headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(String::from)
            .ok_or_else(|| "redirect without a location".into())
    })
}
