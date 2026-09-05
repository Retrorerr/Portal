use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::os::unix::fs::PermissionsExt;
#[cfg(target_os = "android")]
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "android")]
use std::os::unix::process::CommandExt;

use crate::android::diagnostics;
use crate::android::utils::application_context::get_application_context;
use crate::core::config;
use crate::core::runtime::{LinuxRuntime, LogCallback, ProcessSpec, RuntimeHealth};
use winit::platform::android::activity::AndroidApp;

const MAX_CAPTURED_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_DIAGNOSTIC_LINE_BYTES: usize = 64 * 1024;
const SUPPORT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const SUPPORT_CHECK_BINARY: &str = "ld-linux-aarch64.so.1";

/// Concrete PRoot-based Linux runtime.
#[derive(Debug, Clone)]
pub struct PRootRuntime {
    rootfs: PathBuf,
}

impl PRootRuntime {
    pub fn new(rootfs: impl Into<PathBuf>) -> Self {
        Self {
            rootfs: rootfs.into(),
        }
    }

    pub fn production() -> Self {
        Self::new(config::PRODUCTION_FS_ROOT)
    }

    pub fn debian_slot(path: impl Into<PathBuf>) -> Self {
        Self::new(path)
    }

    pub fn active() -> Self {
        #[cfg(not(test))]
        let base = "/data/data/app.polarbear/files";
        #[cfg(test)]
        let base = "/data/local/tmp";
        let layout = crate::core::runtime::RuntimeLayout::new(base);
        Self::new(layout.active_slot().rootfs_path)
    }

    pub fn is_supported(android_app: &AndroidApp) -> bool {
        let context = get_application_context();
        let supported = if Self::ensure_support_probe_rootfs(android_app).is_some() {
            Self::try_proot_probe(
                &context.data_dir,
                &format!("/{}", SUPPORT_CHECK_BINARY),
                &["--help"],
            )
        } else {
            log::info!("Support probe asset missing or could not be extracted");
            false
        };

        if !supported {
            diagnostics::guest_event(
                "support-probe-failed",
                "PRoot loader probe did not complete successfully; device cannot run the ARM64 guest",
            );
            log::error!("⚡️ Device Unsupported");
        }
        supported
    }

    fn ensure_support_probe_rootfs(android_app: &AndroidApp) -> Option<()> {
        let context = get_application_context();
        let probe_exec = context.data_dir.join(SUPPORT_CHECK_BINARY);

        let asset_name = std::ffi::CString::new(SUPPORT_CHECK_BINARY).ok()?;
        let mut asset = android_app.asset_manager().open(&asset_name)?;

        let mut bytes = Vec::with_capacity(asset.length());
        asset.read_to_end(&mut bytes).ok()?;
        fs::write(&probe_exec, bytes).ok()?;
        fs::set_permissions(&probe_exec, fs::Permissions::from_mode(0o755)).ok()?;

        Some(())
    }

    fn try_proot_probe(rootfs: &Path, guest_program: &str, args: &[&str]) -> bool {
        let context = get_application_context();
        let proot_loader = context.native_library_dir.join("libproot_loader.so");

        let mut process = Command::new(context.native_library_dir.join("libproot.so"));
        process
            .env("PROOT_LOADER", &proot_loader)
            .env("PROOT_TMP_DIR", &context.data_dir);

        let mut child = match process
            .arg("-r")
            .arg(rootfs)
            .arg("-w")
            .arg("/")
            .arg(guest_program)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                log::info!(
                    "try_proot_probe rootfs={}, program={} error: {}",
                    rootfs.display(),
                    guest_program,
                    error
                );
                return false;
            }
        };

        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    return child.wait_with_output().map_or_else(
                        |error| {
                            log::info!("Failed to collect support probe output: {error}");
                            diagnostics::host_event(
                                "support-probe",
                                &format!("collect-output-error={error}"),
                            );
                            false
                        },
                        |output| {
                            diagnostics::guest_event(
                                "support-probe",
                                &format!(
                                    "program={} status={:?} stdout={} stderr={}",
                                    guest_program,
                                    output.status.code(),
                                    String::from_utf8_lossy(&output.stdout),
                                    String::from_utf8_lossy(&output.stderr)
                                ),
                            );
                            output.status.success()
                        },
                    );
                }
                Ok(None) if started.elapsed() < SUPPORT_PROBE_TIMEOUT => {
                    thread::sleep(Duration::from_millis(50));
                }
                Ok(None) => {
                    log::error!(
                        "PRoot support probe timed out after {:?}; terminating it",
                        SUPPORT_PROBE_TIMEOUT
                    );
                    let _ = child.kill();
                    let _ = child.wait();
                    diagnostics::host_event(
                        "support-probe",
                        &format!("timeout={SUPPORT_PROBE_TIMEOUT:?} program={guest_program}"),
                    );
                    return false;
                }
                Err(error) => {
                    log::error!("Failed to poll PRoot support probe: {error}");
                    let _ = child.kill();
                    let _ = child.wait();
                    diagnostics::host_event(
                        "support-probe",
                        &format!("poll-error={error} program={guest_program}"),
                    );
                    return false;
                }
            }
        }
    }
}

fn drain_stream<R>(
    reader: R,
    command: String,
    user: String,
    stream: &'static str,
    callback: Option<LogCallback>,
    stop: Arc<AtomicBool>,
) -> Vec<u8>
where
    R: Read + Send + 'static,
{
    let mut reader = BufReader::new(reader);
    let mut output = Vec::new();
    let mut line = Vec::new();

    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {
                if output.len() < MAX_CAPTURED_OUTPUT_BYTES {
                    let remaining = MAX_CAPTURED_OUTPUT_BYTES - output.len();
                    output.extend_from_slice(&line[..line.len().min(remaining)]);
                }

                let mut text = String::from_utf8_lossy(&line)
                    .trim_end_matches(['\r', '\n'])
                    .to_string();
                if text.len() > MAX_DIAGNOSTIC_LINE_BYTES {
                    let mut end = MAX_DIAGNOSTIC_LINE_BYTES;
                    while end > 0 && !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    text.truncate(end);
                    text.push_str("…<line truncated>");
                }
                diagnostics::guest_process_line(&command, &user, stream, &text);

                if stream == "stdout" {
                    if let Some(callback) = callback.as_ref() {
                        callback(text.clone());
                    }
                }

                if stream == "stderr" && !text.is_empty() {
                    log::debug!("guest-stderr: {text}");
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                let text = format!("stream-read-error={error}");
                diagnostics::guest_process_line(&command, &user, stream, &text);
                log::warn!("Failed to drain guest {stream}: {error}");
                break;
            }
        }
    }

    output
}

#[cfg(target_os = "android")]
fn set_nonblocking<R: AsRawFd>(reader: &R) -> std::io::Result<()> {
    let fd = reader.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn signal_process_group(process_group: Option<libc::pid_t>, signal: libc::c_int) -> bool {
    let Some(process_group) = process_group.filter(|group| *group > 1) else {
        return false;
    };
    unsafe { libc::kill(-process_group, signal) == 0 }
}

impl LinuxRuntime for PRootRuntime {
    fn engine_name(&self) -> &'static str {
        "proot"
    }

    fn rootfs_path(&self) -> &Path {
        &self.rootfs
    }

    fn check_health(&self) -> RuntimeHealth {
        if !self.rootfs.exists() {
            return RuntimeHealth::MissingRootfs(self.rootfs.clone());
        }
        let sh_bin = self.rootfs.join("bin/sh");
        let usr_sh_bin = self.rootfs.join("usr/bin/sh");
        if !sh_bin.exists() && !usr_sh_bin.exists() {
            return RuntimeHealth::Degraded("Rootfs missing /bin/sh shell".to_string());
        }
        RuntimeHealth::Healthy
    }

    fn execute(
        &self,
        spec: ProcessSpec,
        log: Option<LogCallback>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Output {
        let context = get_application_context();
        let user = spec.user.as_deref().unwrap_or("root");
        let command = spec.command;
        let user = user.to_string();

        let mut process = Command::new(context.native_library_dir.join("libproot.so"));
        process
            .env(
                "PROOT_LOADER",
                context.native_library_dir.join("libproot_loader.so"),
            )
            .env("PROOT_TMP_DIR", &context.data_dir);

        let rootfs_str = self.rootfs.to_string_lossy().to_string();

        process
            .arg("-r")
            .arg(&self.rootfs)
            .arg("-L")
            .arg("--link2symlink")
            .arg("--sysvipc")
            .arg("--kill-on-exit")
            .arg("--root-id")
            .arg("--bind=/dev")
            .arg("--bind=/proc")
            .arg("--bind=/sys")
            .arg(format!("--bind={}/tmp:/dev/shm", rootfs_str))
            .arg("--bind=/dev/tty:/dev/tty");

        if context.permission_all_files_access {
            process
                .arg("--bind=/sdcard:/android")
                .arg("--bind=/sdcard:/root/Android");
        }

        process
            .arg("--bind=/dev/urandom:/dev/random")
            .arg("--bind=/proc/self/fd:/dev/fd")
            .arg("--bind=/proc/self/fd/0:/dev/stdin")
            .arg("--bind=/proc/self/fd/1:/dev/stdout")
            .arg("--bind=/proc/self/fd/2:/dev/stderr")
            .arg(format!("--bind={}/proc/.loadavg:/proc/loadavg", rootfs_str))
            .arg(format!("--bind={}/proc/.stat:/proc/stat", rootfs_str))
            .arg(format!("--bind={}/proc/.uptime:/proc/uptime", rootfs_str))
            .arg(format!("--bind={}/proc/.version:/proc/version", rootfs_str))
            .arg(format!("--bind={}/proc/.vmstat:/proc/vmstat", rootfs_str))
            .arg(format!("--bind={}/proc/.sysctl_entry_cap_last_cap:/proc/sys/kernel/cap_last_cap", rootfs_str))
            .arg(format!("--bind={}/proc/.sysctl_inotify_max_user_watches:/proc/sys/fs/inotify/max_user_watches", rootfs_str))
            .arg(format!("--bind={}/sys/.empty:/sys/fs/selinux", rootfs_str));

        for bind in &spec.extra_binds {
            let bind_arg = if bind.readonly {
                format!("--bind={}:{}:ro", bind.host_path.display(), bind.guest_path.display())
            } else {
                format!("--bind={}:{}", bind.host_path.display(), bind.guest_path.display())
            };
            process.arg(bind_arg);
        }

        #[cfg(target_os = "android")]
        process.process_group(0);

        process.arg("/usr/bin/env").arg("-i");
        if user == "root" {
            process.arg("HOME=/root");
        } else {
            process.arg(format!("HOME=/home/{}", user));
        }

        process
            .arg("LANG=C.UTF-8")
            .arg("TERM=xterm-256color")
            .arg("PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/local/games:/usr/games:/system/bin:/system/xbin")
            .arg("TMPDIR=/tmp")
            .arg(format!("USER={}", user))
            .arg(format!("LOGNAME={}", user));

        for (k, v) in &spec.env {
            process.arg(format!("{}={}", k, v));
        }

        if let Some(wd) = &spec.working_dir {
            process.arg(format!("PWD={}", wd.display()));
        }

        if user == "root" {
            process.arg("sh");
        } else {
            process
                .arg("runuser")
                .arg("--pty")
                .arg("-u")
                .arg(&user)
                .arg("--")
                .arg("sh");
        }

        process.arg("-c").arg(&command);

        let mut child = process
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to run command");

        #[cfg(target_os = "android")]
        let process_group = {
            let child_pid = child.id() as libc::pid_t;
            let group = unsafe { libc::getpgid(child_pid) };
            (group == child_pid && group > 1).then_some(group)
        };

        let stdout = child.stdout.take().expect("stdout was not piped");
        let stderr = child.stderr.take().expect("stderr was not piped");
        #[cfg(target_os = "android")]
        {
            if let Err(error) = set_nonblocking(&stdout) {
                log::warn!("Failed to make guest stdout nonblocking: {error}");
            }
            if let Err(error) = set_nonblocking(&stderr) {
                log::warn!("Failed to make guest stderr nonblocking: {error}");
            }
        }
        let reader_stop = Arc::new(AtomicBool::new(false));
        let stdout_command = command.clone();
        let stdout_user = user.clone();
        let stdout_callback = log;
        let stdout_stop = reader_stop.clone();
        let stdout_thread = thread::spawn(move || {
            drain_stream(
                stdout,
                stdout_command,
                stdout_user,
                "stdout",
                stdout_callback,
                stdout_stop,
            )
        });

        let stderr_command = command.clone();
        let stderr_user = user.clone();
        let stderr_stop = reader_stop.clone();
        let stderr_thread = thread::spawn(move || {
            drain_stream(stderr, stderr_command, stderr_user, "stderr", None, stderr_stop)
        });

        let mut terminate_deadline = None;
        let status = loop {
            if cancel
                .as_ref()
                .is_some_and(|cancel| cancel.load(Ordering::Acquire))
                && terminate_deadline.is_none()
            {
                #[cfg(target_os = "android")]
                let signalled_group = signal_process_group(process_group, libc::SIGTERM);
                #[cfg(not(target_os = "android"))]
                let signalled_group = false;
                if !signalled_group {
                    let _ = child.kill();
                }
                reader_stop.store(true, Ordering::Release);
                terminate_deadline = Some(Instant::now() + Duration::from_secs(2));
                diagnostics::host_event(
                    "guest-cancel",
                    &format!(
                        "command={} pid={} process_group={:?} signal=TERM",
                        command,
                        child.id(),
                        process_group,
                    ),
                );
            }

            match child.try_wait() {
                Ok(Some(status)) => {
                    if terminate_deadline.is_some() {
                        #[cfg(target_os = "android")]
                        let signalled_group =
                            signal_process_group(process_group, libc::SIGKILL);
                        #[cfg(not(target_os = "android"))]
                        let signalled_group = false;
                        if !signalled_group {
                            let _ = child.kill();
                        }
                        diagnostics::host_event(
                            "guest-cancel",
                            &format!(
                                "command={} pid={} process_group={:?} signal=KILL-after-exit",
                                command,
                                child.id(),
                                process_group,
                            ),
                        );
                    }
                    reader_stop.store(true, Ordering::Release);
                    break status;
                }
                Ok(None) => {
                    if terminate_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                        #[cfg(target_os = "android")]
                        let signalled_group =
                            signal_process_group(process_group, libc::SIGKILL);
                        #[cfg(not(target_os = "android"))]
                        let signalled_group = false;
                        if !signalled_group {
                            let _ = child.kill();
                        }
                        diagnostics::host_event(
                            "guest-cancel",
                            &format!(
                                "command={} pid={} process_group={:?} signal=KILL",
                                command,
                                child.id(),
                                process_group,
                            ),
                        );
                        terminate_deadline = None;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                Err(error) => {
                    log::error!("Failed to poll guest command {command}: {error}");
                    reader_stop.store(true, Ordering::Release);
                    let _ = child.kill();
                    break child.wait().expect("Failed to reap guest command");
                }
            }
        };
        reader_stop.store(true, Ordering::Release);
        let stdout = stdout_thread.join().unwrap_or_default();
        let stderr = stderr_thread.join().unwrap_or_default();

        Output {
            status,
            stdout,
            stderr,
        }
    }

    fn terminate(&self) {
        log::info!("Terminating PRoot runtime at {}", self.rootfs.display());
    }
}
