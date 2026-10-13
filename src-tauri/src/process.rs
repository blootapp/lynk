//! Spawning helper processes (yt-dlp, which in turn runs ffmpeg).
//!
//! On Windows every download runs inside its own Job Object:
//! - cancelling terminates the whole tree (yt-dlp's PyInstaller bootloader,
//!   the Python child, and any ffmpeg it started) in one call;
//! - the job is closed when Lynk exits or crashes, which kills anything
//!   still running instead of leaving orphans behind.

use std::path::Path;
use std::process::{Child, Command};
use std::sync::Arc;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A command that never flashes a console window and speaks UTF-8.
pub fn command(program: &Path) -> Command {
    let mut c = Command::new(program);
    c.env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    c
}

/// Cloneable handle that can kill a spawned process tree.
#[derive(Clone)]
pub struct Killer(Arc<KillerInner>);

pub fn spawn(mut cmd: Command) -> std::io::Result<(Child, Killer)> {
    let child = cmd.spawn()?;
    let inner = KillerInner::attach(&child);
    Ok((child, Killer(Arc::new(inner))))
}

impl Killer {
    pub fn kill(&self) {
        self.0.kill();
    }
}

#[cfg(windows)]
struct KillerInner {
    job: usize,
    pid: u32,
}

#[cfg(windows)]
impl KillerInner {
    fn attach(child: &Child) -> Self {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        // SAFETY: plain Win32 calls on handles we own; failures leave `job`
        // at 0 and we fall back to taskkill.
        let job = unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                0
            } else {
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE);
                job as usize
            }
        };
        KillerInner { job, pid: child.id() }
    }

    fn kill(&self) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        if self.job != 0 {
            // SAFETY: `job` is a live job handle owned by this struct.
            unsafe { TerminateJobObject(self.job as _, 1) };
        } else {
            let _ = command(Path::new("taskkill"))
                .args(["/PID", &self.pid.to_string(), "/T", "/F"])
                .status();
        }
    }
}

#[cfg(windows)]
impl Drop for KillerInner {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        if self.job != 0 {
            // SAFETY: closing our own handle exactly once.
            unsafe { CloseHandle(self.job as _) };
        }
    }
}

#[cfg(unix)]
struct KillerInner {
    pid: u32,
}

#[cfg(unix)]
impl KillerInner {
    fn attach(child: &Child) -> Self {
        KillerInner { pid: child.id() }
    }

    fn kill(&self) {
        // The child leads its own process group (see `command`), so a
        // negative pid stops yt-dlp and any ffmpeg it started together.
        // SAFETY: a plain syscall; a stale pid only yields ESRCH.
        unsafe { libc::kill(-(self.pid as libc::pid_t), libc::SIGKILL) };
    }
}

/// Reads a pipe line by line, decoding lossily so one bad byte can't drop a
/// line (or a title) on the floor. Lines are tagged with `is_stderr`.
pub fn forward_lines<R: std::io::Read + Send + 'static>(
    reader: R,
    tx: std::sync::mpsc::Sender<(bool, String)>,
    is_stderr: bool,
) {
    use std::io::BufRead;
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(reader);
        let mut buf = Vec::with_capacity(512);
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf)
                        .trim_end_matches(['\r', '\n'])
                        .to_string();
                    if tx.send((is_stderr, line)).is_err() {
                        break;
                    }
                }
            }
        }
    });
}
