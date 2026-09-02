use crate::android::utils::application_context::get_application_context;
use crate::core::config;
use crate::android::diagnostics;
use std::ffi::CString;
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Read};
#[cfg(target_os = "android")]
use std::os::fd::AsRawFd;
#[cfg(target_os = "android")]
use std::os::unix::process::CommandExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::thread;
use std::time::{Duration, Instant};
use winit::platform::android::activity::AndroidApp;

pub type Log = Arc<dyn Fn(String) + Send + Sync>;

const SUPPORT_CHECK_BINARY: &str = "ld-linux-aarch64.so.1";
const SUPPORT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
// Keep `Output` bounded even when a guest command enables a verbose protocol
// trace. The complete line stream remains in the rotating diagnostics files.
const MAX_CAPTURED_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_DIAGNOSTIC_LINE_BYTES: usize = 64 * 1024;

/// Runs a shell command inside the Arch Linux PRoot environment.
///
/// - `command`: The shell command to execute (passed to `sh -c`).
/// - `user`: The user to run as. Defaults to `"root"` when `None`.
/// - `log`: Optional stdout line callback. When set, stdout is streamed line-by-line
///   to the callback. When `None`, stdout/stderr are captured.
pub struct ArchProcess {
    pub command: String,
    pub user: Option<String>,
    pub log: Option<Log>,
}

/// Drain one child stream without allowing a verbose guest process to block
/// its sibling stream.  PRoot and Plasma can write a large amount of stderr
/// while stdout is quiet, so both pipes must always be consumed concurrently.
fn drain_stream<R>(
    reader: R,
    command: String,
    user: String,
    stream: &'static str,
    callback: Option<Log>,
    stop: Arc<AtomicBool>,
) -> Vec<u8>
where
    R: Read + Send + 'static,
{
    let mut reader = BufReader::new(reader);
    let mut output = Vec::new();
    let mut line = Vec::new();

    loop {
        // A descendant can inherit stdout/stderr after the direct PRoot child exits.  Stop
        // waiting for those descriptors once the lifecycle owner has cancelled or reaped the
        // session; the reader output is intentionally bounded in that case.
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

                // Keep the original Output bytes intact while giving the
                // diagnostics mirror a stable, newline-free record.  Lossy
                // decoding is intentional: a broken guest locale must not
                // terminate the reader and leave the other pipe undrained.
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

                // stderr used to be inherited directly by the Android
                // process. Keep it visible in debug logcat while the complete
                // line is now persisted by diagnostics::guest_process_line.
                if stream == "stderr" && !text.is_empty() {
                    log::debug!("guest-stderr: {text}");
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                // Pipes are nonblocking on Android, so cancellation can break a reader even if a
                // TERM-ignoring grandchild still owns the write end.
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

/// Put a child pipe in nonblocking mode so a cancelled session cannot leave a reader thread
/// permanently stuck behind a descendant that inherited the pipe.
#[cfg(target_os = "android")]
fn set_nonblocking<R: AsRawFd>(reader: &R) -> std::io::Result<()> {
    let fd = reader.as_raw_fd();
    // SAFETY: `fd` is borrowed from a live ChildStdout/ChildStderr and remains open for this
    // call. `fcntl` only changes the descriptor flags.
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
    // SAFETY: `process_group` is accepted only when it matched the freshly spawned child's pid,
    // which is the private group created by CommandExt::process_group(0). A negative pid targets
    // only that group, never the rest of the Android process tree.
    unsafe { libc::kill(-process_group, signal) == 0 }
}

impl ArchProcess {
    fn ensure_support_probe_rootfs(android_app: &AndroidApp) -> Option<()> {
        let context = get_application_context();
        let probe_exec = context.data_dir.join(SUPPORT_CHECK_BINARY);

        let asset_name = CString::new(SUPPORT_CHECK_BINARY).ok()?;
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
                            log::info!(
                                "try_proot_probe rootfs={}, program={}, status={:?}, stdout: {}, stderr: {}",
                                rootfs.display(),
                                guest_program,
                                output.status.code(),
                                String::from_utf8_lossy(&output.stdout),
                                String::from_utf8_lossy(&output.stderr)
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

    /// Run a guest process and return its complete bounded output.
    pub fn run(self) -> Output {
        self.run_inner(None)
    }

    /// Run a guest process that can be cancelled by its lifecycle owner.
    ///
    /// The Android PRoot process is placed in its own process group. Cancellation signals that
    /// group first (covering the shell and all descendants), then escalates to SIGKILL after a
    /// bounded grace period and reaps the direct child before returning. The normal `run` path
    /// remains unchanged for one-shot setup commands.
    pub fn run_with_cancel(self, cancel: Arc<AtomicBool>) -> Output {
        self.run_inner(Some(cancel))
    }

    fn run_inner(self, cancel: Option<Arc<AtomicBool>>) -> Output {
        let context = get_application_context();
        let user = self.user.as_deref().unwrap_or("root");
        let command = self.command;
        let user = user.to_string();

        let mut process = Command::new(context.native_library_dir.join("libproot.so"));
        process
            .env(
                "PROOT_LOADER",
                context.native_library_dir.join("libproot_loader.so"),
            )
            .env("PROOT_TMP_DIR", context.data_dir);

        process
            .arg("-r")
            .arg(config::ARCH_FS_ROOT)
            .arg("-L")
            .arg("--link2symlink")
            .arg("--sysvipc")
            .arg("--kill-on-exit")
            .arg("--root-id")
            .arg("--bind=/dev")
            .arg("--bind=/proc")
            .arg("--bind=/sys")
            .arg(format!("--bind={}/tmp:/dev/shm", config::ARCH_FS_ROOT))
            // /dev/pts and /dev/ptmx are already covered by --bind=/dev above.
            // The explicit sub-binds added in commit 61d9079 were redundant and caused
            // proot to double-translate PTY ioctls (TIOCGPTN/TIOCSPTLCK/TIOCSWINSZ),
            // breaking terminal initialisation and keyboard arrow keys inside QTerminal.
            // We only need /dev/tty explicitly for processes that open it by path.
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
            .arg(format!("--bind={}/proc/.loadavg:/proc/loadavg", config::ARCH_FS_ROOT))
            .arg(format!("--bind={}/proc/.stat:/proc/stat", config::ARCH_FS_ROOT))
            .arg(format!("--bind={}/proc/.uptime:/proc/uptime", config::ARCH_FS_ROOT))
            .arg(format!("--bind={}/proc/.version:/proc/version", config::ARCH_FS_ROOT))
            .arg(format!("--bind={}/proc/.vmstat:/proc/vmstat", config::ARCH_FS_ROOT))
            .arg(format!("--bind={}/proc/.sysctl_entry_cap_last_cap:/proc/sys/kernel/cap_last_cap", config::ARCH_FS_ROOT))
            .arg(format!("--bind={}/proc/.sysctl_inotify_max_user_watches:/proc/sys/fs/inotify/max_user_watches", config::ARCH_FS_ROOT))
            .arg(format!("--bind={}/sys/.empty:/sys/fs/selinux", config::ARCH_FS_ROOT));

        // Give the PRoot shim and all of its guest descendants a private process group so a
        // runtime handoff can cancel exactly this session without touching unrelated processes.
        #[cfg(target_os = "android")]
        process.process_group(0);

        // env vars
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

        // user shell
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

        // Always pipe both streams.  `Child::wait_with_output` drains both
        // streams internally, but it cannot feed our diagnostics mirror (and
        // the old stdout-only reader could deadlock on a full stderr pipe).
        let mut child = process
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to run command");

        #[cfg(target_os = "android")]
        let process_group = {
            // `process_group(0)` makes the child pid the private group id. Require that exact
            // relationship before ever issuing a group signal; if setpgid was rejected, the
            // cancellation path falls back to the direct Child handle only.
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
        let stdout_callback = self.log;
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
                // Give the private process group a chance to flush and exit cleanly. Do not call
                // Child::kill here: on Android/Unix it is SIGKILL, which would terminate PRoot
                // before its --kill-on-exit cleanup can reap guest descendants.
                #[cfg(target_os = "android")]
                let signalled_group = signal_process_group(process_group, libc::SIGTERM);
                #[cfg(not(target_os = "android"))]
                let signalled_group = false;
                if !signalled_group {
                    // A kernel that rejected setpgid leaves us with only the direct-child handle.
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
                    // PRoot can exit after forwarding TERM while a TERM-ignoring descendant still
                    // owns the group and inherited pipes. One final group KILL is scoped to the
                    // captured private group and closes that window before returning.
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
}
