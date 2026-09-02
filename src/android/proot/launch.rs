use super::process::ArchProcess;
use crate::android::{
    diagnostics,
    utils::application_context::get_application_context,
    utils::webview_handoff,
};
use crate::core::config::ARCH_FS_ROOT;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use std::thread::JoinHandle;

static LAUNCH_RUNNING: AtomicBool = AtomicBool::new(false);

struct LaunchState {
    cancel: Option<Arc<AtomicBool>>,
    handle: Option<JoinHandle<()>>,
    failure_monitor: Option<JoinHandle<()>>,
}

impl Default for LaunchState {
    fn default() -> Self {
        Self {
            cancel: None,
            handle: None,
            failure_monitor: None,
        }
    }
}

static LAUNCH_STATE: OnceLock<Mutex<LaunchState>> = OnceLock::new();

fn launch_state() -> &'static Mutex<LaunchState> {
    LAUNCH_STATE.get_or_init(|| Mutex::new(LaunchState::default()))
}

static LAUNCH_FAILURE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn launch_failure() -> &'static Mutex<Option<String>> {
    LAUNCH_FAILURE.get_or_init(|| Mutex::new(None))
}

fn report_failure(reason: impl Into<String>) {
    let reason = reason.into();
    let should_wake = if let Ok(mut failure) = launch_failure().lock() {
        if failure.is_none() {
            *failure = Some(reason.clone());
            true
        } else {
            false
        }
    } else {
        false
    };
    if should_wake {
        diagnostics::host_event("desktop-failure", &reason);
        webview_handoff::wake_event_loop();
    }
}

/// Consume a failure reported by the guest-session monitor.
pub fn take_failure() -> Option<String> {
    launch_failure().lock().ok().and_then(|mut failure| failure.take())
}

fn clear_failure_markers() {
    let state_dir = Path::new(ARCH_FS_ROOT).join("var/lib/localdesktop");
    for marker in ["plasma-failed", "kwin-crash", "plasma-ready"] {
        let _ = fs::remove_file(state_dir.join(marker));
    }
}

fn marker_reason(path: &Path, name: &str) -> String {
    let details = fs::read_to_string(path)
        .ok()
        .map(|text| {
            text.lines()
                .find(|line| line.starts_with("reason="))
                .unwrap_or(text.lines().next().unwrap_or_default())
                .trim()
                .chars()
                .take(512)
                .collect::<String>()
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| format!("marker={name}"));
    format!("Plasma startup failed ({name}): {details}")
}

/// Watch the guest's durable crash/failure markers independently of Wayland redraws. The monitor
/// wakes the waiting winit loop as soon as a KWin startup attempt enters graphical recovery, so a
/// blank/suspended surface cannot hide the actionable Android error page.
fn spawn_failure_monitor(cancel: Arc<AtomicBool>) -> JoinHandle<()> {
    thread::spawn(move || {
        let state_dir = Path::new(ARCH_FS_ROOT).join("var/lib/localdesktop");
        while !cancel.load(Ordering::Acquire) {
            for name in ["plasma-failed", "kwin-crash"] {
                let path = state_dir.join(name);
                if path.is_file() {
                    report_failure(marker_reason(&path, name));
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
    })
}

struct LaunchRunningGuard;

impl Drop for LaunchRunningGuard {
    fn drop(&mut self) {
        LAUNCH_RUNNING.store(false, Ordering::Release);
        // A bounded stop may detach the finished worker from the event-loop thread. Wake the
        // lifecycle owner so a pending Retry Plasma action can continue as soon as the guard is
        // released, without polling or an arbitrary sleep on the UI thread.
        webview_handoff::wake_event_loop();
    }
}

/// Whether the tracked guest-session worker is still alive.  Retry Plasma uses this as a barrier
/// after a bounded stop: if the worker outlives the event-loop turn, the guard's wake event retries
/// the backend rebuild only after its process group has been reaped.
pub fn is_running() -> bool {
    LAUNCH_RUNNING.load(Ordering::Acquire)
}

pub fn launch() {
    if LAUNCH_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        log::info!("Skipping launch because the desktop session is already running");
        return;
    }

    clear_failure_markers();
    if let Ok(mut failure) = launch_failure().lock() {
        *failure = None;
    }

    let cancel = Arc::new(AtomicBool::new(false));
    let Ok(mut state) = launch_state().lock() else {
        log::error!("Launch registry lock is poisoned; refusing to start an untracked guest session");
        LAUNCH_RUNNING.store(false, Ordering::Release);
        return;
    };

    // Publish the cancellation token before spawning either worker and keep the registry lock
    // through both spawns. A Retry Plasma request cannot observe LAUNCH_RUNNING=true with an empty
    // registry and accidentally miss the cancellation window.
    state.cancel = Some(cancel.clone());
    state.handle = None;
    state.failure_monitor = None;

    let thread_cancel = cancel.clone();
    let handle = thread::spawn(move || {
        let _guard = LaunchRunningGuard;
        diagnostics::host_event("desktop-launch", "starting configured Plasma session");

        // Clean up potential leftover files for display :1
        ArchProcess {
            command: "rm -f /tmp/.X1-lock".into(),
            user: None,
            log: None,
        }
        .run_with_cancel(thread_cancel.clone());
        ArchProcess {
            command: "rm -f /tmp/.X11-unix/X1".into(),
            user: None,
            log: None,
        }
        .run_with_cancel(thread_cancel.clone());

        let local_config = get_application_context().local_config;
        let username = local_config.user.username;

        let started = Instant::now();
        let output = ArchProcess {
            command: local_config.command.launch,
            user: Some(username),
            log: Some(Arc::new(|it| log::info!("guest-session: {}", it))),
        }
        .run_with_cancel(thread_cancel.clone());
        let status = output.status.code();
        diagnostics::desktop_exit(status, started.elapsed().as_millis());
        log::warn!(
            "Desktop session exited after {:?} with status {:?}",
            started.elapsed(),
            status
        );
        if !output.status.success() {
            report_failure(format!("Desktop session exited with status {status:?}"));
        }
        thread_cancel.store(true, Ordering::Release);
    });

    let failure_monitor = spawn_failure_monitor(cancel.clone());
    state.handle = Some(handle);
    state.failure_monitor = Some(failure_monitor);
}

/// Cancel and reap the currently tracked Plasma/PRoot session.
///
/// `ArchProcess::run_with_cancel` signals the private PRoot process group and escalates after a
/// bounded grace period. Joining the launch worker here ensures `LAUNCH_RUNNING` is cleared before
/// a Retry Plasma request can start a replacement session.
pub fn stop() {
    let (handle, failure_monitor) = if let Ok(mut state) = launch_state().lock() {
        if let Some(cancel) = state.cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        (state.handle.take(), state.failure_monitor.take())
    } else {
        log::error!("Launch registry lock is poisoned; cannot join the guest session");
        (None, None)
    };

    // Never block the Android event-loop indefinitely on a guest process. The process worker has
    // its own bounded TERM->KILL path; polling is an additional guard for a broken child or a
    // reader implementation that outlives that path. Dropping an unfinished JoinHandle detaches
    // it safely; LaunchRunningGuard keeps retries barred until the worker really exits.
    join_bounded(handle, "guest launch worker");
    join_bounded(failure_monitor, "guest failure monitor");
}

const STOP_JOIN_BUDGET: Duration = Duration::from_millis(500);

fn join_bounded(handle: Option<JoinHandle<()>>, label: &str) {
    let Some(handle) = handle else { return };
    let deadline = Instant::now() + STOP_JOIN_BUDGET;
    while !handle.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if !handle.is_finished() {
        log::warn!(
            "{label} did not stop within {:?}; detached worker remains cancellation-scoped",
            STOP_JOIN_BUDGET
        );
        return;
    }
    if let Err(error) = handle.join() {
        log::error!("{label} panicked while being reaped: {error:?}");
    }
}
