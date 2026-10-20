//! What YouTube needs beyond yt-dlp itself.
//!
//! - A JavaScript runtime (deno). YouTube hides its stream URLs behind JS
//!   challenges; without a runtime yt-dlp falls back to the few clients that
//!   skip them, which YouTube is phasing out. deno is fetched once, pinned and
//!   SHA-256-verified, into the app data folder — shortly after launch, or
//!   right before the first YouTube download if that comes first.
//! - A PO-token provider (bgutil), used only to retry a YouTube download that
//!   was blocked ("not a bot", 403). It is a ~250 MB local server, so it is
//!   downloaded the first time it's needed, runs on 127.0.0.1:4416 and stops
//!   again when idle. yt-dlp's bgutil plugin asks it for tokens; the plugin
//!   lives in Lynk's own folder and is only loaded for those retries.
//!
//! Everything here is best-effort: when a step fails, downloads run exactly
//! as they would without it.

use std::fs::File;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::db;
use crate::process::{self, Killer};

const EXE: &str = if cfg!(windows) { ".exe" } else { "" };
const MIRROR: &str = "https://api.bloot.app/download";

const DENO_VERSION: &str = "2.9.7";
const POT_PORT: u16 = 4416;
/// The provider is stopped after this long without a retry needing it.
const POT_IDLE: Duration = Duration::from_secs(15 * 60);
/// After a failed setup, don't make every download wait on another attempt.
const RETRY_AFTER: Duration = Duration::from_secs(10 * 60);

const PLUGIN_FILES: [(&str, &str); 2] = [
    ("getpot_bgutil.py", include_str!("../vendor/bgutil-ytdlp-pot-provider/getpot_bgutil.py")),
    ("getpot_bgutil_http.py", include_str!("../vendor/bgutil-ytdlp-pot-provider/getpot_bgutil_http.py")),
];

struct Asset {
    target: &'static str,
    sha256: &'static str,
}

#[cfg(windows)]
const DENO: Option<Asset> = Some(Asset {
    target: "x86_64-pc-windows-msvc",
    sha256: "a0c3101b4158d1dfb7d6a78a7bf0f3de80c96bb423c152beec8beb22786f2238",
});
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const DENO: Option<Asset> = Some(Asset {
    target: "aarch64-apple-darwin",
    sha256: "5cd46d6268f6f78f5d88bdc7159d20bd44cdaa4b3303474839f87ec6fe7ae25c",
});
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const DENO: Option<Asset> = Some(Asset {
    target: "x86_64-apple-darwin",
    sha256: "95daaff11c116a52ad54785e7914c8e9c9cdcaba793c5ed929c74ca2d8e6259a",
});
#[cfg(not(any(windows, target_os = "macos")))]
const DENO: Option<Asset> = None;

// The provider is a deno-compiled build of bgutil's server; there is no Intel
// Mac build, so those Macs simply skip the token retry.
#[cfg(windows)]
const POT: Option<Asset> = Some(Asset {
    target: "x86_64-pc-windows-msvc",
    sha256: "17a539945b0593fff2567f3f68ab9c44486f8f8f14858a4bab3511fd060876cf",
});
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const POT: Option<Asset> = Some(Asset {
    target: "aarch64-apple-darwin",
    sha256: "4414230bb43ed61be7dc30868d3baa0ae5a379e6544ab6a56b3a776921f0bcb4",
});
#[cfg(not(any(windows, all(target_os = "macos", target_arch = "aarch64"))))]
const POT: Option<Asset> = None;

/// True for links yt-dlp hands to its YouTube extractor.
pub fn is_youtube(url: &str) -> bool {
    let rest = url.trim().split_once("://").map_or(url.trim(), |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("").trim_end_matches('.').to_ascii_lowercase();
    ["youtube.com", "youtu.be", "youtube-nocookie.com"]
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// A failure that a PO token can plausibly get past.
pub fn is_blocked(raw: &str) -> bool {
    let l = raw.to_lowercase();
    [
        "not a bot",
        "http error 403",
        "403: forbidden",
        "po token",
        "sabr",
        "try again later",
        "only images are available",
        "requested format is not available",
    ]
    .iter()
    .any(|n| l.contains(n))
}

struct Provider {
    child: Child,
    _killer: Killer,
    last_used: Instant,
}

pub struct YouTube {
    bin: PathBuf,
    plugins: PathBuf,
    deno_setup: Mutex<Option<Instant>>,
    pot_setup: Mutex<Option<Instant>>,
    provider: Arc<Mutex<Option<Provider>>>,
}

impl YouTube {
    pub fn new() -> Self {
        let data = db::data_dir();
        YouTube {
            bin: data.join("bin"),
            plugins: data.join("yt-dlp-plugins"),
            deno_setup: Mutex::new(None),
            pot_setup: Mutex::new(None),
            provider: Arc::new(Mutex::new(None)),
        }
    }

    fn deno_path(&self) -> PathBuf {
        self.bin.join(format!("deno{EXE}"))
    }

    fn deno_current(&self) -> bool {
        std::fs::read_to_string(self.bin.join("deno.version")).is_ok_and(|v| v.trim() == DENO_VERSION)
    }

    /// The JS runtime yt-dlp should use, when one has been installed.
    pub fn deno(&self) -> Option<PathBuf> {
        Some(self.deno_path()).filter(|p| p.is_file())
    }

    /// Whether a YouTube download should wait for deno to be set up first.
    pub fn deno_missing(&self) -> bool {
        DENO.is_some() && self.deno().is_none() && !self.cooling_down(&self.deno_setup)
    }

    fn cooling_down(&self, last_failure: &Mutex<Option<Instant>>) -> bool {
        lock(last_failure).is_some_and(|t| t.elapsed() < RETRY_AFTER)
    }

    /// Installs (or upgrades) deno. Blocking; returns the runtime if one is
    /// ready. Concurrent callers wait for the first one.
    pub fn ensure_deno(&self, cancel: &AtomicBool) -> Option<PathBuf> {
        let asset = DENO.as_ref()?;
        let mut failed = lock_or_cancel(&self.deno_setup, cancel)?;
        if self.deno().is_some() && self.deno_current() {
            return self.deno();
        }
        if failed.is_some_and(|t| t.elapsed() < RETRY_AFTER) {
            return self.deno();
        }
        match self.install_deno(asset, cancel) {
            Ok(()) => *failed = None,
            Err(_) if cancel.load(Ordering::SeqCst) => {}
            Err(_) => *failed = Some(Instant::now()),
        }
        self.deno()
    }

    fn install_deno(&self, asset: &Asset, cancel: &AtomicBool) -> Result<(), String> {
        std::fs::create_dir_all(&self.bin).map_err(|e| e.to_string())?;
        let zip_path = self.bin.join("deno.zip.part");
        let urls = [
            format!("{MIRROR}/deno-{DENO_VERSION}-{}.zip", asset.target),
            format!("https://dl.deno.land/release/v{DENO_VERSION}/deno-{}.zip", asset.target),
        ];
        let result = fetch_verified(&urls, asset.sha256, &zip_path, cancel).and_then(|_| {
            let tmp = self.bin.join(format!("deno{EXE}.part"));
            extract_deno(&zip_path, &tmp)?;
            finish_executable(&tmp, &self.deno_path())?;
            std::fs::write(self.bin.join("deno.version"), DENO_VERSION).map_err(|e| e.to_string())
        });
        let _ = std::fs::remove_file(&zip_path);
        result
    }

    /// Gets a PO-token provider answering on 127.0.0.1:4416 — one already
    /// running (another bloot app may share it) or one Lynk starts — and
    /// returns the plugin folder to hand yt-dlp. Blocking; `None` when the
    /// token retry isn't possible.
    pub fn ensure_provider(&self, cancel: &AtomicBool) -> Option<PathBuf> {
        let asset = POT.as_ref()?;
        let plugins = self.install_plugin().ok()?;
        let mut failed = lock_or_cancel(&self.pot_setup, cancel)?;
        {
            let mut running = lock(&self.provider);
            if let Some(p) = running.as_mut() {
                if matches!(p.child.try_wait(), Ok(None)) {
                    p.last_used = Instant::now();
                } else {
                    *running = None;
                }
            }
        }
        if provider_up() {
            return Some(plugins);
        }
        if failed.is_some_and(|t| t.elapsed() < RETRY_AFTER) {
            return None;
        }
        match self.start_provider(asset, cancel) {
            Ok(()) => {
                *failed = None;
                Some(plugins)
            }
            Err(_) => {
                if !cancel.load(Ordering::SeqCst) {
                    *failed = Some(Instant::now());
                }
                None
            }
        }
    }

    fn start_provider(&self, asset: &Asset, cancel: &AtomicBool) -> Result<(), String> {
        let path = self.bin.join(format!("bgutil-pot{EXE}"));
        let stamp = self.bin.join("bgutil-pot.sha256");
        let current = std::fs::read_to_string(&stamp).is_ok_and(|v| v.trim() == asset.sha256);
        if !path.is_file() || !current {
            std::fs::create_dir_all(&self.bin).map_err(|e| e.to_string())?;
            let gz = self.bin.join("bgutil-pot.gz.part");
            let url = format!("{MIRROR}/qooti-pot-{}.gz", asset.target);
            let result = fetch_verified(&[url], asset.sha256, &gz, cancel).and_then(|_| {
                let tmp = self.bin.join(format!("bgutil-pot{EXE}.part"));
                gunzip(&gz, &tmp)?;
                finish_executable(&tmp, &path)?;
                std::fs::write(&stamp, asset.sha256).map_err(|e| e.to_string())
            });
            let _ = std::fs::remove_file(&gz);
            result?;
        }

        let mut cmd = process::command(&path);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let (mut child, killer) = process::spawn(cmd).map_err(|e| e.to_string())?;
        // BotGuard takes a few seconds to initialise.
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(12) {
            if cancel.load(Ordering::SeqCst) {
                killer.kill();
                let _ = child.wait();
                return Err("cancelled".into());
            }
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("The token provider exited ({status})."));
            }
            if provider_up() {
                *lock(&self.provider) = Some(Provider { child, _killer: killer, last_used: Instant::now() });
                self.stop_when_idle();
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        killer.kill();
        let _ = child.wait();
        Err("The token provider didn't start.".into())
    }

    fn stop_when_idle(&self) {
        let provider = self.provider.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(60));
            let mut running = lock(&provider);
            let Some(p) = running.as_mut() else { return };
            if p.last_used.elapsed() >= POT_IDLE || !matches!(p.child.try_wait(), Ok(None)) {
                if let Some(mut p) = running.take() {
                    let _ = p.child.kill();
                    let _ = p.child.wait();
                }
                return;
            }
        });
    }

    /// Writes the bgutil plugin into Lynk's own plugin folder, which yt-dlp
    /// only sees when pointed at it with `--plugin-dirs`.
    fn install_plugin(&self) -> std::io::Result<PathBuf> {
        let dir = self.plugins.join("bgutil").join("yt_dlp_plugins").join("extractor");
        std::fs::create_dir_all(&dir)?;
        for (name, source) in PLUGIN_FILES {
            let file = dir.join(name);
            if std::fs::read_to_string(&file).ok().as_deref() != Some(source) {
                std::fs::write(&file, source)?;
            }
        }
        Ok(self.plugins.clone())
    }

    /// Background warm-up: makes sure deno is there before it's needed.
    pub fn prefetch(&self) {
        if DENO.is_some() && !(self.deno().is_some() && self.deno_current()) {
            self.ensure_deno(&AtomicBool::new(false));
        }
    }

    /// Stops the provider Lynk started (never one another app runs).
    pub fn shutdown(&self) {
        if let Some(mut p) = lock(&self.provider).take() {
            let _ = p.child.kill();
            let _ = p.child.wait();
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Waits for a setup lock another download may be holding, giving up when
/// this download is cancelled.
fn lock_or_cancel<'a, T>(m: &'a Mutex<T>, cancel: &AtomicBool) -> Option<MutexGuard<'a, T>> {
    loop {
        match m.try_lock() {
            Ok(g) => return Some(g),
            Err(std::sync::TryLockError::Poisoned(e)) => return Some(e.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => {
                if cancel.load(Ordering::SeqCst) {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

fn provider_up() -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], POT_PORT));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    if s.write_all(b"GET /ping HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").is_err() {
        return false;
    }
    let mut head = [0u8; 16];
    let mut n = 0;
    while n < head.len() {
        match s.read(&mut head[n..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k,
        }
    }
    let line = String::from_utf8_lossy(&head[..n]);
    line.starts_with("HTTP/1.") && line.get(9..12) == Some("200")
}

/// Downloads the first URL that serves a file with the expected SHA-256.
fn fetch_verified(urls: &[String], sha256: &str, dest: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let mut last = String::from("no source");
    for url in urls {
        match download(url, dest, cancel) {
            Ok(hash) if hash == sha256 => return Ok(()),
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
fn download(url: &str, dest: &Path, cancel: &AtomicBool) -> Result<String, String> {
    // reqwest is built on rustls without a default crypto provider (the
    // updater plugin does the same).
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    tauri::async_runtime::block_on(async {
        let client = reqwest::Client::builder()
            .user_agent(concat!("lynk/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?;
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

fn extract_deno(zip_path: &Path, dest: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(File::open(zip_path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut entry = archive.by_name(&format!("deno{EXE}")).map_err(|e| e.to_string())?;
    let mut out = File::create(dest).map_err(|e| e.to_string())?;
    std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    Ok(())
}

fn gunzip(src: &Path, dest: &Path) -> Result<(), String> {
    let input = std::io::BufReader::new(File::open(src).map_err(|e| e.to_string())?);
    let mut decoder = flate2::read::GzDecoder::new(input);
    let mut out = File::create(dest).map_err(|e| e.to_string())?;
    std::io::copy(&mut decoder, &mut out).map_err(|e| e.to_string())?;
    Ok(())
}

/// Makes a freshly written binary runnable and moves it into place.
fn finish_executable(tmp: &Path, dest: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(0o755));
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("xattr").args(["-d", "com.apple.quarantine"]).arg(tmp).output();
    std::fs::rename(tmp, dest).map_err(|e| {
        let _ = std::fs::remove_file(tmp);
        e.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_hosts() {
        for url in [
            "https://www.youtube.com/watch?v=ZPlRjqNNdXs",
            "https://youtu.be/SII_dSf1_hw",
            "youtu.be/qtl2fg-xc30",
            "https://music.youtube.com/watch?v=x",
            "https://m.youtube.com/shorts/x",
            "https://www.youtube-nocookie.com/embed/x",
            "HTTPS://WWW.YOUTUBE.COM:443/watch?v=x",
        ] {
            assert!(is_youtube(url), "{url}");
        }
        for url in [
            "https://vimeo.com/1",
            "https://notyoutube.com/watch?v=x",
            "https://example.com/?u=https://youtube.com/watch",
            "https://youtube.com.evil.example/watch",
        ] {
            assert!(!is_youtube(url), "{url}");
        }
    }

    #[test]
    fn blocked_errors() {
        assert!(is_blocked("[youtube] x: Sign in to confirm you’re not a bot. Use --cookies-from-browser"));
        assert!(is_blocked("unable to download video data: HTTP Error 403: Forbidden"));
        assert!(!is_blocked("[youtube] x: Private video. Sign in if you've been granted access"));
        assert!(!is_blocked("Unsupported URL: https://example.com"));
    }

    /// Downloads deno and the provider for real (~130 MB) into the data
    /// folder that APPDATA / HOME points at:
    /// `APPDATA=<scratch> cargo test -- --ignored provisions`
    #[test]
    #[ignore]
    fn provisions_deno_and_provider() {
        let yt = YouTube::new();
        let cancel = AtomicBool::new(false);
        assert!(yt.ensure_deno(&cancel).is_some_and(|p| p.is_file()));
        assert!(yt.deno_current());
        let plugins = yt.ensure_provider(&cancel).expect("provider");
        assert!(provider_up());
        assert!(plugins.join("bgutil/yt_dlp_plugins/extractor/getpot_bgutil_http.py").is_file());
        yt.shutdown();
    }

    #[test]
    fn plugin_is_embedded() {
        assert!(PLUGIN_FILES[0].1.contains("class BgUtilPTPBase"));
        assert!(PLUGIN_FILES[1].1.contains("127.0.0.1:4416"));
    }
}
