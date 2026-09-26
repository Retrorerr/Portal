use super::process::ArchProcess;
use anyhow::Context;
use crate::{
    android::{
        app::build::PolarBearBackend,
        backend::{
            wayland::{Compositor, TouchMode, WaylandBackend},
            webview::{ErrorVariant, WebviewBackend},
        },
        diagnostics,
        utils::application_context::get_application_context,
        utils::ndk::{
            active_refresh_millihz, long_press_timeout_ms, run_in_jvm, scale_factor, touch_slop_px,
        },
    },
    core::{
        config::{DESKTOP_USER, DOCS_HOME_URL, PRODUCTION_FS_ROOT},
        install_plan::{
            validate_initial_setup_proof, AppliedAppearance, AppearanceChoice, InstallPlan,
            InstallPlanState, OptionalApp, PersistedInstallPlan, INSTALL_PLAN_FILE,
        },
        provisioning::{InstallOperationState, ProvisioningPhase, ProvisioningSnapshot},
    },
};
use jni::{objects::JObject, sys::_jobject};
use pathdiff::diff_paths;
use smithay::utils::Clock;
use std::{
    collections::HashSet,
    fs,
    io::{ErrorKind, Read, Write},
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    process,
    sync::{
        mpsc::{self, Sender},
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use winit::platform::android::activity::AndroidApp;
use crate::{
    android::runtime::proot::PRootRuntime,
    core::runtime::{LinuxRuntime, ProcessSpec},
};

#[derive(Clone, Debug)]
pub enum SetupMessage {
    Progress(String),
    Error(String),
}

pub struct SetupOptions {
    pub android_app: AndroidApp,
    pub mpsc_sender: Sender<SetupMessage>,
    pub progress: Arc<dyn Fn(ProvisioningSnapshot) + Send + Sync>,
    /// Exact native-validated choices accepted before this worker started.
    /// Stages must never recover choices from Compose defaults or current
    /// Android settings.
    pub install_plan: Option<InstallPlan>,
    /// Optional apt selections are first-install work only. Existing
    /// bootable runtimes never replay this transaction on launch.
    pub install_optional_apps: bool,
}

/// Completion hook used by the lifecycle owner to dismiss the provisioning
/// WebView and send an event through its event-loop proxy. Keeping this hook
/// outside `WebviewBackend` avoids recreating the NativeActivity just to swap
/// to the Wayland backend.
pub type SetupCompletionCallback = Arc<dyn Fn() + Send + Sync + 'static>;

const KWIN_WRAPPER: &str = include_str!("../../../assets/localdesktop-kwin-wrapper-v2.sh");
const PLASMA_LAUNCHER: &str = include_str!("../../../assets/localdesktop-startplasma.sh");
const PLASMASHELL_SUPERVISOR: &str =
    include_str!("../../../assets/localdesktop-plasmashell-supervisor.sh");
const RECOVERY_LAUNCHER: &str = include_str!("../../../assets/localdesktop-recovery.sh");
const RETRY_PLASMA: &str = include_str!("../../../assets/localdesktop-retry-plasma.sh");
const PREPARE_DESKTOP_LOGIN: &str = include_str!("../../../assets/localdesktop-prepare-login.py");
const SYSTEM_CACHES: &str = include_str!("../../../assets/localdesktop-system-caches.sh");
const PIPEWIRE_CLIENT_NO_RT: &str =
    include_str!("../../../assets/localdesktop-pipewire-client-no-rt.conf");
const PIPEWIRE_CLIENT_NO_RT_PATHS: [&str; 2] = [
    "etc/pipewire/client.conf.d/90-localdesktop-no-rt.conf",
    "etc/pipewire/client-rt.conf.d/90-localdesktop-no-rt.conf",
];
const INITIAL_APPEARANCE_PLAN: &str = "var/lib/localdesktop/initial-setup-plan-v2";
const INITIAL_APPEARANCE_PROOF: &str = ".local/state/localdesktop/initial-setup-proof-v2";
const INITIAL_APPEARANCE_HANDOFF_TIMEOUT: Duration = Duration::from_secs(240);
const INITIAL_APPEARANCE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const OPTIONAL_APPS_LOG: &str = "/tmp/portal-optional-apps-v1.log";
const KONSOLE_CONFIG: &str = include_str!("../../../assets/konsole/konsolerc");
const KONSOLE_PROFILE: &str = include_str!("../../../assets/konsole/LocalDesktop.profile");
const CRASH_HANDLER_BINARY: &[u8] =
    include_bytes!("../../../assets/guest-arm64/localdesktop-crash-handler.so");
const CRASH_HANDLER_SOURCE: &str = include_str!("../../../assets/localdesktop-crash-handler.c");
const PORTAL_IME_BRIDGE: &str = include_str!("../../../assets/portal-ime-bridge.py");
const PORTAL_IME_DESKTOP: &str = include_str!("../../../assets/portal-ime.desktop");
/// Project Anland X11/GTK input-method bridge: IBus engine routing X11
/// editable focus (real FocusIn/FocusOut) and host commits into GTK clients
/// (proven: Firefox URL bar/input/textarea/contenteditable, exact Unicode).
const PORTAL_IBUS_ENGINE: &str = include_str!("../../../assets/portal-ibus-engine.py");
const PORTAL_IBUS_COMPONENT: &str = include_str!("../../../assets/portal-ibus-component.xml");
/// IBus lazy starter for the Plasma autostart entry. Returns in milliseconds
/// and does all work detached with bounded waits: never apt-get, never a
/// fixed sleep on the splash->desktop path. Packages are provisioned
/// pre-session by `provision_ibus_packages`.
const PORTAL_IBUS_LAZY: &str = include_str!("../../../assets/portal-ibus-lazy.sh");
/// One-time first-run Plasma color scheme and KScreen scale applicator. The
/// autostart entry is inert when native setup has no pending plan or has
/// already accepted the matching attempt-bound proof.
const INITIAL_APPEARANCE_HELPER: &str =
    include_str!("../../../assets/localdesktop-apply-initial-appearance.py");
const INITIAL_APPEARANCE_AUTOSTART: &str =
    include_str!("../../../assets/localdesktop-initial-appearance.desktop");
const CLIPBOARD_SYNC: &str = include_str!("../../../assets/localdesktop-clipboard-sync.sh");
const CLIPBOARD_PUSH: &str = include_str!("../../../assets/localdesktop-clipboard-push.sh");
const WL_COPY_BINARY: &[u8] = include_bytes!("../../../assets/guest-arm64/wl-copy");
const WL_PASTE_BINARY: &[u8] = include_bytes!("../../../assets/guest-arm64/wl-paste");
const KWIN_LIBRARY: &[u8] = include_bytes!("../../../assets/kwin-debian-arm64/libkwin.so.6.3.6");
/// Project Anland unified KWin library: on-device build of Debian KWin 6.3.6
/// with the Anland backend plus the Portal Touchpad port (NaturalScroll /
/// ScrollFactor over D-Bus + kcminputrc, Finger source, axis-stop). Served
/// ONLY to Anland GPU sessions from `/usr/local/lib/portal-anland` so the
/// QPainter overlay path is untouched; the stock distro libkwin is never
/// modified. Synced idempotently on every launch (restart/reinstall persist).
const KWIN_ANLAND_LIBRARY: &[u8] =
    include_bytes!("../../../assets/kwin-anland-arm64/libkwin.so.6.3.6");
/// Phase B XWayland touchpad-source candidate: CI build of pinned lfdevs
/// 461772ae (2:24.1.6-91) plus Portal 0005 (package 2:24.1.6-91portal1),
/// served ONLY from `/usr/local/lib/portal-xwayland` when the session
/// explicitly selects `xwayland-variant=candidate`. `/usr/bin/Xwayland`
/// (stock) is never overwritten; the KWin wrapper falls back to stock on
/// any staging problem. Pins: `assets/xwayland-candidate/SHA256SUMS`.
const XWAYLAND_CANDIDATE_BINARY: &[u8] =
    include_bytes!("../../../assets/xwayland-candidate/Xwayland");
/// Expected SHA-256 of the staged candidate Xwayland (guest-side selection
/// gate refuses anything else; mirrors SHA256SUMS).
const XWAYLAND_CANDIDATE_SHA256: &str =
    "b91f55794942a9efd66300e352cea8c671cdb6ff0c3243a0cc176525cb96812d";
/// Stock Debian trixie arm64 xinput (diagnostic tool for the XI2 proof;
/// same overlay staging, never in the default loader path).
const XWAYLAND_XINPUT_BINARY: &[u8] = include_bytes!("../../../assets/xwayland-candidate/xinput");
/// Project Anland load-time stub: satisfies the lfdevs kwin_wayland binary's
/// AnlandBackend reference when QPainter sessions run the overlay libkwin
/// (which has no Anland backend). Preloaded ONLY in QPainter mode; traps if
/// ever called. Recipe: `assets/guest-arm64/anland-stub-recipe.txt`.
/// A unified overlay build (single libkwin with Anland backend + damage fix)
/// will retire this stub.
const ANLAND_STUB_BINARY: &[u8] = include_bytes!("../../../assets/guest-arm64/libanland-stub.so");
/// Project Anland DRM render-device shim: fakes the open()+version probe so
/// KWin's Anland backend initializes OpenGL on devices where the app sandbox
/// cannot open /dev/dri/renderD128. Preloaded ONLY in Anland sessions.
/// Source: `assets/guest-arm64/drmshim.c` (recipe: `drmshim-recipe.txt`).
const DRMSHIM_BINARY: &[u8] = include_bytes!("../../../assets/guest-arm64/drmshim.so");

/// Setup is a process that should be done **only once** when the user installed the app.
/// The setup process consists of several stages.
/// Each stage is a function that takes the `SetupOptions` and returns a `StageOutput`.
type SetupStage = Box<dyn Fn(&SetupOptions) -> StageOutput + Send>;
type NamedSetupStage = (&'static str, SetupStage);

/// Each stage should indicate whether the associated task is done previously or not.
/// Thus, it should return a finished status if the task is done, so that the setup process can move on to the next stage.
/// Otherwise, it should return a `JoinHandle`, so that the setup process can wait for the task to finish, but not block the main thread so that the setup progress can be reported to the user.
///
/// For coding agents: READ THIS BEFORE ADDING WORK HERE.
/// - Heavy/long work belongs inside the spawned thread of a returned `Some(JoinHandle)`, so it runs once at install and surfaces as setup progress.
/// - Simple/light tasks or important settings that must be run every launch (e.g. the Firefox config) can be done inline on the `None` path.
type StageOutput = Option<JoinHandle<anyhow::Result<()>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupFailureKind {
    Network,
    Storage,
    Verification,
    Extraction,
    Graphics,
    Filesystem,
    Unsupported,
    Stage,
}

#[derive(Clone, Debug)]
pub struct SetupFailure {
    pub stage: String,
    pub kind: SetupFailureKind,
    /// Detailed diagnostics stay in native logs/diagnostics and never cross
    /// the installer UI boundary.
    pub diagnostic: String,
    pub user_message: String,
}

impl SetupFailure {
    fn from_detail(index: usize, name: &str, detail: impl Into<String>) -> Self {
        let diagnostic = detail.into();
        let lower = diagnostic.to_ascii_lowercase();
        let kind = if lower.contains("no space")
            || lower.contains("enospc")
            || lower.contains("storage")
            || lower.contains("free space")
        {
            SetupFailureKind::Storage
        } else if lower.contains("sha")
            || lower.contains("size mismatch")
            || lower.contains("content-range")
            || lower.contains("range")
            || lower.contains("verify")
        {
            SetupFailureKind::Verification
        } else if lower.contains("extract")
            || lower.contains("archive")
            || lower.contains("tar")
            || lower.contains("unsafe runtime")
        {
            SetupFailureKind::Extraction
        } else if lower.contains("http")
            || lower.contains("download")
            || lower.contains("timeout")
            || lower.contains("connection")
            || lower.contains("dns")
        {
            SetupFailureKind::Network
        } else if lower.contains("mesa") || lower.contains("kgsl") || lower.contains("gbm") {
            SetupFailureKind::Graphics
        } else if lower.contains("permission")
            || lower.contains("filesystem")
            || lower.contains("file system")
            || lower.contains("failed to")
        {
            SetupFailureKind::Filesystem
        } else {
            SetupFailureKind::Stage
        };
        let action = match kind {
            SetupFailureKind::Network => "Check your connection and tap Retry.",
            SetupFailureKind::Storage => "Free some storage and tap Retry.",
            SetupFailureKind::Verification => "The download could not be verified. Tap Retry.",
            SetupFailureKind::Extraction => "Debian could not be unpacked safely. Tap Retry.",
            SetupFailureKind::Graphics => "Portal's graphics support could not be prepared. Tap Retry.",
            SetupFailureKind::Filesystem => "Portal could not write its setup files. Tap Retry.",
            SetupFailureKind::Unsupported => "This device cannot run Portal.",
            SetupFailureKind::Stage => "Portal could not finish setup. Tap Retry.",
        };
        Self {
            stage: format!("{index} ({name})"),
            kind,
            diagnostic,
            user_message: format!("{action}"),
        }
    }

    fn from_panic(index: usize, name: &str, payload: &(dyn std::any::Any + Send)) -> Self {
        Self::from_detail(index, name, panic_text(payload))
    }
}

struct SetupSink {
    android_app: AndroidApp,
    senders: Vec<Sender<SetupMessage>>,
    progress: Arc<Mutex<u16>>,
    on_complete: Option<SetupCompletionCallback>,
}

#[derive(Clone)]
struct SetupRegistration {
    // The worker owns this shared sink rather than a particular Activity or
    // WebView receiver. A recreation can bind a fresh UI to the same running
    // operation without starting a second worker or sending JNI calls to a
    // stale Activity instance.
    sink: Arc<Mutex<SetupSink>>,
}

impl SetupRegistration {
    fn new(
        android_app: AndroidApp,
        sender: Sender<SetupMessage>,
        progress: Arc<Mutex<u16>>,
        on_complete: Option<SetupCompletionCallback>,
    ) -> Self {
        Self {
            sink: Arc::new(Mutex::new(SetupSink {
                android_app,
                senders: vec![sender],
                progress,
                on_complete,
            })),
        }
    }

    fn rebind_from(&self, other: &Self) {
        if Arc::ptr_eq(&self.sink, &other.sink) {
            return;
        }
        let Some((android_app, sender, progress, on_complete)) = other
            .sink
            .lock()
            .ok()
            .map(|sink| {
                (
                    sink.android_app.clone(),
                    sink.senders.last().cloned(),
                    sink.progress.clone(),
                    sink.on_complete.clone(),
                )
            })
            .and_then(|(android_app, sender, progress, on_complete)| {
                sender.map(|sender| (android_app, sender, progress, on_complete))
            })
        else {
            return;
        };
        if let Ok(mut sink) = self.sink.lock() {
            sink.android_app = android_app;
            // Only the newest Activity owns a live progress receiver. Drop
            // the old sender so its WebView forwarding thread can terminate
            // instead of accumulating one thread per rotation.
            sink.senders = vec![sender];
            sink.progress = progress;
            sink.on_complete = on_complete;
        }
    }

    fn android_app(&self) -> Option<AndroidApp> {
        self.sink.lock().ok().map(|sink| sink.android_app.clone())
    }

    fn sender(&self) -> Option<Sender<SetupMessage>> {
        self.sink
            .lock()
            .ok()
            .and_then(|sink| sink.senders.last().cloned())
    }

    fn progress(&self) -> Option<Arc<Mutex<u16>>> {
        self.sink.lock().ok().map(|sink| sink.progress.clone())
    }

    fn completion_callback(&self) -> Option<SetupCompletionCallback> {
        self.sink
            .lock()
            .ok()
            .and_then(|sink| sink.on_complete.clone())
    }

    fn publish(&self, message: SetupMessage) -> Option<AndroidApp> {
        let Ok(mut sink) = self.sink.lock() else {
            return None;
        };
        sink.senders.retain(|sender| sender.send(message.clone()).is_ok());
        Some(sink.android_app.clone())
    }
}

struct SetupCoordinator {
    state: InstallOperationState,
    snapshot: ProvisioningSnapshot,
    registration: Option<SetupRegistration>,
    /// The plan used by the running operation.  The app-private persisted
    /// record remains authoritative across process death; this copy only
    /// keeps a live worker from rereading mutable UI state.
    install_plan: Option<InstallPlan>,
    initial_preferences_handoff: InitialPreferencesHandoff,
    pending_initial_preferences_failure: Option<String>,
}

/// A provisional guest is permitted only to apply a user-accepted initial
/// appearance/scale plan.  It has no Portal completion marker until the live
/// KScreen/Plasma proof validates.
#[derive(Clone, Debug, Eq, PartialEq)]
enum InitialPreferencesHandoff {
    Idle,
    Pending(InstallPlan),
    Launched(InstallPlan),
    Proven(InstallPlan),
    Failed(String),
}

impl Default for SetupCoordinator {
    fn default() -> Self {
        Self {
            state: InstallOperationState::Idle,
            snapshot: ProvisioningSnapshot::update(
                ProvisioningPhase::Idle,
                0,
                "Portal setup is waiting to begin.",
            ),
            registration: None,
            install_plan: None,
            initial_preferences_handoff: InitialPreferencesHandoff::Idle,
            pending_initial_preferences_failure: None,
        }
    }
}

fn setup_coordinator() -> &'static Mutex<SetupCoordinator> {
    static COORDINATOR: OnceLock<Mutex<SetupCoordinator>> = OnceLock::new();
    COORDINATOR.get_or_init(|| Mutex::new(SetupCoordinator::default()))
}

/// App-private durable record for a committed first-run plan.  It lives next
/// to `runtime-B`, not inside it, because extraction/promotion is allowed to
/// replace an incomplete image but must never lose a choice already accepted
/// by the user.
pub fn persisted_install_plan_path() -> PathBuf {
    get_application_context().data_dir.join(INSTALL_PLAN_FILE)
}

pub fn load_persisted_install_plan() -> anyhow::Result<Option<PersistedInstallPlan>> {
    PersistedInstallPlan::read(&persisted_install_plan_path())
}

fn persist_install_plan_state(
    plan: &InstallPlan,
    state: InstallPlanState,
) -> anyhow::Result<()> {
    let path = persisted_install_plan_path();
    let existing = PersistedInstallPlan::read(&path)?;
    if let Some(existing) = existing {
        anyhow::ensure!(
            existing.plan() == plan,
            "Persisted install plan does not match the active native plan"
        );
        existing.with_state(state).write_atomic(&path)
    } else {
        anyhow::ensure!(
            state == InstallPlanState::InProgress,
            "Cannot create a failed or completed plan without its accepted record"
        );
        PersistedInstallPlan::new(state, plan.clone())?.write_atomic(&path)
    }
}

fn persist_system_appearance(
    plan: &InstallPlan,
    appearance: AppliedAppearance,
) -> anyhow::Result<AppliedAppearance> {
    let path = persisted_install_plan_path();
    let existing = PersistedInstallPlan::read(&path)?
        .context("Accepted install plan disappeared before appearance application")?;
    anyhow::ensure!(
        existing.plan() == plan,
        "Persisted install plan changed before appearance application"
    );
    if let Some(resolved) = existing.resolved_appearance() {
        return Ok(resolved);
    }
    let resolved = existing.with_resolved_appearance(appearance)?;
    resolved.write_atomic(&path)?;
    Ok(appearance)
}

fn current_android_appearance(android_app: &AndroidApp) -> anyhow::Result<AppliedAppearance> {
    run_in_jvm(
        |env, app| -> anyhow::Result<AppliedAppearance> {
            let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as *mut _jobject) };
            let resources = env
                .call_method(
                    &activity,
                    "getResources",
                    "()Landroid/content/res/Resources;",
                    &[],
                )?
                .l()?;
            let configuration = env
                .call_method(
                    &resources,
                    "getConfiguration",
                    "()Landroid/content/res/Configuration;",
                    &[],
                )?
                .l()?;
            let ui_mode = env.get_field(&configuration, "uiMode", "I")?.i()?;
            match ui_mode & 0x30 {
                0x20 => Ok(AppliedAppearance::Dark),
                0x10 => Ok(AppliedAppearance::Light),
                value => anyhow::bail!(
                    "Android uiMode did not resolve a System appearance (night mask {value:#x})"
                ),
            }
        },
        android_app.clone(),
    )
}

fn resolve_initial_appearance(
    android_app: &AndroidApp,
    plan: &InstallPlan,
) -> anyhow::Result<AppliedAppearance> {
    let record = load_persisted_install_plan()?
        .context("Accepted install plan disappeared before appearance application")?;
    anyhow::ensure!(
        record.plan() == plan,
        "Persisted install plan changed before appearance application"
    );
    if let Some(resolved) = record.resolved_appearance() {
        return Ok(resolved);
    }
    anyhow::ensure!(
        plan.appearance() == AppearanceChoice::System,
        "Explicit appearance plan is missing its committed resolution"
    );
    let current = current_android_appearance(android_app)?;
    persist_system_appearance(plan, current)
}

/// Validate a new JNI proposal or recover the single durable accepted plan.
/// The `InProgress` write is deliberately complete before the caller changes
/// the process coordinator or spawns a provisioning thread.
fn persist_plan_before_start(
    proposed: Option<InstallPlan>,
    retrying: bool,
) -> anyhow::Result<InstallPlan> {
    let path = persisted_install_plan_path();
    match PersistedInstallPlan::read(&path)? {
        Some(existing) => {
            if let Some(proposed) = proposed {
                anyhow::ensure!(
                    existing.plan() == &proposed,
                    "A different first-run plan was already accepted; retry must use that plan"
                );
            }
            anyhow::ensure!(
                existing.state() != InstallPlanState::Complete,
                "The persisted install plan is already complete"
            );
            anyhow::ensure!(
                retrying || existing.state() != InstallPlanState::Failed,
                "A failed install plan must use the explicit retry action"
            );
            let plan = existing.plan().clone();
            existing
                .with_state(InstallPlanState::InProgress)
                .write_atomic(&path)?;
            Ok(plan)
        }
        None => {
            let plan = proposed.context(
                "No first-run plan is persisted; Portal cannot infer setup choices from UI defaults",
            )?;
            let root = Path::new(PRODUCTION_FS_ROOT);
            anyhow::ensure!(
                is_truly_fresh_runtime(root),
                "A first-run plan cannot replace or resume an existing or partial runtime without its accepted plan; export diagnostics or recover the accepted plan"
            );
            plan.validate()?;
            PersistedInstallPlan::new(InstallPlanState::InProgress, plan.clone())?
                .write_atomic(&path)?;
            Ok(plan)
        }
    }
}

fn mark_active_plan_failed() {
    let artifact = crate::core::provisioning::RuntimeArtifact::production();
    if artifact.is_bootable(Path::new(PRODUCTION_FS_ROOT)) {
        // The installation marker is the authoritative commit point. A late
        // bookkeeping/setup error must not downgrade a committed runtime or
        // move its immutable plan back to retryable Failed state.
        log::warn!(
            "Not marking the accepted plan failed because the installation marker is bootable"
        );
        return;
    }
    let plan = setup_coordinator()
        .lock()
        .ok()
        .and_then(|coordinator| coordinator.install_plan.clone());
    let Some(plan) = plan else { return };
    if let Err(error) = persist_install_plan_state(&plan, InstallPlanState::Failed) {
        // The worker already reports its real setup failure.  Do not mask it
        // with a secondary record-write error, but keep enough native detail
        // to diagnose why a later process may conservatively resume it.
        log::error!("Could not persist failed install-plan state: {error:#}");
    }
}

fn mark_active_plan_complete() {
    let plan = setup_coordinator()
        .lock()
        .ok()
        .and_then(|coordinator| coordinator.install_plan.clone());
    let Some(plan) = plan else { return };
    if let Err(error) = persist_install_plan_state(&plan, InstallPlanState::Complete) {
        // The Portal completion marker is already the durable installation
        // commit point.  Keep this record for audit/restart behavior when we
        // can, but never downgrade a validated completed installation.
        log::warn!("Could not persist completed install-plan state: {error:#}");
    }
}

/// Result of the explicit, existing-install Anland graphics migration. This
/// is intentionally separate from installation completion: a successful
/// repair never changes the Debian installation truth.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnlandRepairResult {
    Succeeded,
    Failed(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AnlandRepairState {
    Idle,
    Running,
    Failed,
    Complete,
}

struct AnlandRepairCoordinator {
    state: AnlandRepairState,
    snapshot: ProvisioningSnapshot,
    result: Option<AnlandRepairResult>,
}

impl Default for AnlandRepairCoordinator {
    fn default() -> Self {
        Self {
            state: AnlandRepairState::Idle,
            snapshot: ProvisioningSnapshot::update(
                ProvisioningPhase::Idle,
                0,
                "Anland graphics repair is ready.",
            ),
            result: None,
        }
    }
}

fn anland_repair_coordinator() -> &'static Mutex<AnlandRepairCoordinator> {
    static COORDINATOR: OnceLock<Mutex<AnlandRepairCoordinator>> = OnceLock::new();
    COORDINATOR.get_or_init(|| Mutex::new(AnlandRepairCoordinator::default()))
}

/// Native state exposed to the Return-to-Plasma affordance. Availability is
/// deliberately derived here, next to the repair coordinator, so Compose
/// never guesses from a stale flag or from the presence of Mesa files.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnlandRepairAvailability {
    Unavailable,
    Available,
    Running,
    Failed,
    Complete,
}

impl AnlandRepairAvailability {
    pub const fn code(self) -> i32 {
        match self {
            Self::Unavailable => 0,
            Self::Available => 1,
            Self::Running => 2,
            Self::Failed => 3,
            Self::Complete => 4,
        }
    }
}

/// Return the current process-lifetime repair state and its real native
/// progress snapshot. The normal renderer policy remains conservative: a
/// completed runtime is merely *eligible* when it still resolves to QPainter;
/// this query never changes renderer-mode or starts repair work.
pub fn anland_repair_ui_snapshot() -> (AnlandRepairAvailability, ProvisioningSnapshot) {
    let (state, snapshot) = anland_repair_coordinator()
        .lock()
        .ok()
        .map(|coordinator| (coordinator.state, coordinator.snapshot.clone()))
        .unwrap_or_else(|| {
            (
                AnlandRepairState::Idle,
                ProvisioningSnapshot::update(
                    ProvisioningPhase::Idle,
                    0,
                    "Anland graphics repair is ready.",
                ),
            )
        });

    let availability = match state {
        AnlandRepairState::Running => AnlandRepairAvailability::Running,
        AnlandRepairState::Failed => AnlandRepairAvailability::Failed,
        AnlandRepairState::Complete => AnlandRepairAvailability::Complete,
        AnlandRepairState::Idle => {
            let artifact = crate::core::provisioning::RuntimeArtifact::production();
            let root = Path::new(PRODUCTION_FS_ROOT);
            let eligible = artifact
                .classify_runtime(root)
                .is_trusted_recovery()
                && matches!(
                    crate::android::anland::active_renderer(),
                    crate::android::anland::RendererKind::Smithay
                );
            if eligible {
                AnlandRepairAvailability::Available
            } else {
                AnlandRepairAvailability::Unavailable
            }
        }
    };
    (availability, snapshot)
}

pub fn anland_repair_failed() -> bool {
    anland_repair_coordinator()
        .lock()
        .map(|coordinator| coordinator.state == AnlandRepairState::Failed)
        .unwrap_or(false)
}

/// One-shot handoff token set only after targeted repair validation. It keeps
/// the immediately following `launch()` from replaying the broad normal
/// per-launch sync; the token is consumed before the guest starts, and every
/// later launch uses the normal repair path again.
static PREPARED_ANLAND_LAUNCH: AtomicBool = AtomicBool::new(false);
static ANLAND_REPAIR_HANDOFF_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn take_prepared_anland_launch() -> bool {
    PREPARED_ANLAND_LAUNCH.swap(false, Ordering::AcqRel)
}

pub fn cancel_prepared_anland_launch() {
    PREPARED_ANLAND_LAUNCH.store(false, Ordering::Release);
}

/// Mark the first Plasma launch after a successful repair as a committed
/// runtime launch. If the guest exits asynchronously after `resume_wayland`
/// has returned, the event loop must still use the committed-install Retry
/// Plasma page rather than the unfinished-setup path.
pub fn mark_anland_repair_handoff_active() {
    ANLAND_REPAIR_HANDOFF_ACTIVE.store(true, Ordering::Release);
}

pub fn anland_repair_handoff_active() -> bool {
    ANLAND_REPAIR_HANDOFF_ACTIVE.load(Ordering::Acquire)
}

fn setup_debian_runtime(options: &SetupOptions) -> StageOutput {
    let artifact = crate::core::provisioning::RuntimeArtifact::production();
    if artifact.is_bootable(Path::new(PRODUCTION_FS_ROOT)) {
        return None;
    }
    let report = options.progress.clone();
    let base = get_application_context().data_dir.clone();
    Some(thread::spawn(move || {
        artifact.provision_with_progress(&base, |snapshot| {
            diagnostics::host_event("runtime-provisioning", &snapshot.message);
            report(snapshot);
        })
    }))
}

fn installed_dpkg_packages(fs_root: &Path) -> anyhow::Result<HashSet<String>> {
    let status = fs::read_to_string(fs_root.join("var/lib/dpkg/status"))
        .context("Could not read Debian package status before optional app provisioning")?;
    let mut installed = HashSet::new();
    for stanza in status.split("\n\n") {
        let mut package = None;
        let mut state = None;
        for line in stanza.lines() {
            if let Some(value) = line.strip_prefix("Package: ") {
                package = Some(value.trim());
            } else if let Some(value) = line.strip_prefix("Status: ") {
                state = Some(value.trim());
            }
        }
        if state == Some("install ok installed") {
            if let Some(package) = package {
                installed.insert(package.to_owned());
            }
        }
    }
    Ok(installed)
}

fn validate_optional_app_apt_setup(fs_root: &Path) -> anyhow::Result<()> {
    let apt_sandbox = fs::read_to_string(fs_root.join("etc/apt/apt.conf.d/01no-sandbox"))
        .context("Portal's PRoot-safe apt sandbox policy is missing")?;
    anyhow::ensure!(
        apt_sandbox.contains("APT::Sandbox::User \"root\";"),
        "Portal's PRoot-safe apt sandbox policy is invalid"
    );
    let policy_rc_d = fs_root.join("usr/sbin/policy-rc.d");
    let metadata = fs::metadata(&policy_rc_d)
        .context("Portal's PRoot service-start policy is missing")?;
    #[cfg(unix)]
    anyhow::ensure!(
        metadata.permissions().mode() & 0o111 != 0,
        "Portal's PRoot service-start policy is not executable"
    );
    anyhow::ensure!(
        fs_root.join(DPKG_INFO_MIGRATION_MARKER).is_file(),
        "Debian multiarch package metadata is not ready for apt"
    );
    let architectures = fs::read_to_string(fs_root.join("var/lib/dpkg/arch"))
        .context("Debian dpkg architecture metadata is missing")?;
    anyhow::ensure!(
        architectures.lines().any(|line| line.trim() == "arm64"),
        "Debian dpkg arm64 architecture is not configured"
    );
    anyhow::ensure!(
        fs_root.join("etc/apt/sources.list").is_file(),
        "Debian apt sources are not configured"
    );
    Ok(())
}

/// Install only the fixed native allowlist selected in the durable plan.
/// Debian apt/dpkg preparation has already run in `plasma-wayland`; retry
/// checks package state and installs only missing selections. A requested
/// package that cannot be installed fails setup before Portal's marker.
fn setup_optional_apps(options: &SetupOptions) -> StageOutput {
    if !options.install_optional_apps {
        return None;
    }
    let Some(plan) = options.install_plan.as_ref() else {
        return None;
    };
    let selected_apps = plan.selected_apps();
    if selected_apps.is_empty() {
        provision_ibus_packages(Path::new(PRODUCTION_FS_ROOT));
        return None;
    }
    let fs_root = Path::new(PRODUCTION_FS_ROOT);
    if let Err(error) = validate_optional_app_apt_setup(fs_root) {
        let detail = format!("{error:#}");
        return Some(thread::spawn(move || Err(anyhow::anyhow!(detail))));
    }
    let installed = match installed_dpkg_packages(fs_root) {
        Ok(installed) => installed,
        Err(error) => {
            let detail = format!("{error:#}");
            return Some(thread::spawn(move || Err(anyhow::anyhow!(detail))));
        }
    };
    let missing = selected_apps
        .iter()
        .copied()
        .filter(|app| {
            !app.required_packages()
                .iter()
                .all(|package| installed.contains(*package))
        })
        .collect::<Vec<_>>();
    let chatgpt_selected = selected_apps.contains(&OptionalApp::Chatgpt);
    if missing.is_empty() && !chatgpt_selected {
        provision_ibus_packages(fs_root);
        return None;
    }
    Some(thread::spawn(move || -> anyhow::Result<()> {
        // ProcessSpec launches guest shell source rather than an argv vector,
        // so keep every package token inside fixed enum arms. The UI only
        // chooses which literal command fragments are included.
        let package_commands = missing
            .iter()
            .map(|app| match app {
                OptionalApp::Chatgpt => r#"(apt-get install -y --no-install-recommends curl &&
                    curl --fail --location --retry 3 --proto '=https' --proto-redir '=https' --tlsv1.2 \
                        --output /tmp/portal-chatgpt_arm64.deb.part \
                        https://persistent.oaistatic.com/codex-app-prod/linux/deb/latest/chatgpt_arm64.deb &&
                    test "$(dpkg-deb --field /tmp/portal-chatgpt_arm64.deb.part Package)" = chatgpt &&
                    test "$(dpkg-deb --field /tmp/portal-chatgpt_arm64.deb.part Architecture)" = arm64 &&
                    mv -f /tmp/portal-chatgpt_arm64.deb.part /tmp/portal-chatgpt_arm64.deb &&
                    apt-get install -y --no-install-recommends /tmp/portal-chatgpt_arm64.deb &&
                    rm -f /tmp/portal-chatgpt_arm64.deb)"#,
                OptionalApp::Gimp => "apt-get install -y --no-install-recommends gimp",
                OptionalApp::Inkscape => "apt-get install -y --no-install-recommends inkscape",
                OptionalApp::Krita => "apt-get install -y --no-install-recommends krita",
                OptionalApp::Libreoffice => {
                    "apt-get install -y --no-install-recommends libreoffice libreoffice-kf6"
                }
                OptionalApp::Thunderbird => {
                    "apt-get install -y --no-install-recommends thunderbird"
                }
                OptionalApp::Vlc => "apt-get install -y --no-install-recommends vlc",
            })
            .collect::<Vec<_>>()
            .join(&format!(" >>{OPTIONAL_APPS_LOG} 2>&1 && "));
        if !missing.is_empty() {
            // The extracted image has gawk but not its postinst-created awk
            // alternative. Restore it before configuring a retry's packages.
            let command = format!(
                "(test -x /usr/bin/awk || \
             update-alternatives --quiet --install /usr/bin/awk awk /usr/bin/gawk 10) \
             >>{OPTIONAL_APPS_LOG} 2>&1 && \
             dpkg --configure -a >>{OPTIONAL_APPS_LOG} 2>&1 && \
             apt-get update >>{OPTIONAL_APPS_LOG} 2>&1 && \
             apt-get install -y --no-remove --no-install-recommends --fix-broken >>{OPTIONAL_APPS_LOG} 2>&1 && \
             {package_commands} >>{OPTIONAL_APPS_LOG} 2>&1"
            );
            let spec = ProcessSpec::new(command).with_env("DEBIAN_FRONTEND", "noninteractive");
            let output = PRootRuntime::active().execute(spec, None, None);
            anyhow::ensure!(
                output.status.success(),
                "Optional Debian app installation failed (status {:?}); see {OPTIONAL_APPS_LOG}: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .rev()
                    .take(2048)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            );
        }
        let installed = installed_dpkg_packages(Path::new(PRODUCTION_FS_ROOT))?;
        anyhow::ensure!(
            missing
                .iter()
                .all(|app| app.required_packages().iter().all(|package| installed.contains(*package))),
            "A requested optional Debian app is not fully installed after apt completed"
        );
        if chatgpt_selected {
            anyhow::ensure!(
                installed.contains("chatgpt")
                    && Path::new(PRODUCTION_FS_ROOT)
                        .join("usr/share/applications/chatgpt.desktop")
                        .is_file(),
                "The requested official ChatGPT desktop app is not fully installed"
            );
            // This is a one-time first-run application to the selected user's
            // desktop entry. A later cold start must not regenerate it.
            let user = get_application_context().local_config.user.username;
            let desktop_command = "/usr/local/bin/localdesktop-no-sandbox-entries && grep -q '^X-LocalDesktop-NoSandbox=true$' \"$HOME/.local/share/applications/chatgpt.desktop\" && grep -q '^Exec=.* --no-sandbox' \"$HOME/.local/share/applications/chatgpt.desktop\"";
            let output = PRootRuntime::active().execute(
                ProcessSpec::new(desktop_command).with_user(user),
                None,
                None,
            );
            anyhow::ensure!(
                output.status.success(),
                "The official ChatGPT launcher could not be prepared for Portal's PRoot session: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        provision_ibus_packages(Path::new(PRODUCTION_FS_ROOT));
        Ok(())
    }))
}

/// Establish renderer selection before Mesa and Plasma stages inspect it.
/// This is a small durable config transaction, not a renderer probe: fresh
/// and image-only installs get Anland, while a completed legacy/older v1
/// Portal marker gets an explicit QPainter compatibility value. A write
/// failure is a retryable setup error and therefore cannot be hidden by the
/// final marker.
fn setup_renderer_mode(_options: &SetupOptions) -> StageOutput {
    match crate::android::anland::ensure_renderer_mode() {
        Ok(_) => None,
        Err(error) => Some(thread::spawn(move || -> anyhow::Result<()> {
            Err(error)
        })),
    }
}

fn simulate_linux_sysdata_stage(options: &SetupOptions) -> StageOutput {
    let fs_root = Path::new(PRODUCTION_FS_ROOT);
    let report = options.progress.clone();

    if !fs_root.join("proc/.version").exists() {
        return Some(thread::spawn(move || {
            report(ProvisioningSnapshot::update(
                ProvisioningPhase::Configuring,
                72,
                "Preparing Linux system data…",
            ));

            // Create necessary directories - don't fail if they already exist
            fs::create_dir_all(fs_root.join("proc"))?;
            fs::create_dir_all(fs_root.join("sys"))?;
            fs::create_dir_all(fs_root.join("sys/.empty"))?;

            // Set permissions on the guest directories. A failure is a real
            // setup failure, not something the installer may silently ignore.
            #[cfg(unix)]
            {
                fs::set_permissions(fs_root.join("proc"), fs::Permissions::from_mode(0o700))?;
                fs::set_permissions(fs_root.join("sys"), fs::Permissions::from_mode(0o700))?;
                fs::set_permissions(
                    fs_root.join("sys/.empty"),
                    fs::Permissions::from_mode(0o700),
                )?;
            }

            // Create fake proc files
            let proc_files = [
                ("proc/.version", "Linux version 6.1.0-portal\n"),
                ("proc/.sysctl_entry_cap_last_cap", "40\n"),
                ("proc/.sysctl_inotify_max_user_watches", "4096\n"),
            ];

            for (path, content) in proc_files {
                fs::write(fs_root.join(path), content)?;
            }
            Ok(())
        }));
    }
    None
}

fn setup_machine_id(_: &SetupOptions) -> StageOutput {
    let fs_root = Path::new(PRODUCTION_FS_ROOT);
    let machine_id = fs_root.join("etc/machine-id");

    let existing = fs::read_to_string(&machine_id).unwrap_or_default();
    if !is_valid_machine_id(&existing) {
        if let Some(parent) = machine_id.parent() {
            fs::create_dir_all(parent).expect("Failed to create /etc for machine-id");
        }

        // The file is left read-only (0444) after a successful seed. chmod
        // before rewriting so a repair after a crashed first install does not
        // fail with EACCES and panic every subsequent setup.
        let _ = fs::set_permissions(&machine_id, fs::Permissions::from_mode(0o644));
        fs::write(&machine_id, format!("{}\n", generate_machine_id()))
            .expect("Failed to write machine-id");
        let _ = fs::set_permissions(&machine_id, fs::Permissions::from_mode(0o444));
        log::info!("Seeded guest /etc/machine-id");
    }

    let dbus_dir = fs_root.join("var/lib/dbus");
    fs::create_dir_all(&dbus_dir).expect("Failed to create /var/lib/dbus");
    let dbus_machine_id = dbus_dir.join("machine-id");
    match fs::symlink_metadata(&dbus_machine_id) {
        // A regular file or dangling symlink from a crashed install would
        // otherwise pin divergent IDs forever. Replace anything that is not a
        // symlink to /etc/machine-id.
        Ok(meta) => {
            let needs_repair = if meta.file_type().is_symlink() {
                fs::read_link(&dbus_machine_id).ok().as_deref()
                    != Some(Path::new("/etc/machine-id"))
            } else {
                true
            };
            if needs_repair {
                let _ = fs::remove_file(&dbus_machine_id);
                symlink("/etc/machine-id", &dbus_machine_id)
                    .expect("Failed to symlink /var/lib/dbus/machine-id");
            }
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {
            symlink("/etc/machine-id", &dbus_machine_id)
                .expect("Failed to symlink /var/lib/dbus/machine-id");
        }
        Err(err) => panic!("Failed to inspect /var/lib/dbus/machine-id: {}", err),
    }

    None
}

fn is_valid_machine_id(value: &str) -> bool {
    let value = value.trim();
    value.len() == 32
        && value.chars().all(|c| c.is_ascii_hexdigit())
        && value.chars().any(|c| c != '0')
}

fn generate_machine_id() -> String {
    if let Ok(uuid) = fs::read_to_string("/proc/sys/kernel/random/uuid") {
        let id = uuid.trim().replace('-', "").to_ascii_lowercase();
        if is_valid_machine_id(&id) {
            return id;
        }
    }

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{:016x}{:016x}", nanos as u64, process::id() as u64)
}

pub fn sync_firefox_config(fs_root: &Path) {
    let candidates = [
        fs_root.join("usr/lib/firefox"),
        fs_root.join("usr/lib/firefox-esr"),
    ];

    let autoconfig_js = r#"pref("general.config.filename", "localdesktop.cfg");
pref("general.config.obscure_value", 0);
pref("general.config.sandbox_enabled", false);
"#;

    let firefox_cfg = r#"// Auto updated by Portal on each startup, do not edit manually
defaultPref("media.cubeb.sandbox", false);
defaultPref("security.sandbox.content.level", 0);
defaultPref("media.allow-audio-non-utility", true);
defaultPref("media.rdd-process.enabled", false);
// Project Anland GPU compositing (see src/android/anland/mod.rs): there is
// no DRM render node in PRoot, so Firefox's gfxInfo concludes SOFTWARE_GL
// and blocklists hardware compositing — even though real Adreno contexts
// work (proven: WebGL freedreno, glxtest EGL freedreno). These prefs force
// the GPU path back on for the X11/XWayland backend (KGSL glamor), where
// basic compositing needs no GBM allocation. Native Wayland stays SWGL
// until a render node exists (dmabuf-GBM is unavoidable there).
defaultPref("gfx.webrender.all", true);
defaultPref("layers.acceleration.force-enabled", true);

"#;

    for dir in candidates {
        if dir.exists() || dir.parent().map_or(false, |p| p.exists()) {
            let pref_dir = dir.join("defaults/pref");
            let _ = fs::create_dir_all(&pref_dir);
            let _ = fs::write(pref_dir.join("autoconfig.js"), autoconfig_js);
            let _ = fs::write(dir.join("localdesktop.cfg"), firefox_cfg);
        }
    }
}

fn validate_firefox_anland_config(fs_root: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        ["usr/bin/firefox", "usr/bin/firefox-esr"]
            .iter()
            .any(|relative| fs_root.join(relative).is_file()),
        "Firefox is not installed in the Portal runtime"
    );

    let mut checked = 0usize;
    for dir in [fs_root.join("usr/lib/firefox"), fs_root.join("usr/lib/firefox-esr")] {
        if !dir.is_dir() {
            continue;
        }
        checked += 1;
        let autoconfig = fs::read_to_string(dir.join("defaults/pref/autoconfig.js"))
            .map_err(|error| anyhow::anyhow!("Firefox autoconfig is unreadable: {error}"))?;
        anyhow::ensure!(
            autoconfig.contains("pref(\"general.config.filename\", \"localdesktop.cfg\");"),
            "Firefox Portal autoconfig is incomplete"
        );
        let config = fs::read_to_string(dir.join("localdesktop.cfg"))
            .map_err(|error| anyhow::anyhow!("Firefox Portal config is unreadable: {error}"))?;
        for required in [
            "defaultPref(\"gfx.webrender.all\", true);",
            "defaultPref(\"layers.acceleration.force-enabled\", true);",
        ] {
            anyhow::ensure!(
                config.contains(required),
                "Firefox Portal GPU preference is missing: {required}"
            );
        }
    }
    anyhow::ensure!(checked > 0, "Firefox configuration directory is missing");
    Ok(())
}

fn sync_guest_session_directories(fs_root: &Path) -> anyhow::Result<()> {
    // Normally created by systemd-tmpfiles, which does not run in PRoot.
    // KWin refuses to start Xwayland without this socket directory, and
    // Debian's ksmserver still needs that X connection in a Wayland session.
    // `tmp` itself must also be world-writable: the image ships it as 0755,
    // which breaks the Pulse native socket and wayland-0 in the guest.
    for relative in ["tmp", "tmp/.X11-unix", "tmp/.ICE-unix", "var/tmp"] {
        let directory = fs_root.join(relative);
        fs::create_dir_all(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o1777))?;
    }
    Ok(())
}

/// Stage the initial-appearance autostart only for a newly accepted install
/// plan. This must not be part of normal session asset synchronization: an
/// already bootable desktop owns its appearance and display configuration.
pub fn stage_initial_appearance_helper(fs_root: &Path) -> anyhow::Result<()> {
    let completion_marker = fs_root.join(".portal-runtime-complete");
    match fs::symlink_metadata(&completion_marker) {
        Ok(_) => anyhow::bail!(
            "Refusing to stage initial-appearance setup into a completed runtime"
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    stage_initial_appearance_asset(
        fs_root,
        "usr/local/bin/localdesktop-apply-initial-appearance",
        normalize_guest_text(INITIAL_APPEARANCE_HELPER).as_bytes(),
        0o755,
    )?;
    stage_initial_appearance_asset(
        fs_root,
        "etc/xdg/autostart/localdesktop-initial-appearance.desktop",
        normalize_guest_text(INITIAL_APPEARANCE_AUTOSTART).as_bytes(),
        0o644,
    )
}

fn stage_initial_appearance_asset(
    fs_root: &Path,
    relative_path: &str,
    contents: &[u8],
    mode: u32,
) -> anyhow::Result<()> {
    let path = fs_root.join(relative_path);
    let parent = path
        .parent()
        .context("Initial-appearance asset has no parent directory")?;
    fs::create_dir_all(parent).context("Could not create initial-appearance asset directory")?;
    let parent_metadata = fs::symlink_metadata(parent)
        .context("Could not inspect initial-appearance asset directory")?;
    anyhow::ensure!(
        parent_metadata.is_dir() && !parent_metadata.file_type().is_symlink(),
        "Initial-appearance asset directory is not a real directory"
    );

    let temporary = path.with_extension("portal-tmp");
    let write_result = (|| -> anyhow::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .context("Could not create staged initial-appearance asset")?;
        file.write_all(contents)
            .context("Could not write staged initial-appearance asset")?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(mode))
            .context("Could not set staged initial-appearance asset permissions")?;
        file.sync_all()
            .context("Could not sync staged initial-appearance asset")?;
        drop(file);
        fs::rename(&temporary, &path).context("Could not install initial-appearance asset")?;
        if mode & 0o111 != 0 {
            if let Some(name) = path.file_name() {
                let sidecar =
                    parent.join(format!(".proot-meta-file.{}.meta", name.to_string_lossy()));
                let _ = fs::remove_file(sidecar);
            }
        }
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .context("Could not sync initial-appearance asset directory")?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result?;
    Ok(())
}

fn supervise_plasmashell_autostart(original: &str) -> anyhow::Result<String> {
    const STOCK: &str = "Exec=/usr/bin/plasmashell";
    const SUPERVISED: &str = "Exec=/usr/local/bin/localdesktop-plasmashell-supervisor";
    let exec_lines: Vec<_> = original.lines().filter(|line| line.starts_with("Exec=")).collect();
    anyhow::ensure!(
        exec_lines.len() == 1 && (exec_lines[0] == STOCK || exec_lines[0] == SUPERVISED),
        "unexpected Plasma shell autostart command"
    );
    if exec_lines[0] == SUPERVISED {
        return Ok(original.to_owned());
    }
    Ok(original.replacen(STOCK, SUPERVISED, 1))
}

/// Refresh only Portal-owned files needed by the running session. This is
/// shared by normal launch repair and the explicit Anland migration. It never
/// touches Debian packages or user home/configuration state.
fn sync_portal_runtime_assets(fs_root: &Path, ui_scale: i32) {
    // The guest scripts are versioned assets so the classic startup contract,
    // KWin crash capture and graphical recovery UI cannot drift apart. The
    // launcher substitutes only the device-specific scale factor.
    // Debugger capture stays off in all builds: running every KWin instance
    // under gdb changes startup timing and ptrace is commonly denied by
    // Android's sandbox. It remains opt-in via the environment override.
    let gdb_backtrace = "0";
    let launcher = PLASMA_LAUNCHER
        .replace("@UI_SCALE@", &ui_scale.to_string())
        .replace("@GDB_BACKTRACE@", gdb_backtrace);
    write_executable(
        &fs_root.join("usr/local/bin/startplasma-localdesktop"),
        &launcher,
    );
    write_executable(
        &fs_root.join("usr/local/bin/localdesktop-plasmashell-supervisor"),
        PLASMASHELL_SUPERVISOR,
    );
    let shell_autostart = fs_root.join("etc/xdg/autostart/org.kde.plasmashell.desktop");
    let desktop_entry = fs::read_to_string(&shell_autostart)
        .expect("Failed to read the distro Plasma shell autostart entry");
    let supervised_entry = supervise_plasmashell_autostart(&desktop_entry)
        .expect("Could not preserve Plasma shell autostart metadata");
    if supervised_entry != desktop_entry {
        fs::write(&shell_autostart, supervised_entry)
            .expect("Failed to install Plasma shell autostart supervision");
    }
    write_executable(&fs_root.join("usr/local/bin/kwin_wayland"), KWIN_WRAPPER);
    let recovery_launcher = RECOVERY_LAUNCHER.replace("@UI_SCALE@", &ui_scale.to_string());
    write_executable(
        &fs_root.join("usr/local/bin/start-localdesktop-recovery"),
        &recovery_launcher,
    );
    write_executable(
        &fs_root.join("usr/local/bin/localdesktop-retry-plasma"),
        RETRY_PLASMA,
    );
    write_executable(
        &fs_root.join("usr/local/bin/localdesktop-prepare-login"),
        PREPARE_DESKTOP_LOGIN,
    );
    write_executable(
        &fs_root.join("usr/local/bin/localdesktop-system-caches"),
        SYSTEM_CACHES,
    );
    write_executable(
        &fs_root.join("usr/local/bin/localdesktop-clipboard-sync"),
        CLIPBOARD_SYNC,
    );
    write_executable(
        &fs_root.join("usr/local/bin/localdesktop-clipboard-push"),
        CLIPBOARD_PUSH,
    );
    // Debian Trixie's locked runtime still carries wl-clipboard 2.2.1,
    // which predates KWin's ext-data-control support. Bundle the matching
    // ARM64 2.3 clients into /usr/local/bin so the helper is authoritative
    // across existing and newly provisioned runtime slots.
    write_guest_binary(&fs_root.join("usr/local/bin/wl-copy"), WL_COPY_BINARY);
    write_guest_binary(&fs_root.join("usr/local/bin/wl-paste"), WL_PASTE_BINARY);
    // ABI-matched optional backend, reproduced by build_canberra_backend.py.
    // libcanberra discovers modules here; no package manager runs at startup.
    write_guest_binary(
        &fs_root.join("usr/lib/aarch64-linux-gnu/libcanberra-0.30/libcanberra-pulse.so"),
        include_bytes!("../../../assets/guest-arm64/libcanberra-pulse.so"),
    );
    let canberra_notice = fs_root.join("usr/share/doc/portal-canberra-backend");
    fs::create_dir_all(&canberra_notice).expect("Failed to create backend license directory");
    fs::write(
        canberra_notice.join("copyright"),
        include_bytes!("../../../assets/guest-arm64/libcanberra-copyright"),
    )
    .expect("Failed to install backend license");
    // Migrate older APKs that shadowed the distribution's splash executables.
    let _ = fs::remove_file(fs_root.join("usr/local/bin/ksplashqml"));
    let _ = fs::remove_file(fs_root.join("usr/local/bin/plasma_waitforname"));
    write_executable(
        &fs_root.join("usr/local/bin/portal-ime-bridge"),
        PORTAL_IME_BRIDGE,
    );
    // Project Anland X11/GTK input-method bridge: IBus engine routing X11
    // editable focus and host commits into GTK clients (proven: Firefox URL
    // bar/input/textarea/contenteditable, exact Unicode). Synced idempotently
    // like the ime bridge (size-gated rewrite).
    write_executable(
        &fs_root.join("usr/local/bin/portal-ibus-engine"),
        PORTAL_IBUS_ENGINE,
    );
    // Non-blocking autostart launcher for the IBus daemon/engine (see
    // assets/portal-ibus-lazy.sh). Deployed idempotently like the engine.
    write_executable(
        &fs_root.join("usr/local/bin/portal-ibus-lazy"),
        PORTAL_IBUS_LAZY,
    );
    let ibus_component_path = fs_root.join("usr/share/ibus/component/portal.xml");
    if let Some(parent) = ibus_component_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(ibus_component_path, PORTAL_IBUS_COMPONENT);
    let ime_desktop_path = fs_root.join("usr/share/applications/portal-ime.desktop");
    if let Some(parent) = ime_desktop_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(ime_desktop_path, PORTAL_IME_DESKTOP);
    // Stop guest PipeWire clients from clamping RLIMIT_RTTIME to 0 through
    // the RTKit-less realtime portal (see the asset for the failure mode).
    for relative in PIPEWIRE_CLIENT_NO_RT_PATHS {
        let path = fs_root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("Failed to create PipeWire client config directory");
        }
        fs::write(&path, PIPEWIRE_CLIENT_NO_RT)
            .expect("Failed to install PipeWire client realtime policy");
    }
    // Keep the QPainter overlay present and the default loader path free of
    // shadows on every launch (idempotent; see sync_kwin_overlay).
    sync_kwin_overlay(fs_root);
}

fn sync_crash_handler(fs_root: &Path) -> anyhow::Result<()> {
    // All builds need the socket fstat fix in this existing library. A
    // nested gdb frequently dies before it can attach under Android's PRoot;
    // this preload still records the fault PC/LR/SP, loader maps and a
    // best-effort glibc backtrace from inside KWin.
    let crash_handler_source = fs_root.join("usr/local/lib/localdesktop-crash-handler.c");
    if let Some(parent) = crash_handler_source.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &crash_handler_source,
        normalize_guest_text(CRASH_HANDLER_SOURCE),
    )?;
    fs::set_permissions(&crash_handler_source, fs::Permissions::from_mode(0o644))?;

    let handler = fs_root.join("usr/local/lib/localdesktop-crash-handler.so");
    let temporary = handler.with_extension("so.tmp");
    fs::write(&temporary, CRASH_HANDLER_BINARY)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))?;
    fs::rename(&temporary, &handler)?;
    diagnostics::guest_event(
        "guest-support",
        "installed bundled ARM64 glibc socket-stat shim",
    );
    Ok(())
}

/// The explicit Anland migration deliberately avoids the broad normal-launch
/// repair routine. It refreshes only root-owned session assets and graphics
/// integration; Debian package state and all user home/configuration files
/// remain untouched.
fn sync_anland_required_session_files(
    fs_root: &Path,
    ui_scale: i32,
) -> anyhow::Result<()> {
    sync_guest_session_directories(fs_root)?;
    sync_firefox_config(fs_root);
    sync_portal_runtime_assets(fs_root, ui_scale);
    // Normal launch keeps size-gated overlay checks cheap. An explicit repair
    // is the point where same-size corruption must also be replaced.
    sync_kwin_anland_overlay_for_repair(fs_root)?;
    let drmshim = fs_root.join("usr/local/lib/portal/drmshim.so");
    if !fs::read(&drmshim)
        .map(|bytes| bytes == DRMSHIM_BINARY)
        .unwrap_or(false)
    {
        write_guest_binary_result(&drmshim, DRMSHIM_BINARY)?;
    }
    sync_crash_handler(fs_root)?;
    validate_required_session_files(fs_root)
}

fn setup_firefox_config(_: &SetupOptions) -> StageOutput {
    use crate::core::runtime::LinuxRuntime;
    let active_runtime = crate::android::runtime::proot::PRootRuntime::active();
    sync_firefox_config(&active_runtime.rootfs_path());
    None
}

#[derive(Debug)]
enum KvLine {
    Entry {
        key: String,
        value: String,
        prefix: String,
        delimiter: char,
    },
    Other(String),
}

fn parse_kv_lines(content: &str, delimiter: char) -> Vec<KvLine> {
    content
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
                return KvLine::Other(line.to_string());
            }
            if let Some((left, right)) = line.split_once(delimiter) {
                let key = left.trim().to_string();
                if key.is_empty() {
                    return KvLine::Other(line.to_string());
                }
                let prefix_len = line.len() - trimmed.len();
                let prefix = line[..prefix_len].to_string();
                let value = right.trim().to_string();
                KvLine::Entry {
                    key,
                    value,
                    prefix,
                    delimiter,
                }
            } else {
                KvLine::Other(line.to_string())
            }
        })
        .collect()
}

fn set_kv_value(lines: &mut Vec<KvLine>, key: &str, value: &str, delimiter: char) {
    let mut updated = false;
    for line in lines.iter_mut() {
        if let KvLine::Entry {
            key: entry_key,
            value: entry_value,
            ..
        } = line
        {
            if entry_key == key {
                *entry_value = value.to_string();
                updated = true;
            }
        }
    }
    if !updated {
        lines.push(KvLine::Entry {
            key: key.to_string(),
            value: value.to_string(),
            prefix: String::new(),
            delimiter,
        });
    }
}

fn render_kv_lines(lines: &[KvLine]) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        match line {
            KvLine::Entry {
                key,
                value,
                prefix,
                delimiter,
            } => out.push(format!("{}{}{} {}", prefix, key, delimiter, value)),
            KvLine::Other(raw) => out.push(raw.to_string()),
        }
    }
    let mut content = out.join("\n");
    content.push('\n');
    content
}

fn upsert_kv_file(path: &Path, delimiter: char, updates: &[(&str, String)]) {
    let content = fs::read_to_string(path).unwrap_or_default();
    let mut lines = parse_kv_lines(&content, delimiter);
    for (key, value) in updates {
        set_kv_value(&mut lines, key, value, delimiter);
    }
    let content = render_kv_lines(&lines);
    fs::write(path, content).expect("Failed to write key/value file");
}

fn setup_fake_bwrap(_: &SetupOptions) -> StageOutput {
    let fs_root = Path::new(PRODUCTION_FS_ROOT);
    let wrapper_path = fs_root.join("usr/local/bin/bwrap");

    // bwrap (Bubblewrap) requires Linux user namespaces (CLONE_NEWUSER) which are
    // blocked by Android SELinux. We replace it with a shim that strips all
    // namespace/sandbox flags and directly exec's the target binary.
    // This unblocks glycin-svg (used by Onboard) which sandbox-loads SVG files via bwrap.
    let wrapper = r#"#!/bin/sh
# bwrap shim for proot/Android: namespaces are unavailable, exec directly.
# Strips all bwrap sandbox/namespace/bind flags, then exec's the target binary.
while [ $# -gt 0 ]; do
    case "$1" in
        # Three-argument flags (flag + src/key + dest/value)
        --ro-bind|--bind|--dev-bind|--bind-try|--ro-bind-try|--dev-bind-try|\
        --file|--bind-data|--ro-bind-data|--symlink|\
        --setenv|--chmod) shift 3 ;;
        # Two-argument flags (flag + single arg)
        --tmpfs|--proc|--dir|\
        --unsetenv|--perms|--cap-add|--cap-drop|\
        --seccomp|--add-seccomp-fd|--info-fd|--json-status-fd|\
        --block-fd|--userns-block-fd|--userns|--userns2|\
        --pidns|--chdir|--dev|--mqueue) shift 2 ;;
        # Zero-argument flags
        --unshare-all|--unshare-user|--unshare-user-try|--unshare-pid|\
        --unshare-ipc|--unshare-net|--unshare-uts|--unshare-cgroup|\
        --unshare-cgroup-try|--share-net|--remount-ro|\
        --as-pid-1|--die-with-parent|--new-session|--clearenv) shift ;;
        --) shift; break ;;
        *) break ;;
    esac
done
exec "$@"
"#;

    let _ = fs::create_dir_all(
        wrapper_path
            .parent()
            .expect("Failed to read bwrap wrapper parent directory"),
    );
    fs::write(&wrapper_path, wrapper).expect("Failed to write bwrap wrapper");
    fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755))
        .expect("Failed to mark bwrap wrapper executable");

    None
}

fn setup_chromium_no_sandbox(_: &SetupOptions) -> StageOutput {
    let fs_root = Path::new(PRODUCTION_FS_ROOT);

    // Chromium's sandbox needs CLONE_NEWUSER, which Android SELinux blocks.  Electron also
    // initializes Node against inherited descriptors that PRoot cannot faithfully expose,
    // and Xwayland authentication is not a reliable boundary for guest-launched clients.
    // Keep an opt-in helper for Chromium/Electron desktop entries. Never run it
    // from session startup or apt: those hooks recreated user-removed entries.
    write_executable(
        &fs_root.join("usr/local/bin/localdesktop-no-sandbox-entries"),
        r#"#!/bin/sh
target_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
mkdir -p "$target_dir" || exit 1

for src in /usr/share/applications/*.desktop /usr/local/share/applications/*.desktop; do
    [ -f "$src" ] || continue

    prog=$(sed -n 's/^Exec=//p' "$src" | head -n1 | awk '{print $1}')
    [ -n "$prog" ] || continue
    case "$prog" in
        /*) bin="$prog" ;;
        *) bin=$(command -v "$prog" 2>/dev/null) || continue ;;
    esac
    bin=$(readlink -f "$bin" 2>/dev/null)
    [ -n "$bin" ] || continue

    dir=$(dirname "$bin")
    electron=0
    for root in "$dir" "$dir/.."; do
        if [ -f "$root/resources/app.asar" ] || [ -d "$root/resources/app" ]; then
            electron=1
            break
        fi
    done
    if [ "$electron" -ne 1 ] &&
       [ ! -e "$dir/chrome-sandbox" ] && [ ! -e "$dir/../chrome-sandbox" ]; then
        continue
    fi

    dst="$target_dir/$(basename "$src")"
    # Leave alone anything the user wrote themselves.
    if [ -e "$dst" ] && ! grep -q '^X-LocalDesktop-NoSandbox=' "$dst"; then
        continue
    fi

    tmp="$dst.portal-tmp.$$"
    if ! awk -v electron="$electron" '
        /^\[Desktop Entry\]/ && !seen { print; print "X-LocalDesktop-NoSandbox=true"; seen = 1; next }
        /^Exec=/ {
            if (index($0, "--no-sandbox") == 0)
                sub(/^Exec=[^ ]+/, "& --no-sandbox")
            if (electron == 1 && index($0, "--no-stdio-init") == 0)
                sub(/^Exec=[^ ]+/, "& --no-stdio-init")
            if (electron == 1 && index($0, "--ozone-platform=") == 0)
                sub(/^Exec=[^ ]+/, "& --ozone-platform=wayland")
        }
        { print }
    ' "$src" > "$tmp" || ! chmod 0644 "$tmp" || ! mv -f "$tmp" "$dst"; then
        rm -f "$tmp"
        printf 'Failed to update Portal desktop integration for %s\n' "$src" >&2
    fi
done
"#,
    );

    // Same flag for terminal launches, following the /usr/local/bin PATH-priority pattern.
    write_executable(
        &fs_root.join("usr/local/bin/chromium"),
        r#"#!/bin/sh
[ -x /usr/bin/chromium ] || { echo "chromium is not installed" >&2; exit 127; }
exec /usr/bin/chromium --no-sandbox "$@"
"#,
    );

    None
}

fn setup_onboard_signal_fix(_: &SetupOptions) -> StageOutput {
    let fs_root = Path::new(PRODUCTION_FS_ROOT);
    let wrapper_path = fs_root.join("usr/local/bin/onboard");

    // proot intercepts fstat() on socket fds and follows /proc/self/fd/N which points
    // to "socket:[inode]" — not a real path. Python 3.14's signal.set_wakeup_fd()
    // calls fstat(fd) to validate the wakeup socket, which fails with ENOENT under proot.
    // We install a wrapper at /usr/local/bin/onboard (higher PATH priority than /usr/sbin)
    // that monkey-patches signal.set_wakeup_fd to swallow OSError before launching the
    // real Onboard binary.
    let wrapper = r#"#!/usr/bin/python3
# Onboard wrapper for proot/Android: patches signal.set_wakeup_fd to handle
# OSError (ENOENT) caused by proot's fstat translation on socket file descriptors.
import signal as _signal
_orig_swf = _signal.set_wakeup_fd
def _safe_swf(fd, **kwargs):
    try:
        return _orig_swf(fd, **kwargs)
    except OSError:
        return -1
_signal.set_wakeup_fd = _safe_swf

import runpy, sys
sys.argv[0] = '/usr/sbin/onboard'
runpy.run_path('/usr/sbin/onboard', run_name='__main__')
"#;

    let _ = fs::create_dir_all(
        wrapper_path
            .parent()
            .expect("Failed to read onboard wrapper parent directory"),
    );
    fs::write(&wrapper_path, wrapper).expect("Failed to write onboard wrapper");
    fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755))
        .expect("Failed to mark onboard wrapper executable");

    None
}

fn chroot_home_dir(fs_root: &Path, username: &str) -> PathBuf {
    if username == "root" {
        fs_root.join("root")
    } else {
        fs_root.join(format!("home/{username}"))
    }
}

fn normalize_guest_text(contents: &str) -> String {
    // The Android build often runs from a Windows checkout. Keep generated
    // guest text deterministic even when Git has materialized an asset with
    // CRLF line endings.
    contents.replace("\r\n", "\n").replace('\r', "\n")
}

fn write_executable(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // This source tree is checked out on Windows, where Git may materialize
    // text assets with CRLF. A guest kernel interprets the shebang literally,
    // so `#!/bin/bash\r` fails with ENOENT. Normalize at the Android/guest
    // boundary rather than relying on a developer's Git attributes.
    // Stage through a temporary file and rename into place: sync runs on
    // every session launch while kwin_wayland_wrapper resolves
    // /usr/local/bin/kwin_wayland via PATH concurrently. A direct truncate
    // + write (+ chmod after) exposes incomplete/non-executable windows in
    // which resolution silently falls through to stock /usr/bin/kwin_wayland,
    // which then crashes without Anland and wedges the boot. Rename is
    // atomic: concurrent execs always see the old or the new complete script.
    let temporary = path.with_extension("portal-tmp");
    fs::write(&temporary, normalize_guest_text(contents))
        .expect("Failed to write executable script");
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))
        .expect("Failed to mark executable script");
    fs::File::open(&temporary)
        .and_then(|file| file.sync_all())
        .expect("Failed to sync executable script");
    fs::rename(&temporary, path).expect("Failed to install executable script");
    // PRoot's fake-root extension gives a `.proot-meta-file.<name>.meta`
    // sidecar precedence over the host mode for access()/exec. A stale
    // non-executable record can send PATH lookup past this wrapper. These
    // Portal-owned scripts are restaged before a new guest session starts, so
    // remove exactly the destination sidecar after installing the replacement.
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        let sidecar = parent.join(format!(".proot-meta-file.{}.meta", name.to_string_lossy()));
        let _ = fs::remove_file(sidecar);
    }
}

fn write_guest_binary(path: &Path, contents: &[u8]) {
    write_guest_binary_result(path, contents).expect("Failed to install guest binary");
}

fn write_guest_binary_result(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("portal-tmp");
    fs::write(&temporary, contents)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))?;
    fs::File::open(&temporary)?.sync_all()?;
    fs::rename(&temporary, path)?;
    if let Some(parent) = path.parent() {
        crate::core::mesa_layer::sync_dir_best_effort(parent);
    }
    Ok(())
}

/// Install a shipped configuration without clobbering a user's later edits.
/// Executable launch/recovery scripts are always refreshed above so upgrades
/// receive fixes, while normal application preferences remain user-owned.
fn write_default_file(path: &Path, contents: &str) {
    if path.exists() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(path, normalize_guest_text(contents))
        .expect("Failed to write default guest configuration");
}

/// Update one KConfig key while preserving unrelated groups, comments and
/// user preferences. Plasma's classic-session switch is an upgrade-sensitive
/// setting, so unlike an initial default it must be repaired on every setup.
fn upsert_kconfig_value(path: &Path, group: &str, key: &str, value: &str) {
    let content = fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    let group_header = format!("[{group}]");
    let group_start = lines
        .iter()
        .position(|line| line.trim().eq_ignore_ascii_case(&group_header));

    let Some(group_start) = group_start else {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(group_header);
        lines.push(format!("{key}={value}"));
        let mut output = lines.join("\n");
        output.push('\n');
        fs::write(path, output).expect("Failed to write KConfig value");
        return;
    };

    let group_end = lines
        .iter()
        .enumerate()
        .skip(group_start + 1)
        .find(|(_, line)| {
            let trimmed = line.trim();
            trimmed.starts_with('[') && trimmed.ends_with(']')
        })
        .map(|(index, _)| index)
        .unwrap_or(lines.len());
    let mut key_index = None;
    for (index, line) in lines
        .iter()
        .enumerate()
        .take(group_end)
        .skip(group_start + 1)
    {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        let Some((existing_key, _)) = trimmed.split_once('=').or_else(|| trimmed.split_once(':'))
        else {
            continue;
        };
        if existing_key.trim().eq_ignore_ascii_case(key) {
            key_index = Some(index);
            break;
        }
    }
    if let Some(index) = key_index {
        let prefix_len = lines[index].len() - lines[index].trim_start().len();
        let prefix = &lines[index][..prefix_len];
        lines[index] = format!("{prefix}{key}={value}");
    } else {
        lines.insert(group_start + 1, format!("{key}={value}"));
    }
    let mut output = lines.join("\n");
    output.push('\n');
    fs::write(path, output).expect("Failed to update KConfig value");
}

/// Map Android density to a whole-number UI scale factor (same baseline as the old LXQt setup).
#[allow(dead_code)]
fn android_ui_scale(density_dpi: i32) -> i32 {
    ((density_dpi as f32) / 160.0 * 1.1).max(1.0).round() as i32
}

/// Copy a Debian-owned startup file only when the guest has no user version.
/// `symlink_metadata` treats dangling links as user-owned state as well.
fn copy_guest_file_if_missing(source: &Path, destination: &Path) -> bool {
    if fs::symlink_metadata(destination).is_ok() {
        return true;
    }
    if !source.is_file() {
        log::error!("Debian startup source is unavailable: {}", source.display());
        return false;
    }
    let Some(parent) = destination.parent() else {
        return false;
    };
    if fs::create_dir_all(parent).is_err() {
        return false;
    }
    let temporary = parent.join(format!(
        ".{}.portal-default-{}",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file"),
        process::id()
    ));
    if fs::copy(source, &temporary).is_err() {
        let _ = fs::remove_file(&temporary);
        return false;
    }
    if fs::set_permissions(&temporary, fs::Permissions::from_mode(0o644)).is_err() {
        let _ = fs::remove_file(&temporary);
        return false;
    }
    if fs::symlink_metadata(destination).is_ok() {
        let _ = fs::remove_file(&temporary);
        return true;
    }
    if fs::rename(&temporary, destination).is_err() {
        let _ = fs::remove_file(&temporary);
        return false;
    }
    true
}

/// base-files normally creates these files from its maintainer script.  The
/// release image is assembled without executing ARM64 maintainer scripts, so
/// seed only missing files from Debian's packaged defaults and never replace
/// user shell configuration.
fn sync_base_files_defaults(fs_root: &Path, home_dir: &Path) -> bool {
    let mut complete = copy_guest_file_if_missing(
        &fs_root.join("usr/share/base-files/profile"),
        &fs_root.join("etc/profile"),
    );
    let skeleton = fs_root.join("etc/skel");
    let base_files = fs_root.join("usr/share/base-files");
    for home in [fs_root.join("root"), home_dir.to_path_buf()] {
        for (name, fallback) in [(".profile", "dot.profile"), (".bashrc", "dot.bashrc")] {
            let skeleton_source = skeleton.join(name);
            let source = if skeleton_source.is_file() {
                skeleton_source
            } else {
                base_files.join(fallback)
            };
            complete &= copy_guest_file_if_missing(&source, &home.join(name));
        }
    }
    complete
}

/// base-files normally owns these compatibility links.  Package extraction
/// skipped its maintainer script, so repair only an empty real directory. A
/// non-empty directory or a different file/link is user/package state and is
/// reported without deletion.
fn repair_base_files_runtime_links(fs_root: &Path) -> bool {
    if fs::create_dir_all(fs_root.join("run/lock")).is_err() {
        return false;
    }
    let mut complete = true;
    for (relative, target) in [("var/run", "/run"), ("var/lock", "/run/lock")] {
        let path = fs_root.join(relative);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                if symlink(target, &path).is_err() {
                    complete = false;
                }
                continue;
            }
            Err(_) => {
                complete = false;
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            if fs::read_link(&path).ok().as_deref() != Some(Path::new(target)) {
                log::error!(
                    "Refusing to replace non-standard guest link {}",
                    path.display()
                );
                complete = false;
            }
            continue;
        }
        if !metadata.is_dir() {
            log::error!("Refusing to replace guest path {}", path.display());
            complete = false;
            continue;
        }
        let empty = fs::read_dir(&path)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if !empty {
            log::error!(
                "Refusing to replace non-empty guest directory {}",
                path.display()
            );
            complete = false;
            continue;
        }
        let backup = path.with_extension(format!("portal-old-{}", process::id()));
        if fs::symlink_metadata(&backup).is_ok() || fs::rename(&path, &backup).is_err() {
            complete = false;
            continue;
        }
        if symlink(target, &path).is_err() {
            let _ = fs::rename(&backup, &path);
            complete = false;
        } else if fs::remove_dir(&backup).is_err() {
            complete = false;
        }
    }
    complete
}

const DPKG_INFO_MIGRATION_MARKER: &str = "var/lib/localdesktop/dpkg-info-multiarch-v1";
const LEGACY_PORTAL_CLEAN_APT: &str =
    "DPkg::Options { \"--force-confdef\"; \"--force-confold\"; };\n";

fn remove_legacy_portal_clean_apt(fs_root: &Path) {
    let path = fs_root.join("etc/apt/apt.conf.d/01portal-clean");
    if fs::read_to_string(&path).ok().as_deref() == Some(LEGACY_PORTAL_CLEAN_APT)
        && fs::remove_file(&path).is_err()
    {
        panic!("Cannot remove the obsolete Portal apt override");
    }
}

fn dpkg_multiarch_same_packages(status: &str) -> Vec<(String, String)> {
    let mut packages = Vec::new();
    for stanza in status.split("\n\n") {
        let mut package = None;
        let mut architecture = None;
        let mut multi_arch_same = false;
        for line in stanza.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key {
                "Package" => package = Some(value.to_string()),
                "Architecture" => architecture = Some(value.to_string()),
                "Multi-Arch" => multi_arch_same = value == "same",
                _ => {}
            }
        }
        let Some(package) = package else { continue };
        let Some(architecture) = architecture else {
            continue;
        };
        if multi_arch_same
            && !package.is_empty()
            && !package.contains('/')
            && !package.contains(':')
            && !architecture.is_empty()
            && architecture != "all"
            && !architecture.contains('/')
            && !architecture.contains(':')
        {
            packages.push((package, architecture));
        }
    }
    packages
}

/// Repair the package-info names emitted by old Portal images.  dpkg expects
/// every sidecar of Multi-Arch:same packages to use `pkg:arch.*`; moving the
/// unqualified files makes `dpkg --audit` and future maintainer scripts work.
/// Each rename stays within the guest filesystem and is atomic.  A qualified
/// destination is never overwritten: apt may already have upgraded that
/// package and created authoritative `pkg:arch.*` metadata.  In that case the
/// obsolete unqualified image metadata is retained in Portal-owned quarantine
/// rather than being allowed to confuse dpkg or being destroyed.
fn migrate_multiarch_dpkg_info(fs_root: &Path) -> bool {
    let marker = fs_root.join(DPKG_INFO_MIGRATION_MARKER);
    if fs::symlink_metadata(&marker).is_ok() {
        return true;
    }
    let status_path = fs_root.join("var/lib/dpkg/status");
    let info_dir = fs_root.join("var/lib/dpkg/info");
    let Ok(status) = fs::read_to_string(&status_path) else {
        log::error!(
            "Cannot migrate dpkg info: {} is unreadable",
            status_path.display()
        );
        return false;
    };
    if !info_dir.is_dir() {
        log::error!(
            "Cannot migrate dpkg info: {} is missing",
            info_dir.display()
        );
        return false;
    }
    let quarantine_dir = fs_root.join("var/lib/localdesktop/dpkg-info-unqualified-v1");
    if let Err(error) = fs::create_dir_all(&quarantine_dir) {
        log::error!(
            "Cannot create dpkg metadata quarantine {}: {}",
            quarantine_dir.display(),
            error
        );
        return false;
    }

    let mut complete = true;
    for (package, architecture) in dpkg_multiarch_same_packages(&status) {
        let unqualified_prefix = format!("{package}.");
        let qualified_prefix = format!("{package}:{architecture}.");
        let entries = match fs::read_dir(&info_dir) {
            Ok(entries) => entries,
            Err(error) => {
                log::error!("Cannot read {}: {}", info_dir.display(), error);
                return false;
            }
        };
        for entry in entries {
            let Ok(entry) = entry else {
                complete = false;
                continue;
            };
            let source = entry.path();
            let Some(name) = source.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.starts_with(&unqualified_prefix) {
                continue;
            }
            let suffix = &name[unqualified_prefix.len()..];
            if suffix.is_empty() {
                continue;
            }
            let destination = info_dir.join(format!("{qualified_prefix}{suffix}"));
            if fs::symlink_metadata(&destination).is_ok() {
                let quarantine = quarantine_dir.join(name);
                if fs::symlink_metadata(&quarantine).is_ok() {
                    let same = match (fs::read(&source), fs::read(&quarantine)) {
                        (Ok(source_bytes), Ok(quarantine_bytes)) => {
                            source_bytes == quarantine_bytes
                        }
                        _ => false,
                    };
                    if same {
                        if let Err(error) = fs::remove_file(&source) {
                            log::error!(
                                "Cannot remove already-quarantined dpkg metadata {}: {}",
                                source.display(),
                                error
                            );
                            complete = false;
                        }
                    } else {
                        log::error!(
                            "Refusing conflicting dpkg metadata quarantine {} -> {}",
                            source.display(),
                            quarantine.display()
                        );
                        complete = false;
                    }
                } else if let Err(error) = fs::rename(&source, &quarantine) {
                    log::error!(
                        "Cannot quarantine obsolete dpkg metadata {} -> {}: {}",
                        source.display(),
                        quarantine.display(),
                        error
                    );
                    complete = false;
                }
                continue;
            }
            // No apt/dpkg process is started during this setup pass, so the
            // checked-absent destination cannot be a normal concurrent writer.
            // Rename preserves metadata and is atomic on the same filesystem.
            if let Err(error) = fs::rename(&source, &destination) {
                log::error!(
                    "Cannot migrate dpkg metadata {} -> {}: {}",
                    source.display(),
                    destination.display(),
                    error
                );
                complete = false;
            }
        }
    }
    if !complete {
        return false;
    }
    let Some(parent) = marker.parent() else {
        return false;
    };
    if fs::create_dir_all(parent).is_err() {
        return false;
    }
    let temporary = parent.join(format!(".dpkg-info-multiarch-v1-{}", process::id()));
    if fs::write(&temporary, b"completed\n").is_err() {
        let _ = fs::remove_file(&temporary);
        return false;
    }
    if let Err(error) = fs::rename(&temporary, &marker) {
        log::error!("Cannot commit dpkg metadata migration marker: {}", error);
        let _ = fs::remove_file(&temporary);
        return false;
    }
    true
}

/// Run Debian's own PAM generator only when no common stack exists.  If a
/// user has already supplied any common stack file, do not let the package
/// helper rewrite it; an incomplete custom PAM setup is reported instead.
fn sync_pam_defaults(fs_root: &Path) -> bool {
    let common = [
        "etc/pam.d/common-auth",
        "etc/pam.d/common-account",
        "etc/pam.d/common-session",
        "etc/pam.d/common-session-noninteractive",
        "etc/pam.d/common-password",
    ];
    let present = common
        .iter()
        .filter(|path| fs::symlink_metadata(fs_root.join(path)).is_ok())
        .count();
    if present == common.len() {
        return true;
    }
    if present != 0 {
        log::warn!(
            "Guest PAM common stack is incomplete; preserving existing files and refusing to overwrite it"
        );
        return false;
    }
    if !fs_root.join("usr/sbin/pam-auth-update").is_file() {
        log::warn!("Guest libpam-runtime is missing pam-auth-update; PAM defaults unavailable");
        return false;
    }
    let output = ArchProcess {
        command: "DEBIAN_FRONTEND=noninteractive /usr/sbin/pam-auth-update --package --force"
            .into(),
        user: None,
        log: None,
    }
    .run();
    let complete = output.status.success()
        && common
            .iter()
            .all(|path| fs::symlink_metadata(fs_root.join(path)).is_ok());
    if !complete {
        log::error!(
            "Debian PAM configuration failed (status {:?}); package authentication may be unavailable",
            output.status.code()
        );
    }
    complete
}

/// Ensure guest package-management files are durable across slot switches and clean installs.
fn sync_debian_package_management(fs_root: &Path) {
    remove_legacy_portal_clean_apt(fs_root);
    if !repair_base_files_runtime_links(fs_root) {
        panic!("Debian /var/run and /var/lock integration is unsafe; refusing package operations");
    }
    if !migrate_multiarch_dpkg_info(fs_root) {
        panic!("Debian package metadata migration did not complete; refusing package operations");
    }
    if !sync_pam_defaults(fs_root) {
        panic!("Debian PAM configuration did not complete; refusing package operations");
    }
    let apt_conf_d = fs_root.join("etc/apt/apt.conf.d");
    fs::create_dir_all(&apt_conf_d).expect("Failed to create guest apt configuration directory");
    let no_sandbox_path = apt_conf_d.join("01no-sandbox");
    if !no_sandbox_path.exists() {
        fs::write(&no_sandbox_path, "APT::Sandbox::User \"root\";\n")
            .expect("Failed to install the PRoot apt sandbox policy");
    }
    let sbin_dir = fs_root.join("usr/sbin");
    fs::create_dir_all(&sbin_dir).expect("Failed to create guest sbin directory");
    let policy_rc_d = sbin_dir.join("policy-rc.d");
    if !policy_rc_d.exists() {
        fs::write(&policy_rc_d, "#!/bin/sh\nexit 101\n")
            .expect("Failed to install the PRoot service-start policy");
        fs::set_permissions(&policy_rc_d, fs::Permissions::from_mode(0o755))
            .expect("Failed to mark the PRoot service-start policy executable");
    }

    let dpkg_dir = fs_root.join("var/lib/dpkg");
    fs::create_dir_all(&dpkg_dir).expect("Failed to create guest dpkg directory");
    let arch_path = dpkg_dir.join("arch");
    if !arch_path.exists() {
        fs::write(&arch_path, "arm64\n").expect("Failed to seed the guest dpkg architecture");
    }

    let dpkg_info_dir = fs_root.join("var/lib/dpkg/info");
    fs::create_dir_all(&dpkg_info_dir).expect("Failed to create guest dpkg info directory");
    let format_path = dpkg_info_dir.join("format");
    if !format_path.exists() {
        fs::write(&format_path, "1\n").expect("Failed to seed the guest dpkg info format");
    }

    let sources_list = fs_root.join("etc/apt/sources.list");
    if !sources_list.exists() {
        if let Some(parent) = sources_list.parent() {
            fs::create_dir_all(parent).expect("Failed to create guest apt directory");
        }
        fs::write(
            &sources_list,
            "deb http://deb.debian.org/debian trixie main\n\
             deb http://deb.debian.org/debian trixie-updates main\n\
             deb http://security.debian.org/debian-security trixie-security main\n",
        )
        .expect("Failed to seed Debian package sources");
    }
}

fn retire_legacy_desktop_mutators(fs_root: &Path) -> anyhow::Result<()> {
    let marker = fs_root.join("var/lib/localdesktop/desktop-ownership-v1");
    if marker.is_file() {
        return Ok(());
    }
    let username = get_application_context().local_config.user.username;
    let home_dir = chroot_home_dir(fs_root, &username);
    let legacy_autostart = "[Desktop Entry]\nType=Application\nName=Portal Session Integration\nExec=/usr/local/bin/localdesktop-no-sandbox-entries\nOnlyShowIn=KDE;\nX-KDE-autostart-after=panel\n";
    for (path, contents) in [
        (home_dir.join(".config/autostart/localdesktop-session-init.desktop"), legacy_autostart),
        // Completed root-era profiles are imported after this cleanup. Never
        // carry their old cold-start desktop rewriter into the new login.
        (fs_root.join("root/.config/autostart/localdesktop-session-init.desktop"), legacy_autostart),
        (
            fs_root.join("etc/apt/apt.conf.d/99portal-desktop-integration"),
            "DPkg::Post-Invoke { \"if [ -x /usr/local/bin/localdesktop-no-sandbox-entries ]; then HOME=/root XDG_DATA_HOME=/root/.local/share /usr/local/bin/localdesktop-no-sandbox-entries >>/var/lib/localdesktop/desktop-integration.log 2>&1 || printf '%s\\n' 'Portal desktop integration refresh failed' >>/var/lib/localdesktop/desktop-integration.log; fi\"; };\n",
        ),
    ] {
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() => {
                if fs::read(&path)? == contents.as_bytes() {
                    fs::remove_file(&path)?;
                }
            }
            Ok(_) => {} // A user replacement is not Portal's to remove.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if let Some(parent) = marker.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(marker, b"legacy auto-rewriters retired\n")?;
    Ok(())
}

fn sync_initial_desktop_defaults(fs_root: &Path) {
    let username = get_application_context().local_config.user.username;
    let home_dir = chroot_home_dir(fs_root, &username);

    sync_android_timezone(fs_root);
    // Small, verified Debian tools needed for triggers skipped by image extraction.
    // Embedded in the APK so the published base image also works after uninstall.
    tar::Archive::new(std::io::Cursor::new(include_bytes!(
        "../../../assets/daily-use-tools.tar"
    )))
    .unpack(fs_root)
    .expect("Failed to install desktop cache tools");
    sync_firefox_config(fs_root);
    // Repair Portal's own launcher to use Debian's installed browser/icon name.
    let docs_entry = home_dir.join("Desktop/localdesktop-online-docs.desktop");
    if fs_root.join("usr/bin/firefox-esr").exists() {
        if let Ok(text) = fs::read_to_string(&docs_entry) {
            let text = text
                .replace("Exec=firefox ", "Exec=firefox-esr ")
                .replace("Icon=firefox\n", "Icon=firefox-esr\n");
            let _ = fs::write(&docs_entry, text);
            let _ = fs::set_permissions(&docs_entry, fs::Permissions::from_mode(0o755));
        }
    }
    if !sync_base_files_defaults(fs_root, &home_dir) {
        panic!("Debian shell startup defaults could not be repaired; refusing guest launch");
    }
    sync_debian_package_management(fs_root);

    // Configure the stock defaults before the first panel exists. Editing only
    // desktop-appletsrc cannot repair a clean install's initial default value.
    for (relative, old, new) in [
        (
            "usr/share/plasma/plasmoids/org.kde.plasma.taskmanager/contents/config/main.xml",
            "applications:org.kde.discover.desktop,",
            "",
        ),
        (
            "usr/share/plasma/plasmoids/org.kde.plasma.kickoff/contents/config/main.xml",
            "org.kde.kontact.desktop,",
            "",
        ),
        (
            "usr/share/plasma/plasmoids/org.kde.plasma.kickoff/contents/config/main.xml",
            ",org.kde.discover.desktop",
            "",
        ),
    ] {
        let path = fs_root.join(relative);
        if let Ok(text) = fs::read_to_string(&path) {
            if text.contains(old) {
                let _ = fs::write(path, text.replace(old, new));
            }
        }
    }

    // Disable screen locking completely: Android/OxygenOS owns device security.
    let config_dir = home_dir.join(".config");
    let _ = fs::create_dir_all(&config_dir);
    let kdeglobals = config_dir.join("kdeglobals");
    upsert_kconfig_value(
        &kdeglobals,
        "KDE Action Restrictions][$i",
        "action/lock_screen",
        "false",
    );

    // These KCMs configure Linux-owned hardware/services that do not exist in
    // a nested Android PRoot session. Keep the KWin touchscreen-gestures KCM
    // and Portal's narrowly supported touchpad KCM visible.
    for desktop_file in [
        "kcm_clock.desktop",
        "kcm_tablet.desktop",
        "kcm_mouse.desktop",
    ] {
        let path = fs_root.join("usr/share/applications").join(desktop_file);
        if path.is_file() {
            upsert_kv_file(
                &path,
                '=',
                &[
                    ("Hidden", "true".to_string()),
                    ("NoDisplay", "true".to_string()),
                ],
            );
        }
    }
    for plugin in [
        "usr/lib/aarch64-linux-gnu/qt6/plugins/plasma/kcms/systemsettings/kcm_touchscreen.so",
        "usr/lib/aarch64-linux-gnu/qt6/plugins/plasma/kcms/systemsettings/kcm_tablet.so",
        "usr/lib/aarch64-linux-gnu/qt6/plugins/plasma/kcms/systemsettings/kcm_mouse.so",
        "usr/lib/aarch64-linux-gnu/qt6/plugins/plasma/kcms/systemsettings_qwidgets/kcm_clock.so",
    ] {
        let source = fs_root.join(plugin);
        if source.is_file() {
            let disabled = source.with_extension("so.portal-disabled");
            fs::rename(&source, &disabled)
                .expect("Failed to disable an unsupported Plasma settings module");
        }
    }

    let touchpad_desktop = fs_root.join("usr/share/applications/kcm_touchpad.desktop");
    if touchpad_desktop.is_file() {
        upsert_kv_file(
            &touchpad_desktop,
            '=',
            &[
                ("Hidden", "false".to_string()),
                ("NoDisplay", "false".to_string()),
            ],
        );
    }
    let touchpad_plugin = fs_root
        .join("usr/lib/aarch64-linux-gnu/qt6/plugins/plasma/kcms/systemsettings/kcm_touchpad.so");
    let disabled_touchpad_plugin = touchpad_plugin.with_extension("so.portal-disabled");
    if !touchpad_plugin.exists() && disabled_touchpad_plugin.is_file() {
        fs::rename(&disabled_touchpad_plugin, &touchpad_plugin)
            .expect("Failed to restore Portal touchpad settings module");
    }

    // Plasma's stock panel pins Discover even when this deliberately minimal
    // image has no package-management backend. Migrate that one dead launcher
    // to Dolphin once, then leave subsequent user panel customisation alone.
    let panel_marker = home_dir.join(".local/state/portal/panel-launchers-v2");
    if !panel_marker.exists()
        && config_dir
            .join("plasma-org.kde.plasma.desktop-appletsrc")
            .is_file()
    {
        let appletsrc = config_dir.join("plasma-org.kde.plasma.desktop-appletsrc");
        if let Ok(content) = fs::read_to_string(&appletsrc) {
            let discover_available = fs_root.join("usr/bin/plasma-discover").is_file();
            let dolphin_available = fs_root.join("usr/bin/dolphin").is_file();
            let mut changed = false;
            let mut lines = Vec::new();
            for line in content.lines() {
                if let Some(value) = line.strip_prefix("launchers=") {
                    let mut launchers: Vec<&str> = value
                        .split(',')
                        .filter(|entry| {
                            discover_available || *entry != "applications:org.kde.discover.desktop"
                        })
                        .collect();
                    if dolphin_available
                        && !launchers.contains(&"applications:org.kde.dolphin.desktop")
                        && !launchers.contains(&"preferred://filemanager")
                    {
                        let at = launchers
                            .iter()
                            .position(|entry| entry.starts_with("preferred://"))
                            .unwrap_or(launchers.len());
                        launchers.insert(at, "applications:org.kde.dolphin.desktop");
                    }
                    let replacement = format!("launchers={}", launchers.join(","));
                    changed |= replacement != line;
                    lines.push(replacement);
                } else {
                    lines.push(line.to_string());
                }
            }
            if changed {
                fs::write(&appletsrc, format!("{}\n", lines.join("\n")))
                    .expect("Failed to repair the default panel launchers");
            }
        }
        if let Some(parent) = panel_marker.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(panel_marker, "migrated\n");
    }
    let kscreenlockerrc = config_dir.join("kscreenlockerrc");
    upsert_kconfig_value(&kscreenlockerrc, "Daemon][$i", "Autolock", "false");
    upsert_kconfig_value(&kscreenlockerrc, "Daemon][$i", "LockOnResume", "false");
    upsert_kconfig_value(&kscreenlockerrc, "Daemon][$i", "Timeout", "0");
}

pub fn sync_session_runtime_files(fs_root: &Path, ui_scale: i32) {
    sync_guest_session_directories(fs_root).expect("Failed to create guest session directories");
    // A committed desktop is user-owned. Only the still-uncommitted first-run
    // image may receive defaults, panel migration, or home/config repairs.
    if crate::core::provisioning::RuntimeArtifact::production().is_bootable(fs_root) {
        retire_legacy_desktop_mutators(fs_root)
            .expect("Failed to retire Portal's legacy desktop auto-rewriters");
    } else {
        sync_initial_desktop_defaults(fs_root);
    }
    sync_portal_runtime_assets(fs_root, ui_scale);

    sync_guest_network_config(fs_root);
}

/// Run Portal-owned session support behind the same recoverable boundary used
/// by first setup. User desktop defaults are applied only before commitment.
pub fn try_sync_session_runtime_files(fs_root: &Path, ui_scale: i32) -> anyhow::Result<()> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        sync_session_runtime_files(fs_root, ui_scale);
    }))
    .map_err(|payload| {
        anyhow::anyhow!(
            "guest session integration failed: {}",
            panic_text(payload.as_ref())
        )
    })
    .and_then(|()| validate_required_session_files(fs_root))
}

/// The only privileged boundary for the graphical login.  The helper owns
/// account validation, one-time non-destructive root-profile import, and
/// private runtime/session directories; Plasma itself is never run as root.
pub fn prepare_desktop_login(migrate_legacy: bool) -> anyhow::Result<()> {
    ensure_desktop_services(Path::new(PRODUCTION_FS_ROOT))?;
    let caches = PRootRuntime::active().execute(
        ProcessSpec::new("/usr/local/bin/localdesktop-system-caches"),
        None,
        None,
    );
    anyhow::ensure!(
        caches.status.success(),
        "privileged Debian desktop cache preparation failed: {}",
        String::from_utf8_lossy(&caches.stderr)
    );
    let mode = if migrate_legacy { "--migrate" } else { "--prepare" };
    let command = format!("/usr/local/bin/localdesktop-prepare-login {mode}");
    let output = PRootRuntime::active().execute(ProcessSpec::new(command), None, None);
    anyhow::ensure!(
        output.status.success(),
        "desktop account preparation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let validation = PRootRuntime::active().execute(
        ProcessSpec::new(
            "test \"$(id -u)\" = 1000 && test \"$(id -g)\" = 1000 && test \"$HOME\" = /home/desktop && test \"$(stat -c '%u:%g:%a' /run/user/1000)\" = 1000:1000:700 && test \"$(stat -c '%u:%g:%a' /var/lib/localdesktop/session)\" = 1000:1000:700"
        )
        .with_user(DESKTOP_USER),
        None,
        None,
    );
    anyhow::ensure!(
        validation.status.success(),
        "the PRoot desktop login did not validate as UID/GID 1000 with a private runtime: {}",
        String::from_utf8_lossy(&validation.stderr)
    );
    Ok(())
}

const DESKTOP_SERVICE_PACKAGES: &[&str] = &[
    "xdg-desktop-portal",
    "xdg-desktop-portal-kde",
    "xdg-desktop-portal-gtk",
    "xdg-utils",
];

/// Install the freedesktop/KDE session integration once at the privileged
/// boundary. A later user removal is respected: the marker prevents cold
/// launches from reinstalling packages or rewriting desktop preferences.
fn ensure_desktop_services(root: &Path) -> anyhow::Result<()> {
    let marker = root.join("var/lib/localdesktop/desktop-services-v1");
    if marker.is_file() {
        return Ok(());
    }
    let installed = installed_dpkg_packages(root)?;
    if DESKTOP_SERVICE_PACKAGES
        .iter()
        .any(|package| !installed.contains(*package))
    {
        if !root.join(DPKG_INFO_MIGRATION_MARKER).is_file() {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sync_debian_package_management(root)
            }))
            .map_err(|payload| {
                anyhow::anyhow!(
                    "existing Debian package state could not be prepared for desktop services: {}",
                    panic_text(payload.as_ref())
                )
            })?;
        }
        validate_optional_app_apt_setup(root)?;
        let command = "dpkg --configure -a && apt-get update && apt-get install -y --no-remove --no-install-recommends --fix-broken && apt-get install -y --no-remove --no-install-recommends xdg-desktop-portal xdg-desktop-portal-kde xdg-desktop-portal-gtk xdg-utils";
        let output = PRootRuntime::active().execute(
            ProcessSpec::new(command).with_env("DEBIAN_FRONTEND", "noninteractive"),
            None,
            None,
        );
        anyhow::ensure!(
            output.status.success(),
            "Debian portal/desktop service installation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let installed = installed_dpkg_packages(root)?;
    anyhow::ensure!(
        DESKTOP_SERVICE_PACKAGES
            .iter()
            .all(|package| installed.contains(*package)),
        "Debian desktop portal services are not fully installed"
    );
    let temporary = marker.with_extension("tmp");
    fs::write(&temporary, b"installed\n")?;
    fs::File::open(&temporary)?.sync_all()?;
    fs::rename(&temporary, &marker)?;
    fs::File::open(marker.parent().context("Missing desktop service marker parent")?)?
        .sync_all()?;
    Ok(())
}

fn setup_desktop_login(_: &SetupOptions) -> StageOutput {
    Some(thread::spawn(|| prepare_desktop_login(false)))
}

fn validate_required_session_files(fs_root: &Path) -> anyhow::Result<()> {
    for relative in [
        "tmp/.X11-unix",
        "tmp/.ICE-unix",
        "var/tmp",
        "usr/local/bin/startplasma-localdesktop",
        "usr/local/bin/localdesktop-plasmashell-supervisor",
        "etc/xdg/autostart/org.kde.plasmashell.desktop",
        "usr/local/bin/kwin_wayland",
        "usr/local/bin/start-localdesktop-recovery",
        "usr/local/bin/localdesktop-retry-plasma",
        "usr/local/bin/portal-ime-bridge",
        "usr/local/bin/wl-copy",
        "usr/local/bin/wl-paste",
        PIPEWIRE_CLIENT_NO_RT_PATHS[0],
        PIPEWIRE_CLIENT_NO_RT_PATHS[1],
    ] {
        anyhow::ensure!(
            fs_root.join(relative).is_file() || fs_root.join(relative).is_dir(),
            "Required guest setup path is missing: {relative}"
        );
    }

    let kwin_dir = fs_root.join("usr/local/lib/portal");
    anyhow::ensure!(
        fs::metadata(kwin_dir.join("libkwin.so.6.3.6"))
            .map(|metadata| metadata.len() == KWIN_LIBRARY.len() as u64)
            .unwrap_or(false),
        "Required Portal KWin overlay is incomplete"
    );
    for (link, target) in [
        ("libkwin.so.6", "libkwin.so.6.3.6"),
        ("libkwin.so", "libkwin.so.6"),
    ] {
        anyhow::ensure!(
            fs::read_link(kwin_dir.join(link)).ok().as_deref() == Some(Path::new(target)),
            "Required Portal KWin symlink is incomplete: {link}"
        );
    }

    if crate::android::anland::is_anland_requested() {
        let anland_dir = fs_root.join("usr/local/lib/portal-anland");
        anyhow::ensure!(
            fs::metadata(anland_dir.join("libkwin.so.6.3.6"))
                .map(|metadata| metadata.len() == KWIN_ANLAND_LIBRARY.len() as u64)
                .unwrap_or(false),
            "Required Anland KWin library is incomplete"
        );
        for (link, target) in [
            ("libkwin.so.6", "libkwin.so.6.3.6"),
            ("libkwin.so", "libkwin.so.6"),
        ] {
            anyhow::ensure!(
                fs::read_link(anland_dir.join(link)).ok().as_deref()
                    == Some(Path::new(target)),
                "Required Anland KWin symlink is incomplete: {link}"
            );
        }
    }
    Ok(())
}

fn validate_anland_repair_state(fs_root: &Path) -> anyhow::Result<()> {
    let artifact = crate::core::provisioning::RuntimeArtifact::production();
    let classification = artifact.classify_runtime(fs_root);
    anyhow::ensure!(
        classification.is_trusted_recovery(),
        "Portal runtime lost its completion marker during Anland repair"
    );
    anyhow::ensure!(
        matches!(
            crate::android::anland::active_renderer(),
            crate::android::anland::RendererKind::Anland
        ),
        "Anland renderer selection is not active after repair"
    );
    anyhow::ensure!(
        super::mesa_layer::is_provisioned(),
        "Mesa KGSL layer is not provisioned after Anland repair"
    );
    crate::android::anland::validate_launch_contract()?;
    validate_required_session_files(fs_root)?;
    validate_firefox_anland_config(fs_root)?;

    let anland_library = fs_root.join("usr/local/lib/portal-anland/libkwin.so.6.3.6");
    anyhow::ensure!(
        fs::read(&anland_library)
            .map(|bytes| bytes == KWIN_ANLAND_LIBRARY)
            .unwrap_or(false),
        "Anland KWin overlay does not match the pinned Portal asset"
    );
    let drmshim = fs_root.join("usr/local/lib/portal/drmshim.so");
    anyhow::ensure!(
        fs::read(&drmshim)
            .map(|bytes| bytes == DRMSHIM_BINARY)
            .unwrap_or(false),
        "Portal drmshim is missing or corrupt"
    );
    for relative in [
        "usr/local/lib/localdesktop-crash-handler.so",
        "usr/local/bin/portal-ibus-engine",
        "usr/local/bin/portal-ibus-lazy",
        "usr/share/ibus/component/portal.xml",
        "usr/share/applications/portal-ime.desktop",
    ] {
        anyhow::ensure!(
            fs_root.join(relative).is_file(),
            "Required Anland session integration is missing: {relative}"
        );
    }
    Ok(())
}

/// Install Portal's ABI-matched Debian KWin overlay and migrate the
/// pre-Anland layout. Runs on every provisioning pass AND every session
/// launch (via `sync_session_runtime_files`), so existing runtimes converge
/// without re-provisioning.
///
/// Project Anland layout: the overlay lives in `/usr/local/lib/portal` (NOT
/// `/usr/local/lib`) so it can never shadow the distro libkwin through the
/// default loader path. The kwin wrapper adds the portal dir to
/// LD_LIBRARY_PATH only for QPainter sessions; Anland sessions resolve the
/// unified Anland libkwin from `/usr/local/lib/portal-anland`
/// (see `sync_kwin_anland_overlay`).
fn sync_kwin_overlay(fs_root: &Path) {
    let kwin_dir = fs_root.join("usr/local/lib/portal");
    let _ = fs::create_dir_all(&kwin_dir);
    let kwin_library = kwin_dir.join("libkwin.so.6.3.6");
    let fresh = fs::metadata(&kwin_library)
        .map(|m| m.len() == KWIN_LIBRARY.len() as u64)
        .unwrap_or(false);
    if !fresh {
        let kwin_temporary = kwin_library.with_extension("6.3.6.tmp");
        if fs::write(&kwin_temporary, KWIN_LIBRARY).is_ok() {
            let _ = fs::set_permissions(&kwin_temporary, fs::Permissions::from_mode(0o755));
            let _ = fs::rename(&kwin_temporary, &kwin_library);
        }
    }
    // Repair the soname chain even when the binary itself is already the
    // expected size. A process death after the binary rename must not leave a
    // same-size library with missing links and a permanently failed retry.
    for (link, target) in [
        ("libkwin.so.6", "libkwin.so.6.3.6"),
        ("libkwin.so", "libkwin.so.6"),
    ] {
        let path = kwin_dir.join(link);
        let tmp = kwin_dir.join(format!("{link}.tmp"));
        let _ = fs::remove_file(&tmp);
        if symlink(target, &tmp).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
    // Migration: remove pre-Anland overlay links that shadowed libkwin.so.6
    // from the default loader path. Only our overlay ever lived at these
    // paths (the distro libkwin lives under /usr/lib).
    for legacy in ["libkwin.so.6.3.6", "libkwin.so.6", "libkwin.so"] {
        let path = fs_root.join("usr/local/lib").join(legacy);
        if path.is_symlink() || path.is_file() {
            let _ = fs::remove_file(&path);
        }
    }
    // Project Anland load-time stub for QPainter sessions (see ANLAND_STUB_BINARY).
    let stub_path = kwin_dir.join("libanland-stub.so");
    let stub_fresh = fs::metadata(&stub_path)
        .map(|m| m.len() == ANLAND_STUB_BINARY.len() as u64)
        .unwrap_or(false);
    if !stub_fresh {
        let stub_tmp = stub_path.with_extension("so.tmp");
        if fs::write(&stub_tmp, ANLAND_STUB_BINARY).is_ok() {
            let _ = fs::set_permissions(&stub_tmp, fs::Permissions::from_mode(0o755));
            let _ = fs::rename(&stub_tmp, &stub_path);
        }
    }
    // Project Anland DRM shim for Anland sessions (see DRMSHIM_BINARY).
    let shim_path = kwin_dir.join("drmshim.so");
    let shim_fresh = fs::metadata(&shim_path)
        .map(|m| m.len() == DRMSHIM_BINARY.len() as u64)
        .unwrap_or(false);
    if !shim_fresh {
        let shim_tmp = shim_path.with_extension("so.tmp");
        if fs::write(&shim_tmp, DRMSHIM_BINARY).is_ok() {
            let _ = fs::set_permissions(&shim_tmp, fs::Permissions::from_mode(0o755));
            let _ = fs::rename(&shim_tmp, &shim_path);
        }
    }
    // Project Anland unified KWin library (Anland backend + Portal Touchpad).
    // Served from its own dir so the QPainter overlay above is never shadowed.
    sync_kwin_anland_overlay(fs_root);
    // Phase B XWayland touchpad-source candidate (own dir, stock default).
    sync_xwayland_candidate_overlay(fs_root);
}

/// Install Portal's Anland-unified KWin library for GPU sessions. Runs on
/// every provisioning pass AND every session launch (via
/// `sync_session_runtime_files`), so existing runtimes converge without
/// re-provisioning and the setting survives restarts and reinstalls.
///
/// Layout: `/usr/local/lib/portal-anland` (NOT `/usr/local/lib/portal`, NOT
/// `/usr/lib`) so neither the QPainter overlay nor the distro libkwin is
/// shadowed. The kwin wrapper puts this dir first on LD_LIBRARY_PATH only
/// for Anland sessions; the AnlandBackend symbol resolves from the unified
/// lib, so the load-time stub must never be preloaded there.
fn sync_kwin_anland_overlay(fs_root: &Path) {
    if let Err(error) = sync_kwin_anland_overlay_inner(fs_root, false) {
        log::warn!("Could not refresh Anland KWin overlay: {error:#}");
    }
}

/// Strict variant used by the explicit migration. Normal launch only needs a
/// cheap size check, but an intentional repair must replace same-size
/// corruption instead of merely reporting it during final validation.
fn sync_kwin_anland_overlay_for_repair(fs_root: &Path) -> anyhow::Result<()> {
    sync_kwin_anland_overlay_inner(fs_root, true)
}

fn sync_kwin_anland_overlay_inner(fs_root: &Path, verify_bytes: bool) -> anyhow::Result<()> {
    let kwin_dir = fs_root.join("usr/local/lib/portal-anland");
    fs::create_dir_all(&kwin_dir)?;
    let kwin_library = kwin_dir.join("libkwin.so.6.3.6");
    let fresh = if verify_bytes {
        fs::read(&kwin_library)
            .map(|bytes| bytes == KWIN_ANLAND_LIBRARY)
            .unwrap_or(false)
    } else {
        fs::metadata(&kwin_library)
            .map(|m| m.len() == KWIN_ANLAND_LIBRARY.len() as u64)
            .unwrap_or(false)
    };
    if !fresh {
        write_guest_binary_result(&kwin_library, KWIN_ANLAND_LIBRARY)?;
    }
    // Atomic symlink swap (temp + rename): KWin must never observe a
    // half-deployed soname chain if a launch races a previous update. This
    // also repairs links after a process death following the library rename.
    for (link, target) in [
        ("libkwin.so.6", "libkwin.so.6.3.6"),
        ("libkwin.so", "libkwin.so.6"),
    ] {
        let path = kwin_dir.join(link);
        let tmp = kwin_dir.join(format!("{link}.tmp"));
        let link_is_valid = fs::read_link(&path).ok().as_deref() == Some(Path::new(target));
        if !link_is_valid {
            match fs::remove_file(&tmp) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            symlink(target, &tmp)?;
            fs::rename(&tmp, &path)?;
            if let Some(parent) = path.parent() {
                crate::core::mesa_layer::sync_dir_best_effort(parent);
            }
        }
    }
    Ok(())
}

/// Stage the Phase B XWayland touchpad-source candidate plus the xinput
/// diagnostic into `/usr/local/lib/portal-xwayland` (NOT `/usr/bin`, NOT
/// the default loader path). Runs on every provisioning pass AND every
/// session launch, so existing runtimes converge without re-provisioning.
///
/// Staging is inert by itself: the KWin wrapper only puts this dir on PATH
/// when the session explicitly selects `xwayland-variant=candidate`, and it
/// SHA-validates the binary there first (falling back to stock otherwise).
/// Sync is idempotent (size-freshness, atomic temp+rename, 0755) mirroring
/// the KWin overlay above.
fn sync_xwayland_candidate_overlay(fs_root: &Path) {
    let candidate_dir = fs_root.join("usr/local/lib/portal-xwayland");
    let _ = fs::create_dir_all(&candidate_dir);
    for (name, payload) in [
        ("Xwayland", XWAYLAND_CANDIDATE_BINARY),
        ("xinput", XWAYLAND_XINPUT_BINARY),
    ] {
        let target = candidate_dir.join(name);
        let fresh = fs::metadata(&target)
            .map(|m| m.len() == payload.len() as u64)
            .unwrap_or(false);
        if !fresh {
            let tmp = candidate_dir.join(format!("{name}.tmp"));
            if fs::write(&tmp, payload).is_ok() {
                let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755));
                let _ = fs::rename(&tmp, &target);
            }
        }
    }
}

/// Select Portal's profile once for runtimes that previously defaulted to the
/// extracted Debian `Profile 1.profile`.  After this migration the user's
/// selected profile is authoritative; later launches never rewrite `konsolerc`
/// or any profile launch keys.
fn migrate_konsole_profile(home_dir: &Path, guest_home: &str) {
    let config_dir = home_dir.join(".config");
    let config_path = config_dir.join("konsolerc");
    let profile_dir = home_dir.join(".local/share/konsole");
    let profile_path = profile_dir.join("LocalDesktop.profile");
    let marker = home_dir.join(".local/state/portal/konsole-profile-v2");

    write_default_file(&config_path, KONSOLE_CONFIG);
    write_default_file(
        &profile_path,
        &KONSOLE_PROFILE.replace("@HOME@", guest_home),
    );

    if marker.exists() {
        return;
    }

    let content = fs::read_to_string(&config_path).expect("Failed to read Konsole configuration");
    let mut changed = false;
    let mut lines = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        let is_legacy_default = trimmed
            .split_once('=')
            .map(|(key, value)| {
                key.trim().eq_ignore_ascii_case("DefaultProfile")
                    && value.trim() == "Profile 1.profile"
            })
            .unwrap_or(false);
        if is_legacy_default {
            let prefix_len = line.len() - trimmed.len();
            lines.push(format!(
                "{}DefaultProfile=LocalDesktop.profile",
                &line[..prefix_len]
            ));
            changed = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if changed {
        let mut migrated = lines.join("\n");
        migrated.push('\n');
        fs::write(&config_path, migrated).expect("Failed to migrate Konsole default profile");
    }

    if let Some(parent) = marker.parent() {
        fs::create_dir_all(parent).expect("Failed to create Konsole migration state directory");
    }
    fs::write(marker, "version=2\n").expect("Failed to record Konsole profile migration");
}

fn sync_android_timezone(fs_root: &Path) {
    let Some(zone_id) = get_application_context().get_timezone_id() else {
        log::warn!("Android timezone was unavailable; retaining the guest timezone");
        return;
    };
    let relative = Path::new(&zone_id);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        log::warn!("Ignoring invalid Android timezone identifier: {zone_id:?}");
        return;
    }
    let zoneinfo = fs_root.join("usr/share/zoneinfo").join(relative);
    if !zoneinfo.is_file() {
        log::warn!("Android timezone is not present in the guest zoneinfo database: {zone_id}");
        return;
    }

    let etc = fs_root.join("etc");
    fs::create_dir_all(&etc).expect("Failed to create guest /etc for timezone sync");
    let localtime = etc.join("localtime");
    match fs::symlink_metadata(&localtime) {
        Ok(metadata) if metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            fs::remove_file(&localtime).expect("Failed to replace guest /etc/localtime");
        }
        Ok(_) => {
            log::warn!("Guest /etc/localtime is not a file; leaving it unchanged");
            return;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            log::warn!("Failed to inspect guest /etc/localtime: {error}");
            return;
        }
    }
    symlink(format!("/usr/share/zoneinfo/{zone_id}"), &localtime)
        .expect("Failed to link guest timezone to Android's zone");
    fs::write(etc.join("timezone"), format!("{zone_id}\n"))
        .expect("Failed to write guest /etc/timezone");
    log::info!("Synchronized guest timezone from Android: {zone_id}");
}

/// Keep guest network configuration (DNS resolver, NSS, hosts, SSL CA certificates)
/// synchronized with Android system state and distro requirements.
pub fn sync_guest_network_config(fs_root: &Path) {
    let context = get_application_context();
    let mut dns_servers = context.get_active_dns_servers();
    if dns_servers.is_empty() {
        dns_servers = vec!["8.8.8.8".to_string(), "1.1.1.1".to_string()];
    }

    let mut resolv_content =
        String::from("# Generated by Portal from Android ConnectivityManager\n");
    for srv in &dns_servers {
        resolv_content.push_str(&format!("nameserver {srv}\n"));
    }

    let etc_dir = fs_root.join("etc");
    let _ = fs::create_dir_all(&etc_dir);

    let resolv_conf = etc_dir.join("resolv.conf");
    let current_content = fs::read_to_string(&resolv_conf).unwrap_or_default();
    // Follow Android network changes only while this file is still Portal-owned.
    // A manually replaced resolver configuration survives the next launch.
    if (current_content.is_empty()
        || current_content.starts_with("# Generated by Portal from Android ConnectivityManager\n"))
        && current_content != resolv_content
    {
        if let Ok(()) = fs::write(&resolv_conf, normalize_guest_text(&resolv_content)) {
            log::info!(
                "Updated guest /etc/resolv.conf with active DNS: {:?}",
                dns_servers
            );
        }
    }

    // Ensure /etc/nsswitch.conf exists with standard host resolution
    let nsswitch = etc_dir.join("nsswitch.conf");
    if !nsswitch.exists() {
        let nsswitch_content = "passwd:         files\ngroup:          files\nshadow:         files\ngshadow:        files\n\nhosts:          files dns\nnetworks:       files\n\nprotocols:      db files\nservices:       db files\nethers:         db files\nrpc:            db files\n\nnetgroup:       nis\n";
        let _ = fs::write(&nsswitch, normalize_guest_text(nsswitch_content));
        log::info!("Seeded guest /etc/nsswitch.conf");
    }

    // Ensure /etc/hosts exists with localhost definitions
    let hosts = etc_dir.join("hosts");
    if !hosts.exists() {
        let hosts_content =
            "127.0.0.1       localhost\n::1             localhost ip6-localhost ip6-loopback\n";
        let _ = fs::write(&hosts, normalize_guest_text(hosts_content));
        log::info!("Seeded guest /etc/hosts");
    }

    // Ensure SSL CA certificates are present
    sync_guest_ssl_certificates(fs_root);
}

/// Ensure OpenSSL and standard Linux tools inside the guest have access to valid CA certificates.
pub fn sync_guest_ssl_certificates(fs_root: &Path) {
    let certs_dir = fs_root.join("etc/ssl/certs");
    let _ = fs::create_dir_all(&certs_dir);
    let ca_bundle = certs_dir.join("ca-certificates.crt");

    if !ca_bundle.exists() || fs::metadata(&ca_bundle).map(|m| m.len()).unwrap_or(0) == 0 {
        let mut bundle_data = Vec::new();
        for dir in [
            "/system/etc/security/cacerts",
            "/apex/com.android.conscrypt/cacerts",
        ] {
            let path = Path::new(dir);
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    if let Ok(content) = fs::read(entry.path()) {
                        bundle_data.extend_from_slice(&content);
                        if !content.ends_with(b"\n") {
                            bundle_data.push(b'\n');
                        }
                    }
                }
            }
        }
        if !bundle_data.is_empty() {
            let _ = fs::write(&ca_bundle, &bundle_data);
            log::info!(
                "Bundled Android system CA certificates into {}",
                ca_bundle.display()
            );
        }
    }

    // Symlink /etc/ssl/cert.pem -> certs/ca-certificates.crt
    let cert_pem = fs_root.join("etc/ssl/cert.pem");
    if !cert_pem.exists() {
        let _ = symlink("certs/ca-certificates.crt", &cert_pem);
    }

    // Symlink /usr/lib/ssl -> /etc/ssl
    let usr_lib = fs_root.join("usr/lib");
    let _ = fs::create_dir_all(&usr_lib);
    let usr_lib_ssl = usr_lib.join("ssl");
    if !usr_lib_ssl.exists() {
        let _ = symlink("/etc/ssl", &usr_lib_ssl);
    }
}

/// Best-effort detached provisioning of the IBus input-method packages.
/// The Plasma autostart entry only ever starts an already-installed daemon
/// (never apt-get, never a fixed sleep), so packages must exist BEFORE the
/// session starts. Skipped entirely when the daemon is already installed or
/// a previous run completed. Runs on a detached thread and must never block
/// session startup: failures (e.g. offline) are logged and retried on a
/// later launch, and the session degrades to evdev keys meanwhile.
fn provision_ibus_packages(fs_root: &Path) {
    if fs_root.join("usr/bin/ibus-daemon").is_file() {
        return;
    }
    const MARKER: &str = "var/lib/localdesktop/ibus-provisioned-v1";
    if fs_root.join(MARKER).is_file() {
        return;
    }
    std::thread::spawn(|| {
        let output = ArchProcess {
            command: "DEBIAN_FRONTEND=noninteractive apt-get update >>/tmp/portal-ibus-provision.log 2>&1 && DEBIAN_FRONTEND=noninteractive apt-get install -y ibus gir1.2-ibus-1.0 python3-gi >>/tmp/portal-ibus-provision.log 2>&1".into(),
            user: None,
            log: None,
        }
        .run();
        let root = Path::new(PRODUCTION_FS_ROOT);
        if output.status.success() && root.join("usr/bin/ibus-daemon").is_file() {
            let _ = std::fs::write(root.join(MARKER), b"completed\n");
            log::info!("IBus input-method packages provisioned pre-session");
        } else {
            log::warn!(
                "IBus package provisioning deferred (status {:?}); X11/GTK input degrades to evdev keys until a later launch",
                output.status.code()
            );
        }
    });
}

fn setup_mesa_layer(options: &SetupOptions) -> StageOutput {
    // QPainter sessions must avoid Mesa provisioning entirely.
    if !crate::android::anland::is_anland_requested() {
        return None;
    }
    if super::mesa_layer::is_provisioned() {
        return None;
    }
    // Heavy work belongs in the spawned thread so the setup UI stays live
    // during the ~11 MB download/verify/extract/promote. The stage
    // completes (success or clearly reported failure) before Plasma setup
    // runs, so KWin can never launch while provisioning is unfinished.
    let report = options.progress.clone();
    let sender = options.mpsc_sender.clone();
    Some(thread::spawn(move || {
        super::mesa_layer::provision_with_progress(|message| {
            diagnostics::host_event("mesa-provisioning", &message);
            // Mesa keeps detailed failure text in native diagnostics. Do not
            // let its low-level error string briefly become installer copy.
            let display_message = if message.starts_with("Mesa layer unavailable:") {
                "Preparing Portal graphics…".to_string()
            } else {
                message
            };
            let _ = sender.send(SetupMessage::Progress(display_message.clone()));
            report(ProvisioningSnapshot::update(
                ProvisioningPhase::Configuring,
                78,
                display_message,
            ));
        })
    }))
}

fn setup_plasma_wayland(_options: &SetupOptions) -> StageOutput {
    let fs_root = Path::new(PRODUCTION_FS_ROOT);
    let username = get_application_context().local_config.user.username;
    let home_dir = chroot_home_dir(fs_root, &username);
    // The host Wayland compositor already establishes a logical viewport scaled by
    // guest_scale_factor; the guest Plasma session must run at 1:1 (scale 1) to prevent double scaling.
    let ui_scale = 1;
    if let Err(error) = try_sync_session_runtime_files(fs_root, ui_scale) {
        return Some(thread::spawn(move || Err(error)));
    }
    sync_kwin_overlay(fs_root);
    // Package installation remains in later setup stages; no detached apt
    // job may race desktop service or selected-application provisioning.
    // Mesa KGSL layer is provisioned by the dedicated `mesa-kgsl-layer`
    // setup stage (spawned thread with progress). Never download inline
    // here: Plasma setup only verifies presence and fails closed at GBM
    // setup with a diagnosable log when the layer is absent.
    if crate::android::anland::is_anland_requested() && !super::mesa_layer::is_provisioned() {
        log::error!("mesa KGSL layer missing at Plasma setup; Anland GPU boot will fail at GBM setup (will retry on next launch)");
    }

    sync_crash_handler(fs_root).expect("Failed to install crash handler support files");

    // Recovery creates its labwc autostart at runtime, after writing the
    // actionable kdialog message. Do not pre-seed an autostart that launches
    // a terminal or bypasses that recovery flow.

    // Konsole reads this path after PRoot has switched into the guest.  Do
    // not leak the host-side `/data/.../archlinux-*` prefix into the profile;
    // that path is not meaningful inside the guest namespace.
    let guest_home = if username == "root" {
        "/root".to_owned()
    } else {
        format!("/home/{username}")
    };
    migrate_konsole_profile(&home_dir, &guest_home);

    let config_dir = home_dir.join(".config");
    let autostart_dir = config_dir.join("autostart");
    let _ = fs::create_dir_all(&autostart_dir);
    // Plasma 6's launcher reads this KConfig gate; the similarly named
    // environment variables are not sufficient on current Plasma releases.
    // Keep the setting in the user's config so startplasma-wayland takes the
    // classic dbus-run-session path and never asks a missing user systemd to
    // own the session bus.
    let startkde_config = config_dir.join("startkderc");
    write_default_file(&startkde_config, "[General]\nsystemdBoot=false\n");
    upsert_kconfig_value(&startkde_config, "General", "systemdBoot", "false");
    fs::write(
        config_dir.join("ksmserverrc"),
        "[General]\nloginMode=emptySession\nconfirmLogout=false\n",
    )
    .expect("Failed to write Plasma session defaults");
    fs::write(
        autostart_dir.join("portal-ibus-daemon.desktop"),
        "[Desktop Entry]\nType=Application\nName=Portal IBus Daemon\nExec=/usr/local/bin/portal-ibus-lazy\nX-GNOME-Autostart-enabled=true\nX-KDE-autostart-after=panel\nNoDisplay=true\n",
    )
    .expect("Failed to write first-run IBus autostart entry");
    fs::write(
        autostart_dir.join("powerdevil.desktop"),
        "[Desktop Entry]\nType=Application\nName=Power Management\nHidden=true\nOnlyShowIn=KDE;\n",
    )
    .expect("Failed to disable guest power management autostart");

    let kwinrc = config_dir.join("kwinrc");
    upsert_kconfig_value(&kwinrc, "Input", "TabletMode", "off");
    upsert_kconfig_value(
        &kwinrc,
        "Wayland",
        "InputMethod",
        "/usr/share/applications/portal-ime.desktop",
    );
    upsert_kconfig_value(&kwinrc, "Wayland", "VirtualKeyboardMode", "1");

    let desktop_dir = home_dir.join("Desktop");
    let _ = fs::create_dir_all(&desktop_dir);
    let online_docs = desktop_dir.join("localdesktop-online-docs.desktop");
    if !online_docs.exists() {
        fs::write(
            online_docs,
            format!(
                "[Desktop Entry]\nType=Application\nName=Portal - Online Docs\nExec=firefox {DOCS_HOME_URL}\nIcon=firefox\nTerminal=false\n"
            ),
        )
        .expect("Failed to write documentation desktop entry");
    }

    None
}
fn fix_xkb_symlink(_options: &SetupOptions) -> StageOutput {
    let fs_root = Path::new(PRODUCTION_FS_ROOT);
    let xkb_path = fs_root.join("usr/share/X11/xkb");

    if let Ok(meta) = fs::symlink_metadata(&xkb_path) {
        if meta.file_type().is_symlink() {
            if let Ok(target) = fs::read_link(&xkb_path) {
                if target.is_absolute() {
                    log::info!(
                        "Absolute symlink target detected: {} -> {}. This is a problem because libxkbcommon is loaded in NDK, whose / is not Arch FS root!",
                        xkb_path.display(),
                        target.display()
                    );
                    // Compute the relative path from /usr/share/X11/xkb to /usr/share/xkeyboard-config-2
                    // Both are inside the chroot, so strip the fs_root prefix
                    let xkb_inside = Path::new("/usr/share/X11/xkb");
                    let target_inside = Path::new("/usr/share/xkeyboard-config-2");
                    let rel_target = diff_paths(
                        target_inside,
                        xkb_inside.parent().unwrap_or(Path::new("/")),
                    )
                        .unwrap_or_else(|| target_inside.to_path_buf());
                    log::info!(
                        "Fixing with new relative symlink: {} -> {}",
                        xkb_path.display(),
                        rel_target.display()
                    );
                    // Stage the replacement first. Renaming the temporary
                    // symlink over the old one is atomic on the Android
                    // filesystem, so a failed repair leaves the known-good
                    // old link intact.
                    let temporary = xkb_path.with_extension("portal-tmp");
                    let _ = fs::remove_file(&temporary);
                    if let Err(error) = symlink(&rel_target, &temporary) {
                        let message = format!("Failed to stage relative symlink for xkb: {error}");
                        return Some(thread::spawn(move || Err(anyhow::anyhow!(message))));
                    }
                    if let Err(error) = fs::rename(&temporary, &xkb_path) {
                        let _ = fs::remove_file(&temporary);
                        let message = format!("Failed to install relative symlink for xkb: {error}");
                        return Some(thread::spawn(move || Err(anyhow::anyhow!(message))));
                    }
                }
            }
        }
    }
    None
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else {
        "unknown panic".to_string()
    }
}

/// Publish one native provisioning state to both the process coordinator and
/// the two UI surfaces (Compose first, WebView fallback). The native
/// coordinator owns this state; a screen disappearing cannot stop the worker.
fn publish_snapshot(registration: &SetupRegistration, snapshot: ProvisioningSnapshot) {
    if let Some(progress) = registration.progress() {
        if let Ok(mut progress) = progress.lock() {
            *progress = snapshot.progress;
        }
    }
    if let Ok(mut coordinator) = setup_coordinator().lock() {
        coordinator.snapshot = snapshot.clone();
    }
    let message = if snapshot.error.is_some() {
        SetupMessage::Error(snapshot.message.clone())
    } else {
        SetupMessage::Progress(snapshot.message.clone())
    };
    if let Some(android_app) = registration.publish(message) {
        crate::android::utils::compose_overlay::publish_install_state(&android_app, &snapshot);
    }
}

fn publish_failure(registration: &SetupRegistration, failure: &SetupFailure) {
    log::error!(
        "Portal setup stage {} failed ({:?}): {}",
        failure.stage,
        failure.kind,
        failure.diagnostic
    );
    let progress = registration
        .progress()
        .and_then(|progress| progress.lock().ok().map(|progress| *progress))
        .unwrap_or(0);
    let mut snapshot = ProvisioningSnapshot::failed(failure.user_message.clone());
    snapshot.progress = progress;
    let provisional_handoff_failed = if let Ok(mut coordinator) = setup_coordinator().lock() {
        let provisional = matches!(
            &coordinator.initial_preferences_handoff,
            InitialPreferencesHandoff::Pending(_)
                | InitialPreferencesHandoff::Launched(_)
                | InitialPreferencesHandoff::Proven(_)
        );
        if provisional {
            coordinator.initial_preferences_handoff =
                InitialPreferencesHandoff::Failed(failure.user_message.clone());
            coordinator.pending_initial_preferences_failure =
                Some(failure.user_message.clone());
        }
        coordinator.state = InstallOperationState::Failed;
        coordinator.snapshot = snapshot.clone();
        provisional
    } else {
        false
    };
    mark_active_plan_failed();
    if provisional_handoff_failed {
        crate::android::utils::webview_handoff::cancel_initial_preferences_handoff();
        crate::android::utils::webview_handoff::wake_event_loop();
    }
    if let Some(current) = registration.progress() {
        if let Ok(mut current) = current.lock() {
            *current = progress;
        }
    }
    if let Some(android_app) =
        registration.publish(SetupMessage::Error(failure.user_message.clone()))
    {
        crate::android::utils::compose_overlay::publish_install_state(&android_app, &snapshot);
    }
}

fn publish_repair_snapshot(registration: &SetupRegistration, snapshot: ProvisioningSnapshot) {
    if let Ok(mut coordinator) = anland_repair_coordinator().lock() {
        coordinator.snapshot = snapshot.clone();
    }
    let message = if snapshot.error.is_some() {
        SetupMessage::Error(snapshot.message.clone())
    } else {
        SetupMessage::Progress(snapshot.message.clone())
    };
    if let Some(android_app) = registration.publish(message) {
        let (status, _) = anland_repair_ui_snapshot();
        crate::android::utils::compose_overlay::publish_anland_repair_state(
            &android_app,
            status,
            &snapshot,
        );
    }
}

const ANLAND_REPAIR_ERROR_MESSAGE: &str =
    "Portal is installed, but Anland graphics repair could not complete. Tap Retry Plasma.";

fn publish_repair_failure(registration: &SetupRegistration, diagnostic: impl Into<String>) {
    PREPARED_ANLAND_LAUNCH.store(false, Ordering::Release);
    let diagnostic = diagnostic.into();
    log::error!("Anland graphics repair failed: {diagnostic}");
    diagnostics::host_event("anland-repair-failed", &diagnostic);
    if let Ok(mut coordinator) = anland_repair_coordinator().lock() {
        coordinator.state = AnlandRepairState::Failed;
        coordinator.result = Some(AnlandRepairResult::Failed(
            ANLAND_REPAIR_ERROR_MESSAGE.to_string(),
        ));
    }
    publish_repair_snapshot(
        registration,
        ProvisioningSnapshot::failed(ANLAND_REPAIR_ERROR_MESSAGE),
    );
    crate::android::utils::webview_handoff::wake_event_loop();
}

fn publish_repair_success(registration: &SetupRegistration) {
    PREPARED_ANLAND_LAUNCH.store(true, Ordering::Release);
    if let Ok(mut coordinator) = anland_repair_coordinator().lock() {
        coordinator.state = AnlandRepairState::Complete;
        coordinator.result = Some(AnlandRepairResult::Succeeded);
    }
    // Do not publish ProvisioningPhase::Complete here. The Debian install
    // marker is intentionally unchanged by modern-runtime repair; this is a
    // graphics-repair result, not a second installation commit.
    publish_repair_snapshot(
        registration,
        ProvisioningSnapshot::update(
            ProvisioningPhase::Finalising,
            99,
            "Anland graphics ready. Restarting Plasma…",
        ),
    );
    diagnostics::host_event("anland-repair-complete", "targeted graphics repair committed");
    crate::android::utils::webview_handoff::wake_event_loop();
}

fn run_anland_repair_inner(registration: &SetupRegistration) -> anyhow::Result<()> {
    let artifact = crate::core::provisioning::RuntimeArtifact::production();
    let root = Path::new(PRODUCTION_FS_ROOT);
    let initial_classification = artifact.classify_runtime(root);
    anyhow::ensure!(
        initial_classification.is_trusted_recovery(),
        "explicit Anland repair requires an existing Portal completion marker (found {initial_classification:?})"
    );

    publish_repair_snapshot(
        registration,
        ProvisioningSnapshot::update(
            ProvisioningPhase::Preparing,
            70,
            "Preparing targeted Anland graphics repair…",
        ),
    );
    // The worker, rather than a JNI/UI caller, owns the bounded stop/reap so
    // an explicit repair action never blocks the Android main thread.
    crate::android::proot::launch::stop();

    publish_repair_snapshot(
        registration,
        ProvisioningSnapshot::update(
            ProvisioningPhase::Configuring,
            73,
            "Enabling the Anland renderer…",
        ),
    );
    crate::android::anland::force_anland_renderer()?;

    publish_repair_snapshot(
        registration,
        ProvisioningSnapshot::update(
            ProvisioningPhase::Configuring,
            76,
            "Checking Portal's Mesa/KGSL graphics layer…",
        ),
    );
    if super::mesa_layer::is_provisioned() {
        diagnostics::host_event("anland-repair", "Mesa KGSL layer already provisioned");
        publish_repair_snapshot(
            registration,
            ProvisioningSnapshot::update(
                ProvisioningPhase::Configuring,
                82,
                "Mesa/KGSL layer is already ready.",
            ),
        );
    } else {
        let progress_registration = registration.clone();
        super::mesa_layer::provision_with_progress(move |message| {
            diagnostics::host_event("mesa-provisioning", &message);
            let display_message = if message.starts_with("Mesa layer unavailable:")
                || message.to_ascii_lowercase().contains("failed")
            {
                "Preparing Portal graphics…".to_string()
            } else {
                message
            };
            publish_repair_snapshot(
                &progress_registration,
                ProvisioningSnapshot::update(
                    ProvisioningPhase::Configuring,
                    82,
                    display_message,
                ),
            );
        })?;
    }

    publish_repair_snapshot(
        registration,
        ProvisioningSnapshot::update(
            ProvisioningPhase::Configuring,
            88,
            "Refreshing Anland session integration…",
        ),
    );
    // This scoped helper writes only Portal-owned runtime assets. In
    // particular it does not call the broad normal-launch sync that edits
    // Plasma defaults, package metadata, or files in the user's home.
    sync_anland_required_session_files(root, 1)?;

    publish_repair_snapshot(
        registration,
        ProvisioningSnapshot::update(
            ProvisioningPhase::Finalising,
            96,
            "Verifying Anland graphics and Firefox acceleration…",
        ),
    );
    validate_anland_repair_state(root)?;

    // A legacy two-line Portal marker is already an installed runtime. Once
    // the targeted repair has succeeded, upgrade only that marker to the
    // modern durable form so the normal committed handoff can use it. The
    // Debian tree itself is never reprovisioned or replaced.
    if initial_classification == crate::core::provisioning::RuntimeClassification::LegacyPortal {
        artifact.mark_installation_complete(root)?;
        anyhow::ensure!(
            artifact.is_bootable(root),
            "legacy Portal marker did not migrate after Anland repair"
        );
    } else {
        artifact.validate_compatible(root)?;
    }
    validate_anland_repair_state(root)?;
    Ok(())
}

fn run_anland_repair(registration: SetupRegistration) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_anland_repair_inner(&registration)
    }));
    match result {
        Ok(Ok(())) => publish_repair_success(&registration),
        Ok(Err(error)) => publish_repair_failure(&registration, format!("{error:#}")),
        Err(payload) => publish_repair_failure(&registration, panic_text(payload.as_ref())),
    }
}

/// Invoke a stage behind a panic boundary. Remaining legacy helper functions
/// use `expect` for invariant-like guest writes; this boundary converts any
/// environmental failure into a retryable structured error and keeps panic
/// diagnostics native-only.
fn invoke_stage(
    index: usize,
    name: &'static str,
    stage: &SetupStage,
    options: &SetupOptions,
)
    -> Result<StageOutput, SetupFailure> {
    diagnostics::setup_stage(index, name, "start");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| stage(options)));
    match result {
        Ok(output) => Ok(output),
        Err(payload) => {
            diagnostics::setup_stage(index, name, "failed");
            Err(SetupFailure::from_panic(index, name, payload.as_ref()))
        }
    }
}

fn complete_stage(index: usize, name: &'static str) {
    diagnostics::setup_stage(index, name, "complete");
}

fn stage_progress(index: usize, count: usize) -> u16 {
    // Debian image work occupies 0..70; setup stages occupy 70..99. The
    // final marker write owns 100, so no stage can visually finish early.
    70 + ((index * 29) / count.max(1)).min(28) as u16
}

fn join_stage(
    index: usize,
    name: &'static str,
    handle: JoinHandle<anyhow::Result<()>>,
) -> Result<(), SetupFailure> {
    match handle.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(SetupFailure::from_detail(index, name, format!("{error:#}"))),
        Err(payload) => Err(SetupFailure::from_panic(index, name, payload.as_ref())),
    }
}

fn stages() -> Vec<NamedSetupStage> {
    vec![
        ("debian-runtime", Box::new(setup_debian_runtime)),
        ("renderer-mode", Box::new(setup_renderer_mode)),
        ("linux-sysdata", Box::new(simulate_linux_sysdata_stage)),
        ("machine-id", Box::new(setup_machine_id)),
        ("firefox-config", Box::new(setup_firefox_config)),
        ("bwrap-shim", Box::new(setup_fake_bwrap)),
        ("chromium-no-sandbox", Box::new(setup_chromium_no_sandbox)),
        ("onboard-signal-fix", Box::new(setup_onboard_signal_fix)),
        ("mesa-kgsl-layer", Box::new(setup_mesa_layer)),
        ("plasma-wayland", Box::new(setup_plasma_wayland)),
        ("desktop-login", Box::new(setup_desktop_login)),
        ("optional-apps", Box::new(setup_optional_apps)),
        ("xkb-symlink", Box::new(fix_xkb_symlink)),
    ]
}

fn run_all_stages(
    stages: Vec<NamedSetupStage>,
    options: &SetupOptions,
    registration: &SetupRegistration,
) -> Result<(), SetupFailure> {
    let stage_count = stages.len();
    for (index, (name, stage)) in stages.into_iter().enumerate() {
        publish_snapshot(
            registration,
            ProvisioningSnapshot::update(
                ProvisioningPhase::Configuring,
                stage_progress(index, stage_count),
                format!("Configuring Portal ({name})…"),
            ),
        );
        let output = invoke_stage(index + 1, name, &stage, options)?;
        if let Some(handle) = output {
            join_stage(index + 1, name, handle)?;
        }
        complete_stage(index + 1, name);
        publish_snapshot(
            registration,
            ProvisioningSnapshot::update(
                ProvisioningPhase::Configuring,
                stage_progress(index + 1, stage_count),
                format!("Portal setup stage complete: {name}"),
            ),
        );
    }
    Ok(())
}

fn finalise_installation(registration: &SetupRegistration) -> Result<(), SetupFailure> {
    publish_snapshot(
        registration,
        ProvisioningSnapshot::update(
            ProvisioningPhase::Finalising,
            99,
            "Finalising Portal installation…",
        ),
    );
    let renderer = crate::android::anland::ensure_renderer_mode().map_err(|error| {
        SetupFailure::from_detail(2, "renderer-mode", format!("{error:#}"))
    })?;
    if matches!(renderer, crate::android::anland::RendererKind::Anland)
        && !super::mesa_layer::is_provisioned()
    {
        return Err(SetupFailure::from_detail(
            8,
            "mesa-kgsl-layer",
            "Mesa KGSL layer is not provisioned after the setup stage",
        ));
    }
    let artifact = crate::core::provisioning::RuntimeArtifact::production();
    artifact
        .mark_installation_complete(Path::new(PRODUCTION_FS_ROOT))
        .map_err(|error| SetupFailure::from_detail(11, "installation-marker", format!("{error:#}")))?;
    if !artifact.is_bootable(Path::new(PRODUCTION_FS_ROOT)) {
        return Err(SetupFailure::from_detail(
            11,
            "installation-marker",
            "final installation marker did not validate after being written",
        ));
    }
    Ok(())
}

fn atomic_write_guest_file(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .context("Initial-preferences payload has no parent directory")?;
    fs::create_dir_all(parent).context("Could not create initial-preferences directory")?;
    let parent_metadata = fs::symlink_metadata(parent)
        .context("Could not inspect initial-preferences directory")?;
    anyhow::ensure!(
        parent_metadata.is_dir() && !parent_metadata.file_type().is_symlink(),
        "Initial-preferences directory is not a real directory"
    );
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(
        ".initial-preferences-{}-{nonce}.tmp",
        process::id()
    ));
    let write_result = (|| -> anyhow::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .context("Could not create staged initial-preferences payload")?;
        file.write_all(bytes)
            .context("Could not write staged initial-preferences payload")?;
        file.sync_all()
            .context("Could not sync staged initial-preferences payload")?;
        drop(file);
        fs::rename(&temporary, path)
            .context("Could not commit initial-preferences payload")?;
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .context("Could not sync initial-preferences directory")?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

fn remove_guest_file(path: &Path, required: bool) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "Refusing to remove a non-regular initial-preferences file"
            );
            fs::remove_file(path).context("Could not remove initial-preferences file")
        }
        Err(error) if error.kind() == ErrorKind::NotFound && !required => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            Err(error).context("Required initial-preferences file is missing")
        }
        Err(error) => Err(error).context("Could not inspect initial-preferences file"),
    }
}

fn initial_appearance_proof_path(root: &Path) -> PathBuf {
    let username = get_application_context().local_config.user.username;
    chroot_home_dir(root, &username).join(INITIAL_APPEARANCE_PROOF)
}

fn prepare_initial_preferences_handoff(
    root: &Path,
    android_app: &AndroidApp,
    plan: &InstallPlan,
) -> anyhow::Result<(AppliedAppearance, String)> {
    let appearance = resolve_initial_appearance(android_app, plan)?;
    let fingerprint = plan.fingerprint()?;
    let scale = plan.initial_output_scale_string();
    anyhow::ensure!(
        plan.initial_output_scale().is_finite()
            && plan.initial_output_scale() > 0.0
            && plan.initial_output_scale() <= 5.0,
        "Accepted initial output scale is outside KWin's supported range"
    );
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let attempt_id = format!("{}-{nonce}", process::id());
    let proof_path = initial_appearance_proof_path(root);
    remove_guest_file(&proof_path, false)?;
    let app_ids = plan
        .selected_apps()
        .iter()
        .map(|app| app.id())
        .collect::<Vec<_>>()
        .join(",");
    let payload = format!(
        "version=2\nfingerprint={fingerprint}\nattempt_id={attempt_id}\nappearance={}\nscale={scale}\napp_ids={app_ids}\n",
        match appearance {
            AppliedAppearance::Dark => "dark",
            AppliedAppearance::Light => "light",
        }
    );
    atomic_write_guest_file(&root.join(INITIAL_APPEARANCE_PLAN), payload.as_bytes())?;
    Ok((appearance, attempt_id))
}

fn read_appearance_proof(path: &Path) -> anyhow::Result<Option<String>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Could not inspect initial appearance proof"),
    };
    anyhow::ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Initial appearance proof is not a regular file"
    );
    let file = fs::File::open(path).context("Could not read initial appearance proof")?;
    let mut bytes = Vec::new();
    file.take(8 * 1024 + 1)
        .read_to_end(&mut bytes)
        .context("Could not read initial appearance proof")?;
    anyhow::ensure!(
        bytes.len() <= 8 * 1024,
        "Initial appearance proof exceeds the supported size"
    );
    let contents = String::from_utf8(bytes).context("Initial appearance proof is not UTF-8")?;
    Ok(Some(contents))
}

fn verify_initial_preferences_proof(
    root: &Path,
    plan: &InstallPlan,
    appearance: AppliedAppearance,
    attempt_id: &str,
) -> anyhow::Result<bool> {
    let Some(contents) = read_appearance_proof(&initial_appearance_proof_path(root))? else {
        return Ok(false);
    };
    validate_initial_setup_proof(&contents, plan, appearance, attempt_id)?;
    Ok(true)
}

fn wait_for_initial_preferences_proof(
    root: &Path,
    plan: &InstallPlan,
    appearance: AppliedAppearance,
    attempt_id: &str,
) -> anyhow::Result<()> {
    let started = Instant::now();
    loop {
        let handoff = setup_coordinator()
            .lock()
            .map(|coordinator| coordinator.initial_preferences_handoff.clone())
            .map_err(|_| anyhow::anyhow!("Initial-preferences handoff state is unavailable"))?;
        match &handoff {
            InitialPreferencesHandoff::Failed(reason) => {
                anyhow::bail!("Provisional Plasma launch failed: {reason}");
            }
            InitialPreferencesHandoff::Pending(active)
            | InitialPreferencesHandoff::Launched(active)
                if active == plan => {}
            InitialPreferencesHandoff::Proven(active) if active == plan => return Ok(()),
            _ => anyhow::bail!("Initial-preferences handoff ended before proof was accepted"),
        }
        if matches!(&handoff, InitialPreferencesHandoff::Launched(_))
            && verify_initial_preferences_proof(root, plan, appearance, attempt_id)?
        {
            let mut coordinator = setup_coordinator()
                .lock()
                .map_err(|_| anyhow::anyhow!("Initial-preferences handoff state is unavailable"))?;
            match &coordinator.initial_preferences_handoff {
                InitialPreferencesHandoff::Launched(active) if active == plan => {
                    coordinator.initial_preferences_handoff =
                        InitialPreferencesHandoff::Proven(plan.clone());
                    return Ok(());
                }
                InitialPreferencesHandoff::Failed(reason) => {
                    anyhow::bail!("Provisional Plasma launch failed: {reason}");
                }
                _ => anyhow::bail!("Initial-preferences proof arrived for an inactive plan"),
            }
        }
        if started.elapsed() >= INITIAL_APPEARANCE_HANDOFF_TIMEOUT {
            anyhow::bail!("Timed out waiting for verified KScreen appearance and scale state");
        }
        thread::sleep(INITIAL_APPEARANCE_POLL_INTERVAL);
    }
}

fn apply_initial_preferences_before_commit(
    registration: &SetupRegistration,
    android_app: &AndroidApp,
    plan: &InstallPlan,
) -> Result<(), SetupFailure> {
    let root = Path::new(PRODUCTION_FS_ROOT);
    stage_initial_appearance_helper(root)
        .map_err(|error| SetupFailure::from_detail(12, "initial-preferences", format!("{error:#}")))?;
    let prepared = prepare_initial_preferences_handoff(root, android_app, plan)
        .map_err(|error| SetupFailure::from_detail(12, "initial-preferences", format!("{error:#}")))?;
    let (appearance, attempt_id) = prepared;
    {
        let mut coordinator = setup_coordinator().lock().map_err(|_| {
            SetupFailure::from_detail(
                12,
                "initial-preferences",
                "setup coordinator lock is poisoned",
            )
        })?;
        if coordinator.state != InstallOperationState::Running
            || coordinator.install_plan.as_ref() != Some(plan)
        {
            return Err(SetupFailure::from_detail(
                12,
                "initial-preferences",
                "Accepted install plan changed before provisional Plasma handoff",
            ));
        }
        coordinator.initial_preferences_handoff =
            InitialPreferencesHandoff::Pending(plan.clone());
        coordinator.pending_initial_preferences_failure = None;
    }
    publish_snapshot(
        registration,
        ProvisioningSnapshot::update(
            ProvisioningPhase::Finalising,
            99,
            "Applying the selected appearance, display size, and panel shortcuts…",
        ),
    );
    let Some(android_app) = registration.android_app() else {
        return Err(SetupFailure::from_detail(
            12,
            "initial-preferences",
            "Android activity binding is unavailable",
        ));
    };
    crate::android::utils::webview_handoff::request_initial_preferences_handoff(android_app);
    wait_for_initial_preferences_proof(root, plan, appearance, &attempt_id)
        .map_err(|error| SetupFailure::from_detail(12, "initial-preferences", format!("{error:#}")))?;
    // The helper remains installed but has nothing to apply after this point.
    // Remove both staged files before the durable runtime marker is written.
    remove_guest_file(&root.join(INITIAL_APPEARANCE_PLAN), true)
        .and_then(|()| remove_guest_file(&initial_appearance_proof_path(root), true))
        .map_err(|error| SetupFailure::from_detail(12, "initial-preferences", format!("{error:#}")))?;
    Ok(())
}

fn run_installation(registration: SetupRegistration) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let progress_registration = registration.clone();
        let progress: Arc<dyn Fn(ProvisioningSnapshot) + Send + Sync> = Arc::new(move |snapshot| {
            publish_snapshot(&progress_registration, snapshot);
        });
        let Some(android_app) = registration.android_app() else {
            return Err(SetupFailure::from_detail(
                0,
                "setup-coordinator",
                "setup UI binding is unavailable",
            ));
        };
        let Some(mpsc_sender) = registration.sender() else {
            return Err(SetupFailure::from_detail(
                0,
                "setup-coordinator",
                "setup progress channel is unavailable",
            ));
        };
        let install_plan = setup_coordinator()
            .lock()
            .ok()
            .and_then(|coordinator| coordinator.install_plan.clone())
            .context("Accepted install plan is unavailable to the worker")
            .map_err(|error| SetupFailure::from_detail(0, "setup-coordinator", format!("{error:#}")))?;
        let preferences_app = android_app.clone();
        let options = SetupOptions {
            android_app,
            mpsc_sender,
            progress,
            install_plan: Some(install_plan.clone()),
            install_optional_apps: true,
        };
        run_all_stages(stages(), &options, &registration)
            .and_then(|()| {
                apply_initial_preferences_before_commit(
                    &registration,
                    &preferences_app,
                    &install_plan,
                )
            })
            .and_then(|()| finalise_installation(&registration))
    }));
    match result {
        Ok(Ok(())) => {
            mark_active_plan_complete();
            crate::android::utils::webview_handoff::cancel_initial_preferences_handoff();
            let snapshot = ProvisioningSnapshot::complete("Portal installed. Starting Plasma…");
            if let Ok(mut coordinator) = setup_coordinator().lock() {
                coordinator.state = InstallOperationState::Complete;
                coordinator.snapshot = snapshot.clone();
            }
            publish_snapshot(&registration, snapshot);
            diagnostics::host_event("setup-complete", "all guest provisioning stages completed");
            // A first-run plan already closed the fallback popup and is now
            // running through the provisional Wayland backend. Calling the
            // ordinary marker-backed WebView callback here would publish a
            // second, semantically different handoff edge.
            let initial_plan = setup_coordinator()
                .lock()
                .ok()
                .is_some_and(|coordinator| coordinator.install_plan.is_some());
            if !initial_plan {
                if let Some(on_complete) = registration.completion_callback() {
                if let Err(payload) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        on_complete();
                    }))
                {
                    log::error!(
                        "Portal setup handoff callback panicked after installation commit: {}",
                        panic_text(payload.as_ref())
                    );
                }
                }
            }
        }
        Ok(Err(failure)) => publish_failure(&registration, &failure),
        Err(payload) => {
            let failure = SetupFailure::from_panic(0, "setup-coordinator", payload.as_ref());
            publish_failure(&registration, &failure);
        }
    }
}

fn set_registration(
    registration: SetupRegistration,
    state: InstallOperationState,
    snapshot: ProvisioningSnapshot,
) {
    let (active_registration, active_snapshot) = if let Ok(mut coordinator) = setup_coordinator().lock()
    {
        if coordinator.state == InstallOperationState::Running {
            if let Some(active_registration) = coordinator.registration.clone() {
                active_registration.rebind_from(&registration);
                (active_registration, coordinator.snapshot.clone())
            } else {
                coordinator.registration = Some(registration.clone());
                coordinator.state = state;
                coordinator.snapshot = snapshot.clone();
                (registration, snapshot)
            }
        } else {
            coordinator.registration = Some(registration.clone());
            coordinator.state = state;
            coordinator.snapshot = snapshot.clone();
            (registration, snapshot)
        }
    } else {
        (registration, snapshot)
    };
    publish_snapshot(&active_registration, active_snapshot);
}

fn set_operation_complete(registration: &SetupRegistration) {
    let snapshot = ProvisioningSnapshot::complete("Portal installation is complete.");
    if let Ok(mut coordinator) = setup_coordinator().lock() {
        coordinator.state = InstallOperationState::Complete;
        coordinator.snapshot = snapshot.clone();
    }
    publish_snapshot(registration, snapshot);
}

fn start_installation_worker(registration: SetupRegistration, plan: InstallPlan) -> bool {
    let snapshot = ProvisioningSnapshot::update(
        ProvisioningPhase::Preparing,
        0,
        "Preparing Portal installation…",
    );
    {
        let Ok(mut coordinator) = setup_coordinator().lock() else {
            return false;
        };
        if coordinator.state == InstallOperationState::Running {
            return coordinator.install_plan.as_ref() == Some(&plan);
        }
        if coordinator.state == InstallOperationState::Complete
            || coordinator
                .install_plan
                .as_ref()
                .is_some_and(|current| current != &plan)
        {
            return false;
        }
        coordinator.registration = Some(registration.clone());
        coordinator.state = InstallOperationState::Running;
        coordinator.snapshot = snapshot.clone();
        coordinator.install_plan = Some(plan);
        coordinator.initial_preferences_handoff = InitialPreferencesHandoff::Idle;
        coordinator.pending_initial_preferences_failure = None;
    }
    publish_snapshot(&registration, snapshot);
    let worker_registration = registration.clone();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        thread::spawn(move || run_installation(worker_registration))
    })) {
        Ok(_) => true,
        Err(payload) => {
            let failure = SetupFailure::from_panic(0, "setup-coordinator", payload.as_ref());
            publish_failure(&registration, &failure);
            false
        }
    }
}

/// Persist the exact proposal before process state changes or guest work
/// begins. Duplicate calls attach only when they match the already accepted
/// native plan.
pub fn begin_install(plan_json: &str) -> bool {
    let proposed = match InstallPlan::from_json(plan_json) {
        Ok(plan) => plan,
        Err(error) => {
            log::error!("Rejected first-run install plan: {error:#}");
            return false;
        }
    };
    let registration = {
        let Ok(coordinator) = setup_coordinator().lock() else {
            return false;
        };
        let Some(registration) = coordinator.registration.clone() else {
            return false;
        };
        if coordinator.state == InstallOperationState::Running {
            let matches = coordinator.install_plan.as_ref() == Some(&proposed);
            if !matches {
                log::error!("Rejected a different plan while first-run installation is active");
            }
            return matches;
        }
        if coordinator.state == InstallOperationState::Complete
            || coordinator
                .install_plan
                .as_ref()
                .is_some_and(|current| current != &proposed)
        {
            return false;
        }
        registration
    };
    let accepted = match persist_plan_before_start(Some(proposed), false) {
        Ok(plan) => plan,
        Err(error) => {
            log::error!("Could not persist first-run install plan: {error:#}");
            return false;
        }
    };
    start_installation_worker(registration, accepted)
}

/// Retry only the immutable app-private plan. This call has no plan argument
/// by design, so recreated UI defaults and changed display metrics cannot
/// replace the choices already accepted by the user.
pub fn retry_install() -> bool {
    let registration = {
        let Ok(coordinator) = setup_coordinator().lock() else {
            return false;
        };
        if coordinator.state == InstallOperationState::Running {
            return true;
        }
        if coordinator.state == InstallOperationState::Complete {
            return false;
        }
        let Some(registration) = coordinator.registration.clone() else {
            return false;
        };
        registration
    };
    let record = match load_persisted_install_plan() {
        Ok(Some(record)) => record,
        Ok(None) => {
            log::error!("Retry rejected because no accepted install plan is persisted");
            return false;
        }
        Err(error) => {
            log::error!("Retry rejected because the persisted install plan is invalid: {error:#}");
            return false;
        }
    };
    if record.state() != InstallPlanState::Failed {
        log::error!("Retry rejected because the accepted plan is not in the Failed state");
        return false;
    }
    let root = Path::new(PRODUCTION_FS_ROOT);
    let classification = crate::core::provisioning::RuntimeArtifact::production()
        .classify_runtime(root);
    if !matches!(
        classification,
        crate::core::provisioning::RuntimeClassification::Absent
            | crate::core::provisioning::RuntimeClassification::ValidatedImageOnly
    ) {
        log::error!("Retry rejected to preserve unclassified runtime state: {classification:?}");
        return false;
    }
    let plan = match persist_plan_before_start(None, true) {
        Ok(plan) => plan,
        Err(error) => {
            log::error!("Could not re-arm accepted install plan for retry: {error:#}");
            return false;
        }
    };
    start_installation_worker(registration, plan)
}

/// Start or attach to the one explicit existing-install Anland repair.
///
/// This is intentionally a native action rather than a Compose coroutine.
/// It is accepted only for a currently marked Portal runtime, never invokes
/// Debian provisioning/extraction, and leaves the installation coordinator
/// and completion marker alone. A caller may invoke it again after a consumed
/// failure/result; while the worker is active all callers attach to it.
pub fn repair_enable_anland() -> bool {
    let registration = {
        let Ok(coordinator) = setup_coordinator().lock() else {
            return false;
        };
        if coordinator.state == InstallOperationState::Running {
            log::warn!("Ignoring Anland repair while Portal installation is running");
            return false;
        }
        let Some(registration) = coordinator.registration.clone() else {
            log::warn!("Ignoring Anland repair because native setup registration is unavailable");
            return false;
        };
        let artifact = crate::core::provisioning::RuntimeArtifact::production();
        let classification = artifact.classify_runtime(Path::new(PRODUCTION_FS_ROOT));
        if !classification.is_trusted_recovery() {
            log::warn!(
                "Ignoring Anland repair because the runtime is not a completed Portal install: {classification:?}"
            );
            return false;
        }
        registration
    };

    let (start, snapshot) = {
        let Ok(mut coordinator) = anland_repair_coordinator().lock() else {
            return false;
        };
        if coordinator.state == AnlandRepairState::Running
            || coordinator.result.is_some()
        {
            (false, coordinator.snapshot.clone())
        } else {
            coordinator.state = AnlandRepairState::Running;
            coordinator.snapshot = ProvisioningSnapshot::update(
                ProvisioningPhase::Preparing,
                70,
                "Preparing targeted Anland graphics repair…",
            );
            (true, coordinator.snapshot.clone())
        }
    };

    publish_repair_snapshot(&registration, snapshot);
    if !start {
        return true;
    }

    let worker_registration = registration.clone();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        thread::spawn(move || run_anland_repair(worker_registration))
    })) {
        Ok(_) => true,
        Err(payload) => {
            publish_repair_failure(&registration, panic_text(payload.as_ref()));
            false
        }
    }
}

/// Consume the completion edge of the explicit repair operation. The result
/// is consumed only by the event-loop owner; the coordinator state remains
/// Complete/Failed so a later explicit request can retry after this edge.
pub fn take_anland_repair_result() -> Option<AnlandRepairResult> {
    anland_repair_coordinator()
        .lock()
        .ok()
        .and_then(|mut coordinator| coordinator.result.take())
}

/// Rebind a recreated Activity to an existing repair worker before the normal
/// completed-runtime path can synchronously replay setup. The worker keeps its
/// original registration handle, so the handle is rebound in place rather than
/// replaced underneath it. A pending result is included too: the event-loop
/// owner still consumes that result and performs the single Wayland handoff.
fn attach_to_anland_repair(
    registration: &SetupRegistration,
    progress: &Arc<Mutex<u16>>,
) -> Option<ProvisioningSnapshot> {
    let snapshot = anland_repair_coordinator().lock().ok().and_then(|coordinator| {
        if coordinator.state == AnlandRepairState::Running || coordinator.result.is_some() {
            Some(coordinator.snapshot.clone())
        } else {
            None
        }
    })?;
    let active_registration = setup_coordinator()
        .lock()
        .ok()
        .and_then(|coordinator| coordinator.registration.clone())?;
    active_registration.rebind_from(registration);
    if let Ok(mut current) = progress.lock() {
        *current = snapshot.progress;
    }
    publish_repair_snapshot(&active_registration, snapshot.clone());
    Some(snapshot)
}

/// Re-publish the process-lifetime snapshot after an Activity/Compose
/// recreation. Compose also retains the value itself, but this native replay
/// makes a late overlay show deterministic.
pub fn publish_current_install_state(android_app: &AndroidApp) {
    let snapshot = setup_coordinator()
        .lock()
        .ok()
        .map(|coordinator| coordinator.snapshot.clone());
    if let Some(snapshot) = snapshot {
        crate::android::utils::compose_overlay::publish_install_state(android_app, &snapshot);
    }
}

/// Whether the setup worker has finished all guest stages and is waiting for
/// the provisional Plasma session to prove this exact persisted plan.
pub fn can_launch_pending_initial_preferences(root: &Path) -> bool {
    let artifact = crate::core::provisioning::RuntimeArtifact::production();
    if artifact.is_bootable(root)
        || artifact.classify_runtime(root)
            != crate::core::provisioning::RuntimeClassification::ValidatedImageOnly
    {
        return false;
    }
    let Ok(Some(record)) = load_persisted_install_plan() else {
        return false;
    };
    if record.state() != InstallPlanState::InProgress {
        return false;
    }
    setup_coordinator()
        .lock()
        .map(|coordinator| {
            coordinator.state == InstallOperationState::Running
                && coordinator.install_plan.as_ref() == Some(record.plan())
                && matches!(
                    &coordinator.initial_preferences_handoff,
                    InitialPreferencesHandoff::Pending(plan)
                        | InitialPreferencesHandoff::Launched(plan)
                        if plan == record.plan()
                )
        })
        .unwrap_or(false)
}

/// Build the temporary Wayland backend used only to run first-session
/// appearance/scale provisioning. The runtime marker remains absent until the
/// setup worker validates the helper's readback proof.
pub fn build_pending_wayland_backend(
    android_app: AndroidApp,
) -> anyhow::Result<PolarBearBackend> {
    let root = Path::new(PRODUCTION_FS_ROOT);
    anyhow::ensure!(
        can_launch_pending_initial_preferences(root),
        "No validated first-run preferences handoff is pending"
    );
    {
        let mut coordinator = setup_coordinator()
            .lock()
            .map_err(|_| anyhow::anyhow!("Setup coordinator state is unavailable"))?;
        match &coordinator.initial_preferences_handoff {
            InitialPreferencesHandoff::Pending(plan) => {
                coordinator.initial_preferences_handoff =
                    InitialPreferencesHandoff::Launched(plan.clone());
            }
            InitialPreferencesHandoff::Launched(_) => {}
            _ => anyhow::bail!("Initial-preferences handoff is no longer pending"),
        }
    }
    build_wayland_backend(android_app)
}

/// Record a failed provisional Plasma launch and wake the lifecycle owner so
/// it can stop the temporary guest and return to setup under the same plan.
pub fn fail_initial_preferences_handoff(reason: &str) -> bool {
    if crate::core::provisioning::RuntimeArtifact::production()
        .is_bootable(Path::new(PRODUCTION_FS_ROOT))
    {
        return false;
    }
    let reason = reason.chars().take(512).collect::<String>();
    let accepted = if let Ok(mut coordinator) = setup_coordinator().lock() {
        let active_plan = match &coordinator.initial_preferences_handoff {
            InitialPreferencesHandoff::Pending(plan)
            | InitialPreferencesHandoff::Launched(plan) => Some(plan.clone()),
            InitialPreferencesHandoff::Failed(_) => return true,
            InitialPreferencesHandoff::Idle | InitialPreferencesHandoff::Proven(_) => None,
        };
        let Some(plan) = active_plan else { return false };
        coordinator.initial_preferences_handoff =
            InitialPreferencesHandoff::Failed(reason.clone());
        coordinator.pending_initial_preferences_failure = Some(
            "Portal could not verify the selected appearance and display size. Tap Retry Setup."
                .to_string(),
        );
        Some(plan)
    } else {
        None
    };
    let Some(_plan) = accepted else { return false };
    log::error!("Initial-preferences Plasma handoff failed: {reason}");
    diagnostics::host_event("initial-preferences-failed", &reason);
    mark_active_plan_failed();
    crate::android::utils::webview_handoff::cancel_initial_preferences_handoff();
    crate::android::utils::webview_handoff::wake_event_loop();
    true
}

/// Consume the one pending provisional failure event. The worker owns durable
/// plan failure publication; the event-loop owner uses this edge to swap away
/// from the temporary Wayland backend.
pub fn take_initial_preferences_failure() -> Option<String> {
    setup_coordinator()
        .lock()
        .ok()
        .and_then(|mut coordinator| coordinator.pending_initial_preferences_failure.take())
}

fn build_wayland_backend(android_app: AndroidApp) -> anyhow::Result<PolarBearBackend> {
    let size = android_app
        .native_window()
        .map(|nw| (nw.width(), nw.height()))
        .unwrap_or((1920, 1080));
    let guest_scale_factor = scale_factor(&android_app);
    let mut compositor = Compositor::new(size, guest_scale_factor)
        .map_err(|error| anyhow::anyhow!("Failed to build compositor: {error}"))?;
    compositor.enable_android_clipboard(android_app.clone());
    Ok(PolarBearBackend::Wayland(WaylandBackend {
        compositor,
        graphic_renderer: None,
        clock: Clock::new(),
        key_counter: 0,
        guest_scale_factor,
        touch_points: std::collections::HashMap::new(),
        scroll_centroid: None,
        touch_scroll_started: false,
        native_touch_active: false,
        native_touch_preferred: std::fs::read_to_string(format!(
            "{}/touch-mode",
            crate::core::config::APP_FILES_ROOT
        ))
        .map(|value| !value.trim().eq_ignore_ascii_case("pointer"))
        .unwrap_or(true),
        touch_mode: TouchMode::Undecided,
        touch_down_position: None,
        touch_down_time: None,
        touch_down_generation: None,
        touch_slop_px: touch_slop_px(&android_app),
        long_press_timeout_ms: long_press_timeout_ms(&android_app),
        pointer_pressed: false,
        finger_scroll_axes: Default::default(),
        continuous_scroll_axes: Default::default(),
        presentation_sequence: 0,
        pending_kwin_presentation: None,
        // Nominal output mode is the stable preferred target resolved from
        // `Display.getSupportedModes()` (144 Hz on the OnePlus Pad 3,
        // otherwise the device maximum): never the transient cold-start VRR
        // reading. The live physical rate is tracked separately in
        // `physical_refresh_millihz` for diagnostics/pacing and never
        // rewrites `wl_output`.
        refresh_rate_millihz: crate::android::utils::ndk::preferred_high_refresh_millihz(
            &android_app,
        ),
        physical_refresh_millihz: active_refresh_millihz(&android_app),
        pressed_keys: std::collections::HashSet::new(),
        button_tracker: crate::core::pointer_buttons::PointerButtonTracker::new(),
        suppressed_touch_ids: std::collections::HashSet::new(),
        last_plasma_poll_ms: None,
        last_refresh_poll_ms: None,
        frame_rate_requested: false,
        kwin_commit_gate: crate::core::presentation::KwinCommitGate::new(),
        socket_watcher: None,
        output_dirty: true,
        output_damage_tracker: None,
        output_damage_signature: None,
        frame_pacer: crate::android::accessibility::event_loop_proxy()
            .and_then(crate::android::utils::frame_pacing::AndroidFramePacer::new),
        frame_timeline: None,
        frame_timeline_stats: Default::default(),
        pipeline_stats: Default::default(),
        surface_control_cursor: None,
        frame_in_flight: false,
        anland: None,
        surface_convergence: crate::core::surface_geometry::SurfaceConvergence::new(),
        android_app,
    }))
}

/// Construct the Wayland backend after a provisioning worker has committed
/// and revalidated the durable installation marker. This deliberately does
/// not call any setup stage: the immediate first-install handoff must not
/// replay work that has just succeeded. Normal future launches use the
/// committed-runtime repair path, which leaves first-run stages behind.
pub fn build_committed_wayland_backend(
    android_app: AndroidApp,
) -> anyhow::Result<PolarBearBackend> {
    let artifact = crate::core::provisioning::RuntimeArtifact::production();
    anyhow::ensure!(
        artifact.is_bootable(Path::new(PRODUCTION_FS_ROOT)),
        "Committed Portal runtime is no longer bootable"
    );
    // The completion marker is not enough to choose a renderer: a process
    // could have died after an older install committed but before this config
    // was initialized. Repair/validate the durable choice at the handoff
    // boundary so a fresh Anland-capable install can never silently take the
    // QPainter path because the flag is absent.
    crate::android::anland::ensure_renderer_mode()
        .map_err(|error| anyhow::anyhow!("renderer-mode is not durable: {error:#}"))?;
    build_wayland_backend(android_app)
}

/// Backwards-compatible setup entry point. Lifecycle owners that can dismiss
/// the provisioning popup in-process should use `setup_with_completion`.
pub fn setup(android_app: AndroidApp) -> PolarBearBackend {
    setup_with_completion(android_app, None)
}

fn setup_failure_backend(
    registration: &SetupRegistration,
    receiver: mpsc::Receiver<SetupMessage>,
    progress: Arc<Mutex<u16>>,
    plan: Option<InstallPlan>,
    message: &str,
) -> PolarBearBackend {
    if let Some(plan) = plan.as_ref() {
        if let Ok(Some(record)) = load_persisted_install_plan() {
            if record.plan() == plan && record.state() == InstallPlanState::InProgress {
                if let Err(error) = persist_install_plan_state(plan, InstallPlanState::Failed) {
                    log::error!("Could not persist setup recovery failure: {error:#}");
                }
            }
        }
    }
    let snapshot = ProvisioningSnapshot::failed(message.to_string());
    if let Ok(mut coordinator) = setup_coordinator().lock() {
        coordinator.registration = Some(registration.clone());
        coordinator.state = InstallOperationState::Failed;
        coordinator.snapshot = snapshot.clone();
        coordinator.install_plan = plan;
        coordinator.initial_preferences_handoff = InitialPreferencesHandoff::Idle;
        coordinator.pending_initial_preferences_failure = None;
    }
    crate::android::utils::webview_handoff::cancel_initial_preferences_handoff();
    publish_snapshot(registration, snapshot);
    let mut backend = WebviewBackend::build(receiver, progress);
    backend.error = ErrorVariant::Setup(message.to_string());
    PolarBearBackend::WebView(backend)
}

fn is_truly_fresh_runtime(root: &Path) -> bool {
    let artifact = crate::core::provisioning::RuntimeArtifact::production();
    if artifact.classify_runtime(root)
        != crate::core::provisioning::RuntimeClassification::Absent
    {
        return false;
    }
    let Some(base) = root.parent() else { return false };
    [
        "runtime-B.staging",
        "runtime-B.previous",
        "runtime-B.previous.pending",
    ]
    .iter()
    .all(|name| !base.join(name).exists())
}

/// A committed desktop retains its own Plasma and application settings.
/// Refresh only Portal-owned runtime support and perform the one-time login
/// migration; the first-run provisioning stages must never be replayed here.
fn prepare_committed_runtime(registration: &SetupRegistration) -> Result<(), SetupFailure> {
    let root = Path::new(PRODUCTION_FS_ROOT);
    let renderer = crate::android::anland::ensure_renderer_mode().map_err(|error| {
        SetupFailure::from_detail(2, "renderer-mode", format!("{error:#}"))
    })?;
    if matches!(renderer, crate::android::anland::RendererKind::Anland)
        && !super::mesa_layer::is_provisioned()
    {
        super::mesa_layer::provision_with_progress(|message| {
            diagnostics::host_event("mesa-provisioning", &message);
            publish_snapshot(
                registration,
                ProvisioningSnapshot::update(
                    ProvisioningPhase::Configuring,
                    78,
                    "Preparing Portal graphics…",
                ),
            );
        })
        .map_err(|error| SetupFailure::from_detail(9, "mesa-kgsl-layer", format!("{error:#}")))?;
    }
    try_sync_session_runtime_files(root, 1)
        .map_err(|error| SetupFailure::from_detail(10, "portal-runtime", format!("{error:#}")))?;
    sync_crash_handler(root)
        .map_err(|error| SetupFailure::from_detail(10, "crash-handler", format!("{error:#}")))?;
    prepare_desktop_login(true)
        .map_err(|error| SetupFailure::from_detail(11, "desktop-login", format!("{error:#}")))?;
    Ok(())
}

/// Provision the guest and invoke `on_complete` after the final stage without
/// recreating the NativeActivity. The lifecycle owner can use that callback to
/// send an event through its event-loop proxy and construct the Wayland backend
/// in the current activity.
pub fn setup_with_completion(
    android_app: AndroidApp,
    on_complete: Option<SetupCompletionCallback>,
) -> PolarBearBackend {
    let (sender, receiver) = mpsc::channel();
    let progress = Arc::new(Mutex::new(0));

    if !ArchProcess::is_supported(&android_app) {
        log::info!("PRoot support check failed, showing Device Unsupported page");
        diagnostics::host_event("setup-unsupported", "PRoot support probe failed");
        return PolarBearBackend::WebView(WebviewBackend::unsupported(android_app));
    }
    let _ = sender.send(SetupMessage::Progress("✅ Your device is supported!".to_string()));

    let registration = SetupRegistration::new(
        android_app.clone(),
        sender.clone(),
        progress.clone(),
        on_complete,
    );

    // An explicit Anland migration is a separate process-lifetime operation.
    // Activity/Compose recreation must attach to it before the completed
    // runtime branch below has a chance to replay the broad setup pipeline.
    if attach_to_anland_repair(&registration, &progress).is_some() {
        return PolarBearBackend::WebView(WebviewBackend::build(receiver, progress));
    }

    let artifact = crate::core::provisioning::RuntimeArtifact::production();
    let root = Path::new(PRODUCTION_FS_ROOT);
    let existing_operation = setup_coordinator()
        .lock()
        .ok()
        .map(|coordinator| (coordinator.state, coordinator.snapshot.clone()));
    // A new NativeActivity/Compose instance may ask for setup while the
    // process-lifetime worker is still running. Rebind the existing sink and
    // return a receiver for the new UI; never reset the coordinator to Idle or
    // launch a second provisioning worker.
    if matches!(
        existing_operation,
        Some((InstallOperationState::Running, _))
    )
    {
        let _ = set_registration(
            registration,
            InstallOperationState::Running,
            ProvisioningSnapshot::update(
                ProvisioningPhase::Preparing,
                0,
                "Reconnecting to Portal installation…",
            ),
        );
        return PolarBearBackend::WebView(WebviewBackend::build(receiver, progress));
    }
    // A failed operation is also process-lifetime state. Rebinding after an
    // Activity recreation must keep the Retry screen visible; otherwise a
    // resume would silently turn a known failure back into Idle and the HTML
    // fallback could start an uncontrolled retry loop.
    if let Some((InstallOperationState::Failed, snapshot)) = existing_operation {
        set_registration(registration, InstallOperationState::Failed, snapshot);
        let mut backend = WebviewBackend::build(receiver, progress);
        backend.error = ErrorVariant::Setup(
            setup_coordinator()
                .lock()
                .ok()
                .and_then(|coordinator| coordinator.snapshot.error.clone())
                .unwrap_or_else(|| "Portal setup needs attention. Tap Retry.".to_string()),
        );
        return PolarBearBackend::WebView(backend);
    }

    // The plan is the source of truth for a first-run transaction. A new
    // process resumes the immutable InProgress record; it never reconstructs
    // accepted selections from whatever defaults a recreated UI happens to
    // display. A malformed record is actionable unless a committed runtime
    // marker already makes that transaction redundant.
    let persisted_plan = match load_persisted_install_plan() {
        Ok(record) => record,
        Err(error) if artifact.is_bootable(root) || artifact.is_legacy_complete(root) => {
            log::error!("Ignoring invalid first-run plan beside trusted runtime marker: {error:#}");
            None
        }
        Err(error) => {
            let message = format!(
                "Portal found a damaged first-run plan and preserved the existing runtime. Export diagnostics or restore the accepted plan before retrying. ({error:#})"
            );
            return setup_failure_backend(&registration, receiver, progress, None, &message);
        }
    };

    if artifact.is_bootable(root) || artifact.is_legacy_complete(root) {
        if let Some(record) = persisted_plan.as_ref() {
            if record.state() == InstallPlanState::InProgress {
                // The installation marker is the durable commit point. This
                // repairs a crash between marker creation and Complete-record
                // persistence without re-running first-run-only work.
                if let Err(error) = record
                    .with_state(InstallPlanState::Complete)
                    .write_atomic(&persisted_install_plan_path())
                {
                    log::warn!("Could not reconcile completed install-plan record: {error:#}");
                }
            }
        }
        // A committed runtime has crossed the first-run transaction boundary.
        // Keep its marker and user configuration intact while refreshing only
        // Portal-owned support and migrating the login once.
        let initial = ProvisioningSnapshot::update(
            ProvisioningPhase::Configuring,
            70,
            "Checking the existing Portal installation…",
        );
        set_registration(
            registration.clone(),
            InstallOperationState::Running,
            initial,
        );
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            prepare_committed_runtime(&registration)
        }));
        let result = match result {
            Ok(result) => result,
            Err(payload) => Err(SetupFailure::from_panic(
                0,
                "setup-coordinator",
                payload.as_ref(),
            )),
        };
        match result {
            Ok(()) => {
                set_operation_complete(&registration);
                match build_wayland_backend(android_app) {
                    Ok(backend) => return backend,
                    Err(error) => {
                        let failure = SetupFailure::from_detail(
                            12,
                            "wayland-backend",
                            format!("{error:#}"),
                        );
                        publish_failure(&registration, &failure);
                        let mut backend = WebviewBackend::build(receiver, progress);
                        backend.error = ErrorVariant::Setup(failure.user_message);
                        return PolarBearBackend::WebView(backend);
                    }
                }
            }
            Err(failure) => {
                publish_failure(&registration, &failure);
                let mut backend = WebviewBackend::build(receiver, progress);
                backend.error = ErrorVariant::Setup(failure.user_message);
                return PolarBearBackend::WebView(backend);
            }
        }
    }

    if let Some(record) = persisted_plan {
        match record.state() {
            InstallPlanState::Complete => {
                return setup_failure_backend(
                    &registration,
                    receiver,
                    progress,
                    Some(record.plan().clone()),
                    "Portal has a completed setup plan but its runtime marker is missing. The existing guest was preserved; export diagnostics before recovery.",
                );
            }
            InstallPlanState::Failed => {
                let message =
                    "Portal setup previously failed and preserved its accepted choices. Tap Retry Setup to continue.";
                return setup_failure_backend(
                    &registration,
                    receiver,
                    progress,
                    Some(record.plan().clone()),
                    message,
                );
            }
            InstallPlanState::InProgress => {
                let classification = artifact.classify_runtime(root);
                if !matches!(
                    classification,
                    crate::core::provisioning::RuntimeClassification::Absent
                        | crate::core::provisioning::RuntimeClassification::ValidatedImageOnly
                ) {
                    let message = format!(
                        "Portal found a partial runtime for the accepted setup plan ({classification:?}) and preserved it. Export diagnostics before recovery."
                    );
                    return setup_failure_backend(
                        &registration,
                        receiver,
                        progress,
                        Some(record.plan().clone()),
                        &message,
                    );
                }
                let plan = match persist_plan_before_start(None, false) {
                    Ok(plan) => plan,
                    Err(error) => {
                        let message = format!(
                            "Portal could not resume the accepted setup plan; the guest was preserved. Export diagnostics before recovery. ({error:#})"
                        );
                        return setup_failure_backend(
                            &registration,
                            receiver,
                            progress,
                            Some(record.plan().clone()),
                            &message,
                        );
                    }
                };
                if !start_installation_worker(registration.clone(), plan) {
                    return setup_failure_backend(
                        &registration,
                        receiver,
                        progress,
                        Some(record.plan().clone()),
                        "Portal could not restart its accepted setup worker. The guest and choices were preserved; tap Retry Setup or export diagnostics.",
                    );
                }
                return PolarBearBackend::WebView(WebviewBackend::build(receiver, progress));
            }
        }
    }

    if !is_truly_fresh_runtime(root) {
        return setup_failure_backend(
            &registration,
            receiver,
            progress,
            None,
            "Portal found an existing or partial runtime without its accepted setup plan. It was preserved; export diagnostics or restore the plan before retrying.",
        );
    }
    let initial = ProvisioningSnapshot::update(
        ProvisioningPhase::Idle,
        0,
        "Portal setup is ready to begin.",
    );
    set_registration(registration, InstallOperationState::Idle, initial);
    PolarBearBackend::WebView(WebviewBackend::build(receiver, progress))
}

#[cfg(test)]
mod tests {
    use super::{normalize_guest_text, supervise_plasmashell_autostart};

    #[test]
    fn guest_scripts_are_written_with_unix_line_endings() {
        assert_eq!(
            normalize_guest_text("#!/bin/bash\r\nready\r\n"),
            "#!/bin/bash\nready\n"
        );
        assert_eq!(normalize_guest_text("line\rnext\n"), "line\next\n");
    }

    #[test]
    fn plasma_shell_supervisor_preserves_distro_autostart_permissions() {
        let distro = "[Desktop Entry]\nExec=/usr/bin/plasmashell\nX-DBUS-ServiceName=org.kde.plasmashell\nX-KDE-autostart-phase=0\nX-KDE-Wayland-Interfaces=org_kde_plasma_window_management\n";
        let supervised = supervise_plasmashell_autostart(distro).unwrap();
        assert_eq!(
            supervised,
            distro.replace(
                "Exec=/usr/bin/plasmashell",
                "Exec=/usr/local/bin/localdesktop-plasmashell-supervisor"
            )
        );
        assert_eq!(supervise_plasmashell_autostart(&supervised).unwrap(), supervised);
        assert!(supervise_plasmashell_autostart("[Desktop Entry]\nExec=/tmp/other\n").is_err());
    }
}
