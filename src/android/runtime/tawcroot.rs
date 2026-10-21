//! tawcroot, the opt-in guest engine.
//!
//! `libtawcroot.so` (third_party/tawcroot, MIT) runs guest programs with a
//! seccomp filter whose trapped syscalls are translated by a SIGSYS handler
//! inside the guest process itself, instead of PRoot's ptrace round trip to
//! a tracer process. Path syscalls cost a few microseconds instead of tens.
//!
//! It is chosen by writing `tawcroot` to `<files>/guest-engine`, checked once
//! per app process: the choice never changes under a running session, and a
//! device whose kernel cannot run the engine keeps PRoot. Both engines share
//! PRoot's fake-root records (`.proot-meta-file.*`), so ownership survives a
//! switch; hardlinks made under tawcroot live in its own store
//! (`<files>/tawcroot`) and read as dangling symlinks under PRoot.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use crate::android::diagnostics;
use crate::android::utils::application_context::get_application_context;
use crate::core::config;

const ENGINE_CHOICE_FILE: &str = "guest-engine";
const ENGINE_BINARY: &str = "libtawcroot.so";
const SMOKE_TIMEOUT: Duration = Duration::from_secs(10);
const INSTANCES_DIR: &str = "tawcroot-instances";
const TMPFS_DIR: &str = ".tawcroot-tmpfs";
/// Where an instance's marker directory appears in the guest. Absent from
/// the rootfs, so it never shows up in a listing of `/`.
pub const INSTANCE_GUEST_PATH: &str = "/.tawcroot-instance";

/// Whether guest commands in this app process run under tawcroot.
pub fn selected() -> bool {
    static SELECTED: OnceLock<bool> = OnceLock::new();
    *SELECTED.get_or_init(|| {
        let choice = fs::read_to_string(Path::new(config::APP_FILES_ROOT).join(ENGINE_CHOICE_FILE))
            .unwrap_or_default();
        if choice.trim() != "tawcroot" {
            return false;
        }
        let usable = kernel_supports_trap() && smoke_exec();
        diagnostics::host_event(
            "engine-select",
            if usable {
                "engine=tawcroot"
            } else {
                "engine=proot reason=tawcroot-unusable"
            },
        );
        log::info!(
            "Guest engine: {}",
            if usable {
                "tawcroot"
            } else {
                "proot (tawcroot unusable)"
            }
        );
        usable
    })
}

pub fn binary() -> PathBuf {
    get_application_context()
        .native_library_dir
        .join(ENGINE_BINARY)
}

/// The engine's filter returns SECCOMP_RET_TRAP. When the kernel lists its
/// actions and trap is not among them, the engine cannot run.
fn kernel_supports_trap() -> bool {
    match fs::read_to_string("/proc/sys/kernel/seccomp/actions_avail") {
        Ok(actions) => actions.split_whitespace().any(|action| action == "trap"),
        Err(_) => true,
    }
}

/// Run the support probe binary (extracted by `PRootRuntime::is_supported`)
/// through the whole engine path: filter install, SIGSYS dispatch, exec.
fn smoke_exec() -> bool {
    let context = get_application_context();
    let probe = context.data_dir.join(super::proot::SUPPORT_CHECK_BINARY);
    if !probe.is_file() || !binary().is_file() {
        diagnostics::host_event("engine-probe", "tawcroot missing-binary-or-probe");
        return false;
    }
    let mut child = match Command::new(binary())
        .arg("-r")
        .arg(&context.data_dir)
        .arg("--")
        .arg(format!("/{}", super::proot::SUPPORT_CHECK_BINARY))
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            diagnostics::host_event("engine-probe", &format!("tawcroot spawn-error={error}"));
            return false;
        }
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                diagnostics::host_event(
                    "engine-probe",
                    &format!("tawcroot status={:?}", status.code()),
                );
                return status.success();
            }
            Ok(None) if started.elapsed() < SMOKE_TIMEOUT => {
                thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                diagnostics::host_event("engine-probe", "tawcroot timeout");
                return false;
            }
        }
    }
}

/// One engine instance, for PRoot's `--kill-on-exit`. tawcroot keeps a
/// reserved fd on every bind source in every guest process, across fork,
/// exec and setsid, so binding a fresh directory marks exactly this
/// instance's processes, including daemons that left the process group.
pub struct Instance {
    dir: PathBuf,
}

impl Instance {
    pub fn new() -> io::Result<Self> {
        static SWEPT: OnceLock<()> = OnceLock::new();
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = Path::new(config::APP_FILES_ROOT).join(INSTANCES_DIR);
        // Markers left by an earlier app process: Android killed their
        // holders with it.
        // So are the private tmpfs directories their sandboxes mounted
        // (tawcroot makes them beside the rootfs).
        SWEPT.get_or_init(|| {
            let _ = fs::remove_dir_all(&root);
            let _ = fs::remove_dir_all(Path::new(config::APP_FILES_ROOT).join(TMPFS_DIR));
        });
        let dir = root.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn bind_source(&self) -> &Path {
        &self.dir
    }

    /// SIGKILL every process that still holds the marker. Repeats while
    /// it finds some, since a member may fork between the scan and the
    /// kill. Returns how many were killed.
    #[cfg(target_os = "android")]
    pub fn kill_members(&self) -> usize {
        let mut killed = 0;
        for _ in 0..5 {
            let members = marker_holders(&self.dir);
            if members.is_empty() {
                break;
            }
            for pid in &members {
                unsafe {
                    libc::kill(*pid, libc::SIGKILL);
                }
            }
            killed += members.len();
            thread::sleep(Duration::from_millis(20));
        }
        killed
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.dir);
    }
}

/// Processes with an open fd on `dir`. Only the app's own processes are
/// visible to it, and only those can be engine guests.
#[cfg(target_os = "android")]
fn marker_holders(dir: &Path) -> Vec<libc::pid_t> {
    let me = std::process::id() as libc::pid_t;
    let Ok(procs) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    procs
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<libc::pid_t>().ok())
        .filter(|pid| *pid != me)
        .filter(|pid| {
            let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) else {
                return false;
            };
            fds.flatten().any(|fd| {
                // The engine keeps its fds at 1000 and above.
                fd.file_name()
                    .to_str()
                    .and_then(|name| name.parse::<u32>().ok())
                    .is_some_and(|n| n >= 1000)
                    && fs::read_link(fd.path()).is_ok_and(|target| target == dir)
            })
        })
        .collect()
}

/// tawcroot cannot bind `/proc/self/fd/N` sources the way PRoot does for
/// `/dev/std{in,out,err}`; Debian's own links through `/proc/self/fd`
/// resolve per process instead.
pub fn ensure_dev_links(rootfs: &Path) {
    for (name, target) in [
        ("stdin", "/proc/self/fd/0"),
        ("stdout", "/proc/self/fd/1"),
        ("stderr", "/proc/self/fd/2"),
        ("fd", "/proc/self/fd"),
    ] {
        let link = rootfs.join("dev").join(name);
        if fs::symlink_metadata(&link).is_err() {
            let _ = std::os::unix::fs::symlink(target, &link);
        }
    }
}

/// tawcroot has no `-w`: start the command from `working_dir` instead.
pub fn with_working_dir(command: &str, working_dir: &Path) -> String {
    let dir = working_dir.display().to_string().replace('\'', "'\\''");
    format!("cd '{dir}' 2>/dev/null || cd /; {command}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_dir_prefix_quotes_the_path() {
        assert_eq!(
            with_working_dir("echo hi", Path::new("/home/o'brien")),
            "cd '/home/o'\\''brien' 2>/dev/null || cd /; echo hi"
        );
    }
}
