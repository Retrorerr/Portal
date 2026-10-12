//! Portal's updates for an installed runtime: the app itself, the lfdevs
//! Mesa layer KWin renders with, Debian packages (including Firefox and
//! Claude from their vendors' apt repositories) and the ChatGPT .deb.
//!
//! Policy and parsing live in `crate::core::system_updates` (Debian) and
//! `crate::core::update_sources` (everything else). This module keeps
//! Portal's apt pin in place, checks every source in the background while
//! Portal runs, and installs what the user accepts on the Return-to-Plasma
//! screen.
//!
//! What may update is decided by the maintainer's update policy (fetched
//! from the repository before every check): it can hold Debian packages,
//! and the graphics driver only moves to builds it lists as vetted. On top
//! of that every change is guarded:
//!
//! * The app installs only a signed release Android accepts as an update.
//! * A new graphics driver is on trial with the old one parked, and comes
//!   back out by itself if Plasma cannot draw a desktop on it.
//! * Debian upgrades never remove anything, never move a protected or held
//!   package (checked on a simulation first), and must leave KWin, Xwayland
//!   and Plasma resolving every library they link.
//!
//! An update stops Plasma first, so no running process has its libraries
//! replaced underneath it. The event loop then starts a fresh session
//! whether it succeeded or not: each step leaves a usable system, and
//! `dpkg --configure -a` finishes interrupted Debian work next time. A Portal
//! update runs last, because Android ends the process to replace the app.

use crate::{
    android::{
        app_update::{self, InstallOutcome},
        diagnostics,
        proot::mesa_layer,
        runtime::proot::PRootRuntime,
        update_net,
        utils::{application_context::get_application_context, compose_overlay, webview_handoff},
    },
    core::{
        config::{APP_FILES_ROOT, PRODUCTION_FS_ROOT, VERSION},
        install_plan::OptionalApp,
        runtime::{LinuxRuntime, ProcessSpec},
        system_updates::{
            apt_pin_content_with_holds, changed_protected_packages, check_due, friendly_status,
            installed_versions, missing_libraries, parse_apt_status, parse_upgrade_simulation,
            plan_violations, sort_for_display, still_pending, AptStatus, PendingUpdate,
            UpdateCache, APT_OPTIONS, APT_PIN_REL, CACHE_REL, CRITICAL_BINARIES, INTERRUPTED_REL,
            LOG_GUEST_PATH,
        },
        update_sources::{
            compare_mesa_versions, deb_refresh_needed, mesa_update, parse_http_date, parse_semver,
            release_summary,
            AppRelease, DebStamp, GithubRelease, MesaRelease, UpdatePolicy, CHATGPT_DEB_URL,
            CHATGPT_STAMP_REL, CHECK_INTERVAL_SECS, MESA_RELEASES_API, POLICY_URL,
        },
    },
};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::HashMap,
    fs,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        Arc, Mutex, MutexGuard, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use winit::platform::android::activity::AndroidApp;

/// Wait after the first desktop frame before checking, so startup work
/// (Plasma, Baloo, the first app launch) has the device to itself.
const CHECK_DELAY: Duration = Duration::from_secs(90);
/// How often the background loop asks whether a check is due.
const CHECK_TICK: Duration = Duration::from_secs(30 * 60);
const PUBLISH_INTERVAL: Duration = Duration::from_millis(150);
/// Host-side record of the last check of the non-Debian sources.
const HOST_CACHE_REL: &str = "updates/state.json";
/// The last update policy fetched from the repository.
const POLICY_CACHE_REL: &str = "updates/policy.json";
/// Free space a Debian upgrade needs before it starts: dpkg fails midway,
/// leaving half-configured packages, when the disk fills up.
const MIN_FREE_BYTES: u64 = 1536 << 20;
/// The apt transcript is cut back to its tail beyond this size.
const LOG_LIMIT_BYTES: u64 = 4 << 20;
/// How long a user-started download waits for a lost connection.
const OFFLINE_BUDGET: Duration = Duration::from_secs(5 * 60);

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

/// Where one part of an update run stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StepState {
    Pending,
    Active,
    Done,
    Failed,
    /// It did not run to the end without failing: the user declined
    /// Android's confirmation for the app.
    Skipped,
}

impl StepState {
    const fn code(self) -> u8 {
        match self {
            Self::Pending => 0,
            Self::Active => 1,
            Self::Done => 2,
            Self::Failed => 3,
            Self::Skipped => 4,
        }
    }
}

/// One part of the run the user started, for the Return screen's checklist.
#[derive(Clone, Debug)]
pub struct RunStep {
    pub kind: &'static str,
    pub title: String,
    pub state: StepState,
    /// Why it failed or was skipped.
    pub note: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct HostCache {
    /// Unix seconds of the last check in which every source answered.
    checked_at: u64,
    app: Option<AppRelease>,
    driver: Option<MesaRelease>,
    chatgpt: bool,
}

/// What the Return screen shows.
#[derive(Clone, Debug)]
pub struct UpdateSnapshot {
    pub status: UpdateStatus,
    pub progress: u16,
    pub message: String,
    /// Pending Debian packages.
    pub updates: Vec<PendingUpdate>,
    /// A previous Debian upgrade stopped before it finished.
    pub interrupted: bool,
    pub app: Option<AppRelease>,
    pub driver: Option<MesaRelease>,
    pub chatgpt: bool,
    /// The parts of the current or last run, in order.
    pub steps: Vec<RunStep>,
}

impl UpdateSnapshot {
    fn has_work(&self) -> bool {
        self.interrupted
            || !self.updates.is_empty()
            || self.app.is_some()
            || self.driver.is_some()
            || self.chatgpt
    }

    /// `kind\ttitle\tdetail` per pending non-Debian update, for the UI.
    pub fn component_lines(&self) -> String {
        let clean = |text: &str| text.replace(['\t', '\n'], " ");
        let mut lines = Vec::new();
        if let Some(app) = &self.app {
            let summary = release_summary(&app.notes);
            lines.push(format!(
                "app\tPortal {}\t{}",
                clean(&app.version),
                clean(summary.as_deref().unwrap_or("A new Portal release"))
            ));
        }
        if let Some(driver) = &self.driver {
            lines.push(format!(
                "driver\tGraphics driver\tMesa {} from lfdevs",
                clean(&driver.version)
            ));
        }
        if self.chatgpt {
            lines.push("chatgpt\tChatGPT\tA newer build from OpenAI".to_owned());
        }
        lines.join("\n")
    }

    /// `kind\tstate\ttitle\tnote` per step of the current or last run.
    pub fn step_lines(&self) -> String {
        let clean = |text: &str| text.replace(['\t', '\n'], " ");
        self.steps
            .iter()
            .map(|step| {
                format!(
                    "{}\t{}\t{}\t{}",
                    step.kind,
                    step.state.code(),
                    clean(&step.title),
                    clean(&step.note)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// What one update run installs.
#[derive(Clone, Debug)]
struct Plan {
    driver: Option<MesaRelease>,
    debian: bool,
    chatgpt: bool,
    app: Option<AppRelease>,
}

#[derive(Debug, Default)]
struct RunReport {
    errors: Vec<String>,
    driver_done: bool,
    debian_done: bool,
    chatgpt_done: bool,
    app_done: bool,
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
                app: None,
                driver: None,
                chatgpt: false,
                steps: Vec::new(),
            },
            result: None,
        })
    })
}

/// Serialises every apt run Portal starts (checks, updates, optional apps):
/// apt's own locks fail rather than wait for some of them.
pub fn apt_lock() -> MutexGuard<'static, ()> {
    static APT: Mutex<()> = Mutex::new(());
    APT.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn root() -> &'static Path {
    Path::new(PRODUCTION_FS_ROOT)
}

fn files() -> &'static Path {
    Path::new(APP_FILES_ROOT)
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

/// The update policy in force: the last one fetched, else the APK's copy.
fn current_policy() -> UpdatePolicy {
    fs::read_to_string(files().join(POLICY_CACHE_REL))
        .ok()
        .and_then(|text| UpdatePolicy::parse(&text))
        .unwrap_or_else(UpdatePolicy::builtin)
}

/// Fetch the maintainer's policy. A failed fetch keeps the last good one;
/// a malformed one is ignored rather than loosening anything.
fn refresh_policy() -> UpdatePolicy {
    match update_net::get_text(POLICY_URL, 64 * 1024) {
        Ok(text) => match UpdatePolicy::parse(&text) {
            Some(policy) => {
                if let Err(error) = write_atomic(&files().join(POLICY_CACHE_REL), text.as_bytes()) {
                    log::warn!("updates: could not cache the update policy: {error:#}");
                }
                return policy;
            }
            None => log::warn!("updates: the fetched update policy is malformed; keeping the last one"),
        },
        Err(error) => log::warn!("updates: could not fetch the update policy: {error:#}"),
    }
    current_policy()
}

/// Keep Portal's platform packages, and any the update policy holds, pinned
/// away from Debian's archives. Runs on every launch and before every apt
/// run Portal starts, so a user who deletes or edits the file gets it back.
pub fn sync_apt_policy(fs_root: &Path) -> anyhow::Result<()> {
    let path = fs_root.join(APT_PIN_REL);
    let content = apt_pin_content_with_holds(&current_policy().debian_hold);
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

fn read_host_cache() -> Option<HostCache> {
    serde_json::from_str(&fs::read_to_string(files().join(HOST_CACHE_REL)).ok()?).ok()
}

fn write_host_cache(cache: &HostCache) -> anyhow::Result<()> {
    write_atomic(
        &files().join(HOST_CACHE_REL),
        serde_json::to_string_pretty(cache)?.as_bytes(),
    )
}

/// Drop cached findings that no longer apply: an app release this build
/// already is (or no longer updates from), a driver Portal already has (or
/// must not take), ChatGPT removed since.
fn still_relevant(mut cache: HostCache, installed: Option<&HashMap<String, String>>) -> HostCache {
    let policy = current_policy();
    if cache.app.as_ref().is_some_and(|app| {
        !app_update::enabled()
            || parse_semver(&app.version) <= parse_semver(VERSION)
            || !policy.allows_app(&app.version)
    }) {
        cache.app = None;
    }
    if let Some(driver) = &cache.driver {
        let newer = compare_mesa_versions(&driver.version, &mesa_layer::active_pin().version)
            == Some(Ordering::Greater);
        let denied = mesa_layer::denied_versions().contains(&driver.version);
        if !newer || denied || !policy.allows_mesa(&driver.version) || !crate::android::anland::is_anland_requested() {
            cache.driver = None;
        }
    }
    if installed.is_some_and(|installed| !installed.contains_key("chatgpt")) {
        cache.chatgpt = false;
    }
    cache
}

fn idle_status(snapshot: &UpdateSnapshot) -> UpdateStatus {
    if snapshot.has_work() {
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
    let installed = read_installed(fs_root).ok();
    let mut updates = match (read_cache(fs_root), installed.as_ref()) {
        (Some(cache), Some(installed)) => still_pending(&cache.updates, installed),
        _ => Vec::new(),
    };
    sort_for_display(&mut updates);
    let host = still_relevant(read_host_cache().unwrap_or_default(), installed.as_ref());
    let snapshot = &mut coordinator.snapshot;
    snapshot.interrupted = fs_root.join(INTERRUPTED_REL).is_file();
    snapshot.updates = updates;
    snapshot.app = host.app;
    snapshot.driver = host.driver;
    snapshot.chatgpt = host.chatgpt;
    snapshot.status = idle_status(snapshot);
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

/// Run a guest shell command as root, its output appended to the log.
fn guest(command: &str) -> anyhow::Result<()> {
    let output = PRootRuntime::active().execute(
        ProcessSpec::new(format!("{{ {command} ; }} >>{LOG_GUEST_PATH} 2>&1"))
            .with_env("DEBIAN_FRONTEND", "noninteractive")
            .with_env("APT_LISTCHANGES_FRONTEND", "none"),
        None,
        None,
    );
    anyhow::ensure!(
        output.status.success(),
        "exited with status {:?}; see {LOG_GUEST_PATH}",
        output.status.code()
    );
    Ok(())
}

/// The plan `apt-get upgrade --with-new-pkgs` would carry out from the
/// current package lists. Simulation needs no lock.
fn simulate_upgrade_output() -> anyhow::Result<String> {
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
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn simulate_upgrade() -> anyhow::Result<Vec<PendingUpdate>> {
    Ok(parse_upgrade_simulation(&simulate_upgrade_output()?))
}

fn write_cache(updates: &[PendingUpdate]) -> anyhow::Result<()> {
    let cache = UpdateCache {
        checked_at: now_secs(),
        updates: updates.to_vec(),
    };
    write_atomic(&root().join(CACHE_REL), cache.to_json().as_bytes())
}

/// `apt-get update`, recovering from the usual damage: a half-downloaded
/// list (hash sum mismatch) is dropped and fetched again, and as a last
/// resort every list is (they are only a cache of the mirror).
fn apt_refresh_command() -> String {
    format!(
        "apt-get -q {APT_OPTIONS} update || \
         {{ rm -rf /var/lib/apt/lists/partial/* && apt-get -q {APT_OPTIONS} update ; }} || \
         {{ find /var/lib/apt/lists -maxdepth 1 -type f ! -name lock -delete && \
            apt-get -q {APT_OPTIONS} update ; }}"
    )
}

/// Refresh apt's lists and record what an upgrade would install.
fn check_debian() -> anyhow::Result<usize> {
    let _apt = apt_lock();
    sync_apt_policy(root())?;
    guest(&format!("nice -n 10 sh -c '{}'", apt_refresh_command().replace('\'', "'\\''")))
        .context("apt-get update failed")?;
    let mut updates = simulate_upgrade()?;
    write_cache(&updates)?;
    sort_for_display(&mut updates);
    let count = updates.len();
    if let Ok(mut coordinator) = coordinator().lock() {
        ensure_loaded(&mut coordinator);
        if coordinator.snapshot.status != UpdateStatus::Running {
            coordinator.snapshot.updates = updates;
        }
    }
    Ok(count)
}

fn mtime_secs(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_secs())
}

/// Whether OpenAI's server has a different ChatGPT build than the one
/// installed. False when ChatGPT is not installed.
fn check_chatgpt(installed: &HashMap<String, String>) -> anyhow::Result<bool> {
    if !installed.contains_key("chatgpt") {
        return Ok(false);
    }
    let fs_root = root();
    let stamp = fs::read_to_string(fs_root.join(CHATGPT_STAMP_REL))
        .ok()
        .map(|text| DebStamp::parse(&text));
    let installed_at = mtime_secs(&fs_root.join("var/lib/dpkg/info/chatgpt.list"));
    let (etag, last_modified) = update_net::head(CHATGPT_DEB_URL)?;
    Ok(deb_refresh_needed(
        stamp.as_ref(),
        installed_at,
        etag.as_deref(),
        last_modified.as_deref().and_then(parse_http_date),
    ))
}

/// Check every source once. Each source is independent: one that fails is
/// reported and the others still count. Returns an error listing the
/// failed sources, so the caller retries sooner.
fn run_check(android_app: &AndroidApp) -> anyhow::Result<()> {
    let metered = crate::android::utils::ndk::is_active_network_metered(android_app);
    let policy = refresh_policy();
    let fs_root = root();
    let mut errors = Vec::new();
    let mut cache = read_host_cache().unwrap_or_default();

    if app_update::enabled() {
        match app_update::check(&policy) {
            Ok(app) => cache.app = app,
            Err(error) => errors.push(format!("Portal release: {error:#}")),
        }
    }

    let anland = crate::android::anland::is_anland_requested();
    if anland && mesa_layer::is_provisioned() && !mesa_layer::trial_pending() {
        match update_net::get_json::<Vec<GithubRelease>>(MESA_RELEASES_API) {
            Ok(releases) => {
                cache.driver = mesa_update(
                    &releases,
                    &mesa_layer::active_pin().version,
                    &mesa_layer::denied_versions(),
                    &policy,
                );
            }
            Err(error) => errors.push(format!("graphics driver: {error:#}")),
        }
    } else if !anland {
        cache.driver = None;
    }

    match read_installed(fs_root) {
        Ok(installed) => match check_chatgpt(&installed) {
            Ok(needed) => cache.chatgpt = needed,
            Err(error) => errors.push(format!("ChatGPT: {error:#}")),
        },
        Err(error) => errors.push(format!("{error:#}")),
    }

    // apt's package lists are tens of megabytes: never on a metered
    // network, and not while an app install owns apt.
    if metered {
        log::info!("updates: skipping Debian's package lists on a metered network");
    } else if check_due(read_cache(fs_root).as_ref(), now_secs())
        && !crate::android::proot::optional_apps::is_busy()
    {
        match check_debian() {
            Ok(count) => log::info!("updates: {count} Debian updates available"),
            Err(error) => errors.push(format!("Debian: {error:#}")),
        }
    }

    if errors.is_empty() {
        cache.checked_at = now_secs();
    }
    let installed = read_installed(fs_root).ok();
    let cache = still_relevant(cache, installed.as_ref());
    write_host_cache(&cache)?;
    if let Ok(mut coordinator) = coordinator().lock() {
        ensure_loaded(&mut coordinator);
        let snapshot = &mut coordinator.snapshot;
        if snapshot.status != UpdateStatus::Running {
            snapshot.app = cache.app.clone();
            snapshot.driver = cache.driver.clone();
            snapshot.chatgpt = cache.chatgpt;
            // A finished run's result gives way to whatever is still due.
            if matches!(
                snapshot.status,
                UpdateStatus::None | UpdateStatus::Available | UpdateStatus::Complete
            ) {
                snapshot.status = idle_status(snapshot);
            }
        }
    }
    publish();
    diagnostics::host_event(
        "updates-check",
        &format!(
            "app={} driver={} chatgpt={} errors={}",
            cache.app.as_ref().map_or("-", |app| app.version.as_str()),
            cache.driver.as_ref().map_or("-", |driver| driver.version.as_str()),
            cache.chatgpt,
            errors.len()
        ),
    );

    // Fetch a new Portal release ahead of time on Wi-Fi, so installing it
    // later is instant. Partial downloads resume on the next check.
    app_update::discard_stale(cache.app.as_ref());
    if let (Some(app), false) = (&cache.app, metered) {
        if !app_update::is_downloaded(app) {
            if let Err(error) = app_update::download(app, Duration::from_secs(60), |_, _| {}) {
                log::warn!("updates: could not download Portal {}: {error:#}", app.version);
            }
        }
    }

    anyhow::ensure!(errors.is_empty(), "{}", errors.join("; "));
    Ok(())
}

fn check_is_due() -> bool {
    let now = now_secs();
    match read_host_cache() {
        None => true,
        Some(cache) => cache.checked_at > now || now - cache.checked_at >= CHECK_INTERVAL_SECS,
    }
}

/// Wait before retrying a failed check: 15 minutes, doubling to 2 hours.
fn retry_delay(failures: u32) -> Duration {
    Duration::from_secs(15 * 60 * (1u64 << failures.saturating_sub(1).min(3)))
}

/// Check for updates while Portal runs: first a while after the desktop
/// appears, then whenever the last complete check is older than
/// [`CHECK_INTERVAL_SECS`], retrying failures with a growing delay. Results
/// show on the Return-to-Plasma screen.
pub fn schedule_background_check(android_app: &AndroidApp) {
    static SCHEDULED: AtomicBool = AtomicBool::new(false);
    if SCHEDULED.swap(true, AtomicOrdering::AcqRel) {
        return;
    }
    let android_app = android_app.clone();
    thread::spawn(move || {
        app_update::discard_installed();
        thread::sleep(CHECK_DELAY);
        let mut failures = 0u32;
        loop {
            let ready = crate::core::provisioning::RuntimeArtifact::production().is_bootable(root())
                && !update_in_progress();
            if ready && (failures > 0 || check_is_due()) {
                match run_check(&android_app) {
                    Ok(()) => failures = 0,
                    Err(error) => {
                        failures += 1;
                        log::warn!("updates: check incomplete: {error:#}");
                        diagnostics::host_event("updates-check-failed", &format!("{error:#}"));
                    }
                }
            }
            thread::sleep(if failures > 0 {
                retry_delay(failures)
            } else {
                CHECK_TICK
            });
        }
    });
}

/// Start the updates the user accepted. Returns false when none can start;
/// while one runs, further calls attach to it. `include_app` is false when
/// the user would not let Portal install apps; everything else still runs.
pub fn begin_update(include_app: bool) -> bool {
    let fs_root = root();
    if !crate::core::provisioning::RuntimeArtifact::production().is_bootable(fs_root) {
        log::warn!("Ignoring update: the runtime is not a completed Portal install");
        return false;
    }
    let (repair, _) = crate::android::proot::setup::anland_repair_ui_snapshot();
    if repair == crate::android::proot::setup::AnlandRepairAvailability::Running {
        log::warn!("Ignoring update while the Anland graphics repair runs");
        return false;
    }
    if crate::android::proot::optional_apps::is_busy() {
        log::warn!("Ignoring update while an app is being installed or removed");
        return false;
    }
    let plan = {
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
        let snapshot = &coordinator.snapshot;
        let plan = Plan {
            driver: snapshot.driver.clone(),
            debian: snapshot.interrupted || !snapshot.updates.is_empty(),
            chatgpt: snapshot.chatgpt,
            app: snapshot.app.clone().filter(|_| include_app),
        };
        if plan.driver.is_none() && !plan.debian && !plan.chatgpt && plan.app.is_none() {
            return false;
        }
        let snapshot = &mut coordinator.snapshot;
        snapshot.status = UpdateStatus::Running;
        snapshot.progress = 1;
        snapshot.message = "Closing Plasma…".to_owned();
        snapshot.steps = plan_items(&plan);
        plan
    };
    publish();
    thread::spawn(move || {
        let report = std::panic::catch_unwind(|| run_update(&plan)).unwrap_or_else(|_| RunReport {
            errors: vec!["the update worker panicked".to_owned()],
            ..Default::default()
        });
        finish_update(report);
    });
    true
}

/// The run's checklist, in the order `run_update` works through it.
fn plan_items(plan: &Plan) -> Vec<RunStep> {
    let step = |kind, title: String| RunStep {
        kind,
        title,
        state: StepState::Pending,
        note: String::new(),
    };
    let mut steps = Vec::new();
    if plan.driver.is_some() {
        steps.push(step("driver", "Graphics driver".to_owned()));
    }
    if plan.debian {
        steps.push(step("debian", "Debian packages".to_owned()));
    }
    if plan.chatgpt {
        steps.push(step("chatgpt", "ChatGPT".to_owned()));
    }
    if let Some(app) = &plan.app {
        steps.push(step("app", format!("Portal {}", app.version)));
    }
    steps
}

fn set_step(kind: &str, state: StepState, note: String) {
    update_snapshot(|snapshot| {
        if let Some(step) = snapshot.steps.iter_mut().find(|step| step.kind == kind) {
            step.state = state;
            step.note = note;
        }
    });
}

/// Record one step's outcome; failures also go to the run's report.
fn finish_step(kind: &str, label: &str, result: anyhow::Result<bool>, report: &mut RunReport) -> bool {
    match result {
        Ok(true) => {
            set_step(kind, StepState::Done, String::new());
            true
        }
        Ok(false) => {
            set_step(kind, StepState::Skipped, "Not installed".to_owned());
            false
        }
        Err(error) => {
            set_step(kind, StepState::Failed, format!("{error:#}"));
            report.errors.push(format!("{label}: {error:#}"));
            false
        }
    }
}

/// Maps one step's own 0–100 progress into its share of the whole run.
struct Step {
    from: u16,
    to: u16,
    last: Mutex<Instant>,
}

impl Step {
    fn report(&self, percent: u16, message: String) {
        if let Ok(mut last) = self.last.lock() {
            if last.elapsed() < PUBLISH_INTERVAL && percent < 100 {
                return;
            }
            *last = Instant::now();
        }
        let overall = self.from + (u32::from(percent.min(100)) * u32::from(self.to - self.from) / 100) as u16;
        update_snapshot(|snapshot| {
            snapshot.progress = snapshot.progress.max(overall);
            snapshot.message = message;
        });
    }
}

/// Split 3..97 between the planned steps by rough cost.
fn plan_steps(plan: &Plan) -> Vec<Arc<Step>> {
    let weights: Vec<u16> = [
        plan.driver.is_some().then_some(15),
        plan.debian.then_some(60),
        plan.chatgpt.then_some(15),
        plan.app.is_some().then_some(10),
    ]
    .into_iter()
    .flatten()
    .collect();
    let total: u16 = weights.iter().sum::<u16>().max(1);
    let mut from = 3u16;
    weights
        .into_iter()
        .map(|weight| {
            let to = from + weight * 94 / total;
            let step = Arc::new(Step {
                from,
                to,
                last: Mutex::new(Instant::now() - PUBLISH_INTERVAL),
            });
            from = to;
            step
        })
        .collect()
}

fn run_update(plan: &Plan) -> RunReport {
    let mut report = RunReport::default();
    if let Err(error) = stop_plasma() {
        report.errors.push(format!("{error:#}"));
        return report;
    }
    let mut steps = plan_steps(plan).into_iter();

    if let Some(driver) = &plan.driver {
        let step = steps.next().expect("a step per planned update");
        set_step("driver", StepState::Active, String::new());
        let result = mesa_layer::apply_refresh(driver, |percent, message| step.report(percent, message));
        report.driver_done = finish_step("driver", "Graphics driver", result.map(|()| true), &mut report);
    }
    if plan.debian {
        let step = steps.next().expect("a step per planned update");
        set_step("debian", StepState::Active, String::new());
        let result = run_debian(&step);
        report.debian_done = finish_step("debian", "Debian", result.map(|()| true), &mut report);
    }
    if plan.chatgpt {
        let step = steps.next().expect("a step per planned update");
        set_step("chatgpt", StepState::Active, String::new());
        let result = run_chatgpt(&step);
        report.chatgpt_done = finish_step("chatgpt", "ChatGPT", result.map(|()| true), &mut report);
    }
    if let Some(app) = &plan.app {
        let step = steps.next().expect("a step per planned update");
        set_step("app", StepState::Active, String::new());
        let result = run_app(app, &step);
        report.app_done = finish_step("app", "Portal", result, &mut report);
    }
    report
}

fn stop_plasma() -> anyhow::Result<()> {
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
    Ok(())
}

fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is NUL-terminated and `stat` is a valid out pointer.
    (unsafe { libc::statvfs(path.as_ptr(), &mut stat) } == 0)
        .then(|| stat.f_bavail as u64 * stat.f_frsize as u64)
}

/// Keep the apt transcript bounded: past the limit, keep its last megabyte.
fn trim_log(fs_root: &Path) {
    let path = fs_root.join(LOG_GUEST_PATH.trim_start_matches('/'));
    let Ok(meta) = fs::metadata(&path) else {
        return;
    };
    if meta.len() <= LOG_LIMIT_BYTES {
        return;
    }
    if let Ok(bytes) = fs::read(&path) {
        let tail = &bytes[bytes.len().saturating_sub(1 << 20)..];
        let _ = fs::write(&path, tail);
    }
}

/// `ldd` complaints for the desktop's critical binaries.
fn unresolved_libraries() -> anyhow::Result<Vec<String>> {
    let script = CRITICAL_BINARIES
        .iter()
        .map(|binary| format!("[ -x {binary} ] && ldd {binary}"))
        .collect::<Vec<_>>()
        .join("; ");
    let output = PRootRuntime::active().execute(
        ProcessSpec::new(format!("{{ {script} ; }} 2>&1; true")),
        None,
        None,
    );
    anyhow::ensure!(output.status.success(), "could not inspect the desktop's libraries");
    Ok(missing_libraries(&String::from_utf8_lossy(&output.stdout)))
}

fn run_debian(step: &Arc<Step>) -> anyhow::Result<()> {
    let _apt = apt_lock();
    let fs_root = root();
    if let Some(free) = free_bytes(fs_root) {
        anyhow::ensure!(
            free >= MIN_FREE_BYTES,
            "only {} MB of storage is free; Debian updates need {} MB",
            free >> 20,
            MIN_FREE_BYTES >> 20
        );
    }
    trim_log(fs_root);
    let policy = current_policy();
    sync_apt_policy(fs_root)?;
    let before = read_installed(fs_root)?;
    // Libraries that already do not resolve outside a session (some are
    // only bound in when Plasma starts) are not this update's doing.
    let baseline = unresolved_libraries()?;
    write_atomic(&fs_root.join(INTERRUPTED_REL), b"1\n")?;
    step.report(2, "Refreshing package lists…".to_owned());

    // Finish anything an earlier run left half-done, refresh the lists and
    // repair broken dependencies without removing anything.
    guest(&format!(
        "echo \"== Portal system update $(date -Is) ==\"; \
         (test -x /usr/bin/awk || \
          update-alternatives --quiet --install /usr/bin/awk awk /usr/bin/gawk 10) && \
         dpkg --configure -a && \
         {{ {} ; }} && \
         apt-get -q -y {APT_OPTIONS} --no-remove \
             -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold \
             install --fix-broken",
        apt_refresh_command()
    ))
    .context("preparing apt failed")?;

    // Refuse the plan outright if it would remove anything or touch the
    // platform, before a single package changes.
    let simulated = simulate_upgrade_output()?;
    let violations = plan_violations(&simulated, &policy.debian_hold);
    anyhow::ensure!(
        violations.is_empty(),
        "Portal refused this upgrade because it {}; nothing was installed",
        violations.join(", ")
    );
    step.report(8, "Downloading updates…".to_owned());

    // apt's machine-readable progress goes to fd 3, which is the pipe the
    // host reads; everything else goes to the log. confdef/confold keep the
    // user's edited configuration files without prompting.
    let command = format!(
        "exec 3>&1 >>{LOG_GUEST_PATH} 2>&1; \
         apt-get -q -y {APT_OPTIONS} -o APT::Status-Fd=3 \
             -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold \
             upgrade --with-new-pkgs"
    );
    let last_error = Arc::new(Mutex::new(None::<String>));
    let callback_error = last_error.clone();
    let callback_step = step.clone();
    let output = PRootRuntime::active().execute(
        ProcessSpec::new(command)
            .with_env("DEBIAN_FRONTEND", "noninteractive")
            .with_env("APT_LISTCHANGES_FRONTEND", "none"),
        Some(Arc::new(move |line: String| match parse_apt_status(&line) {
            Some(AptStatus::Error(message)) => {
                if let Ok(mut error) = callback_error.lock() {
                    *error = Some(message);
                }
            }
            Some(AptStatus::Download(percent)) => callback_step.report(
                10 + (percent * 0.35) as u16,
                "Downloading updates…".to_owned(),
            ),
            Some(AptStatus::Install(percent, message)) => {
                callback_step.report(45 + (percent * 0.5) as u16, friendly_status(&message))
            }
            None => {}
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

    step.report(96, "Checking Portal's desktop…".to_owned());
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
    let broken: Vec<String> = unresolved_libraries()?
        .into_iter()
        .filter(|line| !baseline.contains(line))
        .collect();
    if !broken.is_empty() {
        diagnostics::host_event("system-update-broken-libraries", &broken.join(" | "));
        anyhow::bail!(
            "after the update the desktop cannot load: {}; see {LOG_GUEST_PATH}",
            broken.join("; ")
        );
    }
    // Downloaded packages are no use once installed.
    let _ = guest("apt-get -q clean");
    // Whatever apt kept back stays listed; it is not an error.
    match simulate_upgrade() {
        Ok(remaining) => {
            if let Err(error) = write_cache(&remaining) {
                log::warn!("system-updates: could not record the remaining updates: {error:#}");
            }
            if let Ok(mut coordinator) = coordinator().lock() {
                let mut remaining = remaining;
                sort_for_display(&mut remaining);
                coordinator.snapshot.updates = remaining;
            }
        }
        Err(error) => log::warn!("system-updates: post-update check failed: {error:#}"),
    }
    step.report(100, "Debian updates installed".to_owned());
    Ok(())
}

/// Install OpenAI's current ChatGPT build over the installed one.
fn run_chatgpt(step: &Arc<Step>) -> anyhow::Result<()> {
    let _apt = apt_lock();
    sync_apt_policy(root())?;
    step.report(0, "Downloading ChatGPT…".to_owned());
    let command = format!(
        "exec 3>&1 >>{LOG_GUEST_PATH} 2>&1; echo \"== Portal ChatGPT refresh $(date -Is) ==\"; {}",
        crate::core::optional_apps::install_command(OptionalApp::Chatgpt)
    );
    let output = PRootRuntime::active().execute(
        ProcessSpec::new(command)
            .with_env("DEBIAN_FRONTEND", "noninteractive")
            .with_env("APT_LISTCHANGES_FRONTEND", "none"),
        Some(Arc::new({
            let step = step.clone();
            move |line: String| match parse_apt_status(&line) {
                Some(AptStatus::Download(_)) => {
                    step.report(30, "Downloading ChatGPT…".to_owned())
                }
                Some(AptStatus::Install(percent, message)) => {
                    step.report(50 + (percent * 0.5) as u16, friendly_status(&message))
                }
                _ => {}
            }
        })),
        None,
    );
    anyhow::ensure!(
        output.status.success(),
        "the new build did not install (status {:?}); see {LOG_GUEST_PATH}",
        output.status.code()
    );
    step.report(100, "ChatGPT updated".to_owned());
    Ok(())
}

/// Download and install a Portal release. Ok(false) when the user declined
/// Android's confirmation.
fn run_app(app: &AppRelease, step: &Step) -> anyhow::Result<bool> {
    let label = format!("Downloading Portal {}…", app.version);
    step.report(0, label.clone());
    let apk = app_update::download(app, OFFLINE_BUDGET, |done, total| {
        step.report((done * 80 / total.max(1)) as u16, label.clone())
    })?;
    step.report(85, format!("Installing Portal {}…", app.version));
    diagnostics::host_event("app-update-install", &app.version);
    match app_update::install(&apk) {
        InstallOutcome::Installed => Ok(true),
        InstallOutcome::Cancelled => {
            log::info!("app-update: the user cancelled Portal {}", app.version);
            Ok(false)
        }
        InstallOutcome::NeedsPermission => anyhow::bail!(
            "allow Portal to install apps (Settings › Apps › Special app access › Install unknown apps), then try again"
        ),
        InstallOutcome::Failed(message) => anyhow::bail!("{message}"),
    }
}

fn finish_update(report: RunReport) {
    if let Ok(mut coordinator) = coordinator().lock() {
        let snapshot = &mut coordinator.snapshot;
        if report.driver_done {
            snapshot.driver = None;
        }
        if report.debian_done {
            snapshot.interrupted = false;
        } else {
            snapshot.interrupted = root().join(INTERRUPTED_REL).is_file();
        }
        if report.chatgpt_done {
            snapshot.chatgpt = false;
        }
        if report.app_done {
            snapshot.app = None;
        }
        let cache = HostCache {
            checked_at: read_host_cache().map_or(0, |cache| cache.checked_at),
            app: snapshot.app.clone(),
            driver: snapshot.driver.clone(),
            chatgpt: snapshot.chatgpt,
        };
        if let Err(error) = write_host_cache(&cache) {
            log::warn!("updates: could not record what is still pending: {error:#}");
        }
    }
    let result = if report.errors.is_empty() {
        diagnostics::host_event("system-update-complete", "");
        update_snapshot(|snapshot| {
            snapshot.status = UpdateStatus::Complete;
            snapshot.progress = 100;
            snapshot.message = "Restarting Plasma…".to_owned();
        });
        UpdateResult::Installed
    } else {
        let detail = report.errors.join("\n");
        log::error!("updates: update did not complete: {detail}");
        diagnostics::host_event("system-update-failed", &detail);
        update_snapshot(|snapshot| {
            snapshot.status = UpdateStatus::Failed;
            snapshot.message = detail.clone();
        });
        UpdateResult::Failed(detail)
    };
    if let Ok(mut coordinator) = coordinator().lock() {
        coordinator.result = Some(result);
    }
    webview_handoff::wake_event_loop();
}

/// While an update runs, no Plasma session may start: its libraries are
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
