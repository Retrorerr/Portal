//! Debian package updates for an installed runtime.
//!
//! Policy and parsing live in `crate::core::system_updates`. This module
//! keeps Portal's apt pin in place, checks for updates in the background
//! once the desktop is up, and runs an upgrade the user starts from the
//! Return-to-Plasma screen.
//!
//! An upgrade stops Plasma first, so no running process has its libraries
//! replaced underneath it. The event loop then starts a fresh session
//! whether the upgrade succeeded or not: apt leaves a usable system either
//! way, and `dpkg --configure -a` finishes any interrupted work next time.

use crate::{
    android::{
        diagnostics,
        runtime::proot::PRootRuntime,
        utils::{application_context::get_application_context, compose_overlay, webview_handoff},
    },
    core::{
        config::PRODUCTION_FS_ROOT,
        runtime::{LinuxRuntime, ProcessSpec},
        system_updates::{
            apt_pin_content, changed_protected_packages, check_due, friendly_status,
            installed_versions, overall_progress, parse_apt_status, parse_upgrade_simulation,
            sort_for_display, still_pending, AptStatus, PendingUpdate, UpdateCache,
            APT_PIN_REL, CACHE_REL, INTERRUPTED_REL, LOG_GUEST_PATH,
        },
    },
};
use anyhow::Context;
use std::{
    collections::HashMap,
    fs,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use winit::platform::android::activity::AndroidApp;

/// Wait after the first desktop frame before checking, so startup work
/// (Plasma, Baloo, the first app launch) has the device to itself.
const CHECK_DELAY: Duration = Duration::from_secs(90);
const PUBLISH_INTERVAL: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateStatus {
    None,
    Available,
    Running,
    Failed,
    Complete,
}

impl UpdateStatus {
    pub const fn code(self) -> i32 {
        match self {
            Self::None => 0,
            Self::Available => 1,
            Self::Running => 2,
            Self::Failed => 3,
            Self::Complete => 4,
        }
    }
}

/// Completion edge consumed by the event loop, which restarts Plasma.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateResult {
    Installed,
    Failed(String),
}

/// What the Return screen shows.
#[derive(Clone, Debug)]
pub struct UpdateSnapshot {
    pub status: UpdateStatus,
    pub progress: u16,
    pub message: String,
    pub updates: Vec<PendingUpdate>,
    /// A previous upgrade stopped before it finished.
    pub interrupted: bool,
}

struct Coordinator {
    loaded: bool,
    snapshot: UpdateSnapshot,
    result: Option<UpdateResult>,
}

fn coordinator() -> &'static Mutex<Coordinator> {
    static COORDINATOR: OnceLock<Mutex<Coordinator>> = OnceLock::new();
    COORDINATOR.get_or_init(|| {
        Mutex::new(Coordinator {
            loaded: false,
            snapshot: UpdateSnapshot {
                status: UpdateStatus::None,
                progress: 0,
                message: String::new(),
                updates: Vec::new(),
                interrupted: false,
            },
            result: None,
        })
    })
}

fn root() -> &'static Path {
    Path::new(PRODUCTION_FS_ROOT)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes)?;
    fs::File::open(&temporary)?.sync_all()?;
    fs::rename(&temporary, path)?;
    Ok(())
}

/// Keep Portal's platform packages pinned away from Debian's archives.
/// Runs on every launch and before every apt run Portal starts, so a
/// user who deletes or edits the file gets it back.
pub fn sync_apt_policy(fs_root: &Path) -> anyhow::Result<()> {
    let path = fs_root.join(APT_PIN_REL);
    let content = apt_pin_content();
    if fs::read_to_string(&path).ok().as_deref() == Some(content.as_str()) {
        return Ok(());
    }
    write_atomic(&path, content.as_bytes())
        .with_context(|| format!("could not write {}", path.display()))
}

fn read_installed(fs_root: &Path) -> anyhow::Result<HashMap<String, String>> {
    let status = fs::read_to_string(fs_root.join("var/lib/dpkg/status"))
        .context("could not read the Debian package status")?;
    Ok(installed_versions(&status))
}

fn read_cache(fs_root: &Path) -> Option<UpdateCache> {
    UpdateCache::parse(&fs::read_to_string(fs_root.join(CACHE_REL)).ok()?)
}

fn idle_status(updates: &[PendingUpdate], interrupted: bool) -> UpdateStatus {
    if interrupted || !updates.is_empty() {
        UpdateStatus::Available
    } else {
        UpdateStatus::None
    }
}

/// Load the last check's result once per process, dropping anything the
/// user has since installed another way.
fn ensure_loaded(coordinator: &mut Coordinator) {
    if coordinator.loaded {
        return;
    }
    coordinator.loaded = true;
    let fs_root = root();
    let interrupted = fs_root.join(INTERRUPTED_REL).is_file();
    let mut updates = match (read_cache(fs_root), read_installed(fs_root)) {
        (Some(cache), Ok(installed)) => still_pending(&cache.updates, &installed),
        _ => Vec::new(),
    };
    sort_for_display(&mut updates);
    coordinator.snapshot.status = idle_status(&updates, interrupted);
    coordinator.snapshot.updates = updates;
    coordinator.snapshot.interrupted = interrupted;
}

pub fn ui_snapshot() -> Option<UpdateSnapshot> {
    let mut coordinator = coordinator().lock().ok()?;
    ensure_loaded(&mut coordinator);
    Some(coordinator.snapshot.clone())
}

/// Rehydrate the Return screen after the overlay is created or recreated.
pub fn publish_current(android_app: &AndroidApp) {
    if let Some(snapshot) = ui_snapshot() {
        compose_overlay::publish_system_update_state(android_app, &snapshot);
    }
}

fn publish() {
    publish_current(&get_application_context().android_app);
}

fn update_snapshot(change: impl FnOnce(&mut UpdateSnapshot)) {
    if let Ok(mut coordinator) = coordinator().lock() {
        change(&mut coordinator.snapshot);
    }
    publish();
}

fn log_tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let tail: Vec<&str> = text.lines().rev().take(6).collect();
    tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
}

/// List what `apt-get upgrade --with-new-pkgs` would install from the
/// current package lists. Simulation needs no lock.
fn simulate_upgrade() -> anyhow::Result<Vec<PendingUpdate>> {
    let output = PRootRuntime::active().execute(
        ProcessSpec::new(
            "nice -n 10 apt-get -s -o Debug::NoLocking=true upgrade --with-new-pkgs",
        )
        .with_env("DEBIAN_FRONTEND", "noninteractive"),
        None,
        None,
    );
    anyhow::ensure!(
        output.status.success(),
        "upgrade simulation failed: {}",
        log_tail(&output.stderr)
    );
    Ok(parse_upgrade_simulation(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn write_cache(updates: &[PendingUpdate]) -> anyhow::Result<()> {
    let cache = UpdateCache {
        checked_at: now_secs(),
        updates: updates.to_vec(),
    };
    write_atomic(&root().join(CACHE_REL), cache.to_json().as_bytes())
}

fn run_check() -> anyhow::Result<usize> {
    let fs_root = root();
    sync_apt_policy(fs_root)?;
    let output = PRootRuntime::active().execute(
        ProcessSpec::new(format!(
            "nice -n 10 apt-get -q update >>{LOG_GUEST_PATH} 2>&1"
        ))
        .with_env("DEBIAN_FRONTEND", "noninteractive"),
        None,
        None,
    );
    anyhow::ensure!(
        output.status.success(),
        "apt-get update failed (status {:?}); see {LOG_GUEST_PATH}",
        output.status.code()
    );
    let mut updates = simulate_upgrade()?;
    write_cache(&updates)?;
    sort_for_display(&mut updates);
    let count = updates.len();
    if let Ok(mut coordinator) = coordinator().lock() {
        ensure_loaded(&mut coordinator);
        if coordinator.snapshot.status != UpdateStatus::Running {
            let interrupted = coordinator.snapshot.interrupted;
            coordinator.snapshot.status = idle_status(&updates, interrupted);
            coordinator.snapshot.updates = updates;
        }
    }
    Ok(count)
}

/// Check for updates once per process, a while after the desktop first
/// appears. The result is shown on the next Return-to-Plasma screen.
/// Skipped on metered networks and when the last check is recent.
pub fn schedule_background_check(android_app: &AndroidApp) {
    static SCHEDULED: AtomicBool = AtomicBool::new(false);
    if SCHEDULED.swap(true, Ordering::AcqRel) {
        return;
    }
    let android_app = android_app.clone();
    thread::spawn(move || {
        thread::sleep(CHECK_DELAY);
        if !crate::core::provisioning::RuntimeArtifact::production().is_bootable(root()) {
            return;
        }
        if !check_due(read_cache(root()).as_ref(), now_secs()) {
            return;
        }
        if crate::android::utils::ndk::is_active_network_metered(&android_app) {
            log::info!("system-updates: skipping the check on a metered network");
            return;
        }
        match run_check() {
            Ok(count) => {
                log::info!("system-updates: {count} Debian updates available");
                diagnostics::host_event("system-updates-check", &format!("available={count}"));
            }
            Err(error) => {
                log::warn!("system-updates: check failed: {error:#}");
                diagnostics::host_event("system-updates-check-failed", &format!("{error:#}"));
            }
        }
    });
}

/// Start an upgrade the user asked for. Returns false when one cannot
/// start; while one runs, further calls attach to it.
pub fn begin_update() -> bool {
    let fs_root = root();
    if !crate::core::provisioning::RuntimeArtifact::production().is_bootable(fs_root) {
        log::warn!("Ignoring system update: the runtime is not a completed Portal install");
        return false;
    }
    let (repair, _) = crate::android::proot::setup::anland_repair_ui_snapshot();
    if repair == crate::android::proot::setup::AnlandRepairAvailability::Running {
        log::warn!("Ignoring system update while the Anland graphics repair runs");
        return false;
    }
    {
        let Ok(mut coordinator) = coordinator().lock() else {
            return false;
        };
        ensure_loaded(&mut coordinator);
        match coordinator.snapshot.status {
            UpdateStatus::Running => return true,
            UpdateStatus::Available | UpdateStatus::Failed => {}
            UpdateStatus::None | UpdateStatus::Complete => return false,
        }
        if coordinator.result.is_some() {
            return false;
        }
        coordinator.snapshot.status = UpdateStatus::Running;
        coordinator.snapshot.progress = 1;
        coordinator.snapshot.message = "Closing Plasma…".to_owned();
    }
    publish();
    thread::spawn(|| {
        let outcome = std::panic::catch_unwind(run_update)
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the update worker panicked")));
        finish_update(outcome);
    });
    true
}

fn run_update() -> anyhow::Result<()> {
    let fs_root = root();
    let android_app = get_application_context().android_app;
    compose_overlay::notify_desktop_suspended(&android_app);
    crate::android::proot::launch::stop();
    // `stop` bounds its own wait; the session must be gone before its
    // libraries are replaced.
    let deadline = Instant::now() + Duration::from_secs(15);
    while crate::android::proot::launch::is_running() {
        anyhow::ensure!(
            Instant::now() < deadline,
            "Plasma did not stop, so nothing was changed"
        );
        thread::sleep(Duration::from_millis(50));
    }

    sync_apt_policy(fs_root)?;
    let before = read_installed(fs_root)?;
    write_atomic(&fs_root.join(INTERRUPTED_REL), b"1\n")?;
    update_snapshot(|snapshot| {
        snapshot.progress = 3;
        snapshot.message = "Refreshing package lists…".to_owned();
    });

    // apt's machine-readable progress goes to fd 3, which is the pipe the
    // host reads; everything else goes to the log. confdef/confold keep the
    // user's edited configuration files without prompting.
    let command = format!(
        "exec 3>&1 >>{LOG_GUEST_PATH} 2>&1; \
         echo \"== Portal system update $(date -Is) ==\"; \
         (test -x /usr/bin/awk || \
          update-alternatives --quiet --install /usr/bin/awk awk /usr/bin/gawk 10) && \
         dpkg --configure -a && \
         apt-get -q update && \
         apt-get -q -y -o APT::Status-Fd=3 \
             -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold \
             upgrade --with-new-pkgs"
    );
    let last_error = Arc::new(Mutex::new(None::<String>));
    let callback_error = last_error.clone();
    let last_publish = Mutex::new(Instant::now() - PUBLISH_INTERVAL);
    let output = PRootRuntime::active().execute(
        ProcessSpec::new(command)
            .with_env("DEBIAN_FRONTEND", "noninteractive")
            .with_env("APT_LISTCHANGES_FRONTEND", "none"),
        Some(Arc::new(move |line: String| {
            let Some(status) = parse_apt_status(&line) else {
                return;
            };
            if let AptStatus::Error(message) = &status {
                if let Ok(mut error) = callback_error.lock() {
                    *error = Some(message.clone());
                }
                return;
            }
            let Ok(mut last) = last_publish.lock() else {
                return;
            };
            if last.elapsed() < PUBLISH_INTERVAL {
                return;
            }
            *last = Instant::now();
            let progress = overall_progress(&status);
            let message = match &status {
                AptStatus::Download(_) => "Downloading updates…".to_owned(),
                AptStatus::Install(_, message) => friendly_status(message),
                AptStatus::Error(_) => return,
            };
            update_snapshot(|snapshot| {
                if let Some(progress) = progress {
                    snapshot.progress = snapshot.progress.max(progress);
                }
                snapshot.message = message;
            });
        })),
        None,
    );
    if !output.status.success() {
        let detail = last_error
            .lock()
            .ok()
            .and_then(|error| error.clone())
            .unwrap_or_else(|| format!("apt exited with status {:?}", output.status.code()));
        anyhow::bail!("{detail}; see {LOG_GUEST_PATH}");
    }

    update_snapshot(|snapshot| {
        snapshot.progress = 97;
        snapshot.message = "Checking Portal's graphics stack…".to_owned();
    });
    let after = read_installed(fs_root)?;
    let changed = changed_protected_packages(&before, &after);
    anyhow::ensure!(
        changed.is_empty(),
        "Portal's pinned packages changed during the update: {}",
        changed.join(", ")
    );
    anyhow::ensure!(
        fs_root.join("usr/bin/kwin_wayland").is_file(),
        "kwin_wayland is missing after the update"
    );
    let _ = fs::remove_file(fs_root.join(INTERRUPTED_REL));
    // Whatever apt kept back stays listed; it is not an error.
    match simulate_upgrade() {
        Ok(remaining) => {
            if let Err(error) = write_cache(&remaining) {
                log::warn!("system-updates: could not record the remaining updates: {error:#}");
            }
        }
        Err(error) => log::warn!("system-updates: post-update check failed: {error:#}"),
    }
    Ok(())
}

fn finish_update(outcome: anyhow::Result<()>) {
    let result = match outcome {
        Ok(()) => {
            diagnostics::host_event("system-update-complete", "");
            update_snapshot(|snapshot| {
                snapshot.status = UpdateStatus::Complete;
                snapshot.progress = 100;
                snapshot.message = "Restarting Plasma…".to_owned();
                snapshot.updates.clear();
                snapshot.interrupted = false;
            });
            UpdateResult::Installed
        }
        Err(error) => {
            let detail = format!("{error:#}");
            log::error!("system-updates: update failed: {detail}");
            diagnostics::host_event("system-update-failed", &detail);
            update_snapshot(|snapshot| {
                snapshot.status = UpdateStatus::Failed;
                snapshot.message = detail.clone();
            });
            UpdateResult::Failed(detail)
        }
    };
    if let Ok(mut coordinator) = coordinator().lock() {
        coordinator.result = Some(result);
    }
    webview_handoff::wake_event_loop();
}

/// While an upgrade runs, no Plasma session may start: its libraries are
/// being replaced. The update result handler starts the next session.
pub fn update_in_progress() -> bool {
    coordinator()
        .lock()
        .map(|coordinator| coordinator.snapshot.status == UpdateStatus::Running)
        .unwrap_or(false)
}

/// Consume the completion edge. The event loop restarts Plasma on either
/// outcome, because the worker stopped it.
pub fn take_update_result() -> Option<UpdateResult> {
    coordinator()
        .lock()
        .ok()
        .and_then(|mut coordinator| coordinator.result.take())
}
