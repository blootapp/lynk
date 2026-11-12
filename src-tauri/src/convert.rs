//! Making MP4 video open in editing apps.
//!
//! Premiere Pro and After Effects read H.264 but not VP9 or AV1: they import
//! such an MP4 as sound with no picture. YouTube has no H.264 above 1080p, so
//! a 4K MP4 arrives as VP9 (see `parse::video_sort`). MP4 is Lynk's "opens
//! everywhere" format, so once a download is done, video that isn't H.264 is
//! re-encoded to H.264, on the GPU when there is one, and sound in a codec
//! not every MP4 reader takes (Opus, Vorbis, FLAC) becomes AAC. MKV stays as
//! downloaded.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::process;

/// What `ffmpeg -i` says about a file.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Probe {
    /// ffmpeg's codec name: h264, hevc, vp9, av1, …
    pub video: Option<String>,
    pub audio: Option<String>,
    /// Seconds.
    pub duration: Option<f64>,
    pub height: Option<u32>,
}

/// What has to change for the file to open in editing apps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub video: bool,
    pub audio: bool,
}

/// Sound every MP4 reader takes.
const MP4_AUDIO: [&str; 5] = ["aac", "mp3", "ac3", "eac3", "alac"];

pub fn probe(ffmpeg: &Path, file: &Path) -> Option<Probe> {
    let out = process::command(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(file)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let p = parse_probe(&String::from_utf8_lossy(&out.stderr));
    (p.video.is_some() || p.audio.is_some()).then_some(p)
}

/// Reads the first picture and sound streams out of `ffmpeg -i`'s listing.
pub fn parse_probe(text: &str) -> Probe {
    let mut p = Probe::default();
    for line in text.lines().map(str::trim) {
        if p.duration.is_none() {
            if let Some(rest) = line.strip_prefix("Duration: ") {
                p.duration = parse_clock(rest.split(',').next().unwrap_or(""));
            }
        }
        if !line.starts_with("Stream #") {
            continue;
        }
        if let Some(rest) = after(line, "Video: ") {
            // Cover art is a video stream too.
            if p.video.is_none() && !line.contains("(attached pic)") {
                p.video = codec_name(rest);
                p.height = line.split([',', ' ']).find_map(frame_height);
            }
        } else if let Some(rest) = after(line, "Audio: ") {
            if p.audio.is_none() {
                p.audio = codec_name(rest);
            }
        }
    }
    p
}

/// None when the file already opens in editing apps.
pub fn plan(p: &Probe) -> Option<Plan> {
    let video = p.video.as_deref().is_some_and(|c| c != "h264");
    let audio = p.audio.as_deref().is_some_and(|c| !MP4_AUDIO.contains(&c));
    (video || audio).then_some(Plan { video, audio })
}

fn after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    line.find(marker).map(|i| &line[i + marker.len()..])
}

fn codec_name(rest: &str) -> Option<String> {
    let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    (!name.is_empty()).then_some(name)
}

/// "3840x2160" → 2160. Codec tags like "0x31637661" aren't sizes.
fn frame_height(token: &str) -> Option<u32> {
    let (w, h) = token.split_once('x')?;
    if w.is_empty() || w.starts_with('0') || !w.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    h.parse().ok().filter(|h| *h > 0)
}

/// "00:12:34.56" → seconds.
fn parse_clock(s: &str) -> Option<f64> {
    let mut total = 0.0;
    for part in s.trim().split(':') {
        total = total * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(total).filter(|t| *t > 0.0)
}

// ── Encoders ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoder {
    Nvenc,
    Qsv,
    Amf,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    VideoToolbox,
    X264,
}

/// Hardware first; x264 (CPU) always works, so it's always last.
#[cfg(target_os = "macos")]
const CANDIDATES: &[Encoder] = &[Encoder::VideoToolbox, Encoder::X264];
#[cfg(not(target_os = "macos"))]
const CANDIDATES: &[Encoder] = &[Encoder::Nvenc, Encoder::Qsv, Encoder::Amf, Encoder::X264];

impl Encoder {
    pub fn name(self) -> &'static str {
        match self {
            Encoder::Nvenc => "h264_nvenc",
            Encoder::Qsv => "h264_qsv",
            Encoder::Amf => "h264_amf",
            Encoder::VideoToolbox => "h264_videotoolbox",
            Encoder::X264 => "libx264",
        }
    }

    /// Quality-based settings that keep what the source had, as 8-bit 4:2:0
    /// High profile, which every editor and player decodes. Measured on a
    /// 4K VP9 YouTube clip (17.6 Mb/s): NVENC cq 23 gives 31 Mb/s at SSIM
    /// 0.9925 against the source, x264 crf 20 27 Mb/s at 0.9905.
    fn args(self, height: Option<u32>) -> Vec<String> {
        let a: Vec<&str> = match self {
            Encoder::Nvenc => vec!["-preset", "p5", "-rc", "vbr", "-cq", "23", "-b:v", "0", "-pix_fmt", "yuv420p"],
            Encoder::Qsv => vec!["-preset", "medium", "-global_quality", "21", "-pix_fmt", "nv12"],
            Encoder::Amf => vec!["-quality", "balanced", "-rc", "cqp", "-qp_i", "20", "-qp_p", "22", "-qp_b", "24", "-pix_fmt", "nv12"],
            // Intel Macs have no constant-quality mode, so a bitrate by size.
            Encoder::VideoToolbox => vec!["-b:v", toolbox_bitrate(height), "-allow_sw", "1", "-pix_fmt", "yuv420p"],
            Encoder::X264 => vec!["-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p"],
        };
        ["-c:v", self.name()].into_iter().chain(a).chain(["-profile:v", "high"]).map(String::from).collect()
    }
}

fn toolbox_bitrate(height: Option<u32>) -> &'static str {
    match height.unwrap_or(1080) {
        h if h > 1440 => "35M",
        h if h > 1080 => "20M",
        h if h > 720 => "10M",
        h if h > 480 => "6M",
        _ => "3M",
    }
}

static AVAILABLE: Mutex<Option<Vec<Encoder>>> = Mutex::new(None);

/// The encoders to try, in order. ffmpeg lists GPU encoders whether or not
/// the machine has that GPU, so each is checked once per run with a tiny
/// encode; one that still fails on a real file falls through to the next.
pub fn encoders(ffmpeg: &Path) -> Vec<Encoder> {
    let mut cached = AVAILABLE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(list) = cached.as_ref() {
        return list.clone();
    }
    let list: Vec<Encoder> = CANDIDATES
        .iter()
        .copied()
        .filter(|e| *e == Encoder::X264 || works(ffmpeg, *e))
        .collect();
    *cached = Some(list.clone());
    list
}

fn works(ffmpeg: &Path, enc: Encoder) -> bool {
    let mut cmd = process::command(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error"])
        .args(["-f", "lavfi", "-i", "color=c=black:s=1280x720:r=30", "-frames:v", "10"])
        .args(enc.args(Some(720)))
        .args(["-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let Ok((mut child, killer)) = process::spawn(cmd) else { return false };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() < Duration::from_secs(20) => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                killer.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// ffmpeg's arguments for one conversion attempt, reporting on stdout
/// (`-progress`). Keeps the picture (not cover art), every sound track and
/// the tags yt-dlp wrote; streams that are fine are copied as they are.
pub fn args(input: &Path, output: &Path, plan: Plan, enc: Encoder, height: Option<u32>) -> Vec<OsString> {
    let os = |items: &[&str]| items.iter().map(OsString::from).collect::<Vec<_>>();
    let mut a = os(&["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-i"]);
    a.push(input.into());
    a.extend(os(&["-map", "0:V:0?", "-map", "0:a?", "-map_metadata", "0"]));
    if plan.video {
        a.extend(enc.args(height).into_iter().map(OsString::from));
    } else {
        a.extend(os(&["-c:v", "copy"]));
    }
    let audio: &[&str] = if plan.audio { &["-c:a", "aac", "-b:a", "192k"] } else { &["-c:a", "copy"] };
    a.extend(os(audio));
    a.extend(os(&["-movflags", "+faststart"]));
    a.push(output.into());
    a
}

/// Seconds written so far, from a `-progress` line.
pub fn progress_seconds(line: &str) -> Option<f64> {
    let us: f64 = line.trim().strip_prefix("out_time_us=")?.parse().ok()?;
    (us >= 0.0).then_some(us / 1_000_000.0)
}

/// Encoding speed against real time, from a `-progress` line ("speed=2.5x").
pub fn progress_speed(line: &str) -> Option<f64> {
    let v: f64 = line.trim().strip_prefix("speed=")?.trim().trim_end_matches('x').parse().ok()?;
    (v > 0.0).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// What ffmpeg 9 prints for a 4K YouTube download Lynk merged into MP4.
    const VP9_4K: &str = r#"Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'How to Become a Master Video Editor.mp4':
  Metadata:
    major_brand     : isom
  Duration: 00:10:12.48, start: 0.000000, bitrate: 6912 kb/s
  Stream #0:0[0x1](und): Video: vp9 (Profile 0) (vp09 / 0x39307076), yuv420p(tv, bt709), 3840x2160, 6777 kb/s, 24 fps, 24 tbr, 16k tbn (default)
  Stream #0:1[0x2](eng): Audio: aac (LC) (mp4a / 0x6134706D), 44100 Hz, stereo, fltp, 129 kb/s (default)
At least one output file must be specified"#;

    #[test]
    fn reads_codecs_size_and_length() {
        let p = parse_probe(VP9_4K);
        assert_eq!(p.video.as_deref(), Some("vp9"));
        assert_eq!(p.audio.as_deref(), Some("aac"));
        assert_eq!(p.height, Some(2160));
        assert!((p.duration.unwrap() - 612.48).abs() < 1e-6);
        assert_eq!(plan(&p), Some(Plan { video: true, audio: false }));
    }

    #[test]
    fn h264_with_aac_is_left_alone() {
        let p = parse_probe(
            "  Duration: 00:00:30.00, start: 0.000000, bitrate: 900 kb/s\n\
             \x20 Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(tv, bt709, progressive), 1920x1080 [SAR 1:1 DAR 16:9], 778 kb/s, 24 fps\n\
             \x20 Stream #0:1[0x2](eng): Audio: aac (LC) (mp4a / 0x6134706D), 44100 Hz, stereo, fltp, 129 kb/s",
        );
        assert_eq!((p.video.as_deref(), p.height), (Some("h264"), Some(1080)));
        assert_eq!(plan(&p), None);
    }

    #[test]
    fn skips_cover_art_and_flags_opus() {
        let p = parse_probe(
            "  Stream #0:0: Video: mjpeg (Baseline), yuvj420p(pc), 1280x720, 90k tbr (attached pic)\n\
             \x20 Stream #0:1: Video: av1 (libdav1d) (av01 / 0x31307661), yuv420p(tv, bt709), 2560x1440, 30 fps\n\
             \x20 Stream #0:2: Audio: opus (Opus / 0x7375704F), 48000 Hz, stereo, fltp",
        );
        assert_eq!((p.video.as_deref(), p.height, p.audio.as_deref()), (Some("av1"), Some(1440), Some("opus")));
        assert_eq!(plan(&p), Some(Plan { video: true, audio: true }));
    }

    #[test]
    fn conversion_arguments() {
        let (i, o) = (PathBuf::from("in.mp4"), PathBuf::from("in.h264.mp4"));
        let s = |a: Vec<OsString>| a.iter().map(|x| x.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
        let video = s(args(&i, &o, Plan { video: true, audio: false }, Encoder::Nvenc, Some(2160)));
        assert!(video.contains("-i in.mp4 -map 0:V:0? -map 0:a? -map_metadata 0 -c:v h264_nvenc"), "{video}");
        assert!(video.contains("-profile:v high -c:a copy -movflags +faststart in.h264.mp4"), "{video}");
        let sound = s(args(&i, &o, Plan { video: false, audio: true }, Encoder::X264, None));
        assert!(sound.contains("-c:v copy -c:a aac -b:a 192k"), "{sound}");
        assert!(s(args(&i, &o, Plan { video: true, audio: false }, Encoder::VideoToolbox, Some(2160))).contains("-b:v 35M"));
        assert_eq!(CANDIDATES.last(), Some(&Encoder::X264));
    }

    #[test]
    fn reads_progress_lines() {
        assert_eq!(progress_seconds("out_time_us=12500000"), Some(12.5));
        assert_eq!(progress_seconds("out_time_us=N/A"), None);
        assert_eq!(progress_seconds("frame=12"), None);
        assert_eq!(progress_speed("speed=2.5x"), Some(2.5));
        assert_eq!(progress_speed("speed=N/A"), None);
    }
}
