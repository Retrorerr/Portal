//! Optional apps on an installed runtime: the Return screen's app manager,
//! the install/remove worker it shares with first-run setup, and Portal's
//! Steam integration files.
//!
//! Policy (which packages, which commands, what a device needs) lives in
//! `crate::core::optional_apps`; this module runs it in the guest and
//! reports per-app progress to Compose. Installs run beside a live Plasma
//! session: they only add packages, so nothing running loses its libraries.

use crate::{
    android::{
        diagnostics,
        runtime::proot::PRootRuntime,
        utils::{application_context::get_application_context, compose_overlay},
    },
    core::{
        config::PRODUCTION_FS_ROOT,
        install_plan::{optional_app_catalog, OptionalApp},
        optional_apps::{
            install_command, installed_packages, is_installed, parse_bootstrap_progress,
            prepare_apt_command, remove_command, unavailable_reason, DeviceCapabilities,
            OPTIONAL_APPS_LOG, STEAM_MARKER_REL, STEAM_ROOT_HOME_REL,
        },
        runtime::{LinuxRuntime, ProcessSpec},
        system_updates::{overall_progress, parse_apt_status, AptStatus},
    },
};
use anyhow::Context;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

const PUBLISH_INTERVAL: Duration = Duration::from_millis(150);

/// Progress sink: overall percent and a short status line.
type Progress = Arc<dyn Fn(u16, String) + Send + Sync>;

// Portal's Steam integration, synced into the guest while Steam is installed.
const STEAM_LAUNCHER: &str = include_str!("../../../assets/steam/portal-steam.sh");
const STEAM_BOOTSTRAP: &str = include_str!("../../../assets/steam/portal-steam-bootstrap.py");
const STEAM_DESKTOP: &str = include_str!("../../../assets/steam/portal-steam.desktop");
const STEAM_ICON: &[u8] = include_bytes!("../../../assets/steam/steam-256.png");
const STEAM_COMPAT_TOOL_VDF: &str =
    include_str!("../../../assets/steam/compat/compatibilitytool.vdf");
const STEAM_COMPAT_MANIFEST_VDF: &str =
    include_str!("../../../assets/steam/compat/toolmanifest.vdf");
const STEAM_COMPAT_SCRIPT: &str = include_str!("../../../assets/steam/compat/portal-proton.sh");
const STEAM_BOX64_TOOL_VDF: &str =
    include_str!("../../../assets/steam/compat-box64/compatibilitytool.vdf");
const STEAM_BOX64_MANIFEST_VDF: &str =
    include_str!("../../../assets/steam/compat-box64/toolmanifest.vdf");
const STEAM_BOX64_SCRIPT: &str = include_str!("../../../assets/steam/compat-box64/portal-box64.sh");
const STEAM_COMPAT_MAPPER: &str = include_str!("../../../assets/steam/portal-steam-compat.py");
/// Source: `assets/guest-arm64/portal-lsof.c`. Steam checks its websocket
/// peers with `lsof -i`, which needs /proc/net/tcp; Android denies it.
const PORTAL_LSOF: &[u8] = include_bytes!("../../../assets/guest-arm64/portal-lsof");

const STEAM_LAUNCHER_REL: &str = "usr/local/bin/steam";
const STEAM_BOOTSTRAP_REL: &str = "usr/local/lib/portal/steam/portal-steam-bootstrap.py";
const STEAM_DESKTOP_REL: &str = "usr/share/applications/portal-steam.desktop";
const STEAM_ICON_REL: &str = "usr/share/icons/hicolor/256x256/apps/steam.png";
const STEAM_COMPAT_DIR_REL: &str = "usr/share/steam/compatibilitytools.d/portal-proton-arm64";
const STEAM_BOX64_DIR_REL: &str = "usr/share/steam/compatibilitytools.d/portal-box64";
const STEAM_COMPAT_MAPPER_REL: &str = "usr/local/lib/portal/steam/portal-steam-compat.py";
const PORTAL_LSOF_REL: &str = "usr/sbin/lsof";
const GUEST_STEAM_BOOTSTRAP: &str = "/usr/local/lib/portal/steam/portal-steam-bootstrap.py";

/// Per-app state codes shared with Kotlin (`ComposeOverlay.APP_*`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppState {
    Absent,
    Installed,
    Queued,
    Installing,
    Removing,
    Failed,
}

impl AppState {
    pub const fn code(self) -> i32 {
        match self {
            Self::Absent => 0,
            Self::Installed => 1,
            Self::Queued => 2,
            Self::Installing => 3,
            Self::Removing => 4,
            Self::Failed => 5,
        }
    }

    const fn busy(self) -> bool {
        matches!(self, Self::Queued | Self::Installing | Self::Removing)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Install,
    Remove,
}

#[derive(Clone, Debug)]
struct Entry {
    state: AppState,
    progress: u16,
    message: String,
}

struct Coordinator {
    loaded: bool,
    entries: HashMap<OptionalApp, Entry>,
    queue: VecDeque<(OptionalApp, Action)>,
    worker: bool,
}

fn coordinator() -> &'static Mutex<Coordinator> {
    static COORDINATOR: OnceLock<Mutex<Coordinator>> = OnceLock::new();
    COORDINATOR.get_or_init(|| {
        Mutex::new(Coordinator {
            loaded: false,
            entries: HashMap::new(),
            queue: VecDeque::new(),
            worker: false,
        })
    })
}

fn root() -> &'static Path {
    Path::new(PRODUCTION_FS_ROOT)
}

/// What this device offers, for the catalog's availability column.
pub fn device_capabilities() -> DeviceCapabilities {
    // SAFETY: sysconf has no preconditions.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    DeviceCapabilities {
        anland: crate::android::anland::is_anland_requested(),
        kgsl: crate::android::anland::kgsl_available(),
        page_size: usize::try_from(page_size).unwrap_or(0),
    }
}

/// The catalog the pickers show, with this device's availability.
pub fn catalog() -> String {
    let caps = device_capabilities();
    optional_app_catalog(|app| unavailable_reason(app, caps).map(str::to_owned))
}

fn installed_now(fs_root: &Path) -> HashSet<OptionalApp> {
    let dpkg = fs::read_to_string(fs_root.join("var/lib/dpkg/status"))
        .map(|status| installed_packages(&status))
        .unwrap_or_default();
    let steam = fs_root.join(STEAM_MARKER_REL).is_file();
    OptionalApp::ALL
        .into_iter()
        .filter(|&app| is_installed(app, &dpkg, steam))
        .collect()
}

fn idle_entry(installed: bool) -> Entry {
    Entry {
        state: if installed { AppState::Installed } else { AppState::Absent },
        progress: 0,
        message: String::new(),
    }
}

/// Refresh idle entries from the guest; busy and failed ones keep their
/// state until the worker or the user changes it.
fn refresh(coordinator: &mut Coordinator) {
    let installed = installed_now(root());
    for app in OptionalApp::ALL {
        let entry = coordinator
            .entries
            .entry(app)
            .or_insert_with(|| idle_entry(false));
        if !entry.state.busy() && entry.state != AppState::Failed {
            *entry = idle_entry(installed.contains(&app));
        }
    }
    coordinator.loaded = true;
}

/// `id state progress message` per app, in catalog order.
fn state_text(coordinator: &Coordinator) -> String {
    OptionalApp::ALL
        .iter()
        .filter_map(|app| {
            let entry = coordinator.entries.get(app)?;
            Some(format!(
                "{}\t{}\t{}\t{}",
                app.id(),
                entry.state.code(),
                entry.progress,
                entry.message.replace(['\t', '\n'], " ")
            ))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Rehydrate the Return screen's app manager.
pub fn publish_current(android_app: &winit::platform::android::activity::AndroidApp) {
    let text = {
        let Ok(mut coordinator) = coordinator().lock() else {
            return;
        };
        if !coordinator.worker {
            refresh(&mut coordinator);
        }
        state_text(&coordinator)
    };
    compose_overlay::publish_optional_apps_state(android_app, &text);
}

fn publish() {
    publish_current(&get_application_context().android_app);
}

fn set_entry(app: OptionalApp, change: impl FnOnce(&mut Entry)) {
    if let Ok(mut coordinator) = coordinator().lock() {
        if let Some(entry) = coordinator.entries.get_mut(&app) {
            change(entry);
        }
    }
    publish();
}

/// Queue an install or removal the user asked for. Returns false when the
/// request cannot be accepted (unknown app, unavailable here, or another
/// Portal operation owns apt).
pub fn request(app_id: &str, install: bool) -> bool {
    let Some(app) = OptionalApp::from_id(app_id) else {
        log::warn!("optional-apps: ignoring unknown app {app_id:?}");
        return false;
    };
    if !crate::core::provisioning::RuntimeArtifact::production().is_bootable(root()) {
        log::warn!("optional-apps: the runtime is not a completed Portal install");
        return false;
    }
    if crate::android::proot::system_updates::update_in_progress() {
        log::warn!("optional-apps: a system update is running");
        return false;
    }
    if install {
        if let Some(reason) = unavailable_reason(app, device_capabilities()) {
            log::warn!("optional-apps: {} is unavailable: {reason}", app.id());
            return false;
        }
    }
    let action = if install { Action::Install } else { Action::Remove };
    let spawn = {
        let Ok(mut coordinator) = coordinator().lock() else {
            return false;
        };
        if !coordinator.loaded {
            refresh(&mut coordinator);
        }
        let Some(entry) = coordinator.entries.get_mut(&app) else {
            return false;
        };
        if entry.state.busy() {
            return true;
        }
        let installed = entry.state == AppState::Installed;
        if install == installed && entry.state != AppState::Failed {
            return true;
        }
        entry.state = AppState::Queued;
        entry.progress = 0;
        entry.message = String::new();
        coordinator.queue.push_back((app, action));
        !std::mem::replace(&mut coordinator.worker, true)
    };
    publish();
    if spawn {
        thread::spawn(run_queue);
    }
    true
}

fn run_queue() {
    loop {
        let next = coordinator()
            .lock()
            .ok()
            .and_then(|mut coordinator| {
                let next = coordinator.queue.pop_front();
                if next.is_none() {
                    coordinator.worker = false;
                }
                next
            });
        let Some((app, action)) = next else {
            break;
        };
        set_entry(app, |entry| {
            entry.state = match action {
                Action::Install => AppState::Installing,
                Action::Remove => AppState::Removing,
            };
            entry.progress = 1;
            entry.message = "Preparing…".to_owned();
        });
        let outcome = std::panic::catch_unwind(|| match action {
            Action::Install => install(app, entry_progress(app)),
            Action::Remove => remove(app),
        })
        .unwrap_or_else(|_| Err(anyhow::anyhow!("the app worker panicked")));
        match outcome {
            Ok(()) => {
                diagnostics::host_event(
                    "optional-app",
                    &format!("{} {:?} complete", app.id(), action),
                );
                let installed = installed_now(root()).contains(&app);
                set_entry(app, |entry| *entry = idle_entry(installed));
            }
            Err(error) => {
                let detail = format!("{error:#}");
                log::error!("optional-apps: {} {action:?} failed: {detail}", app.id());
                diagnostics::host_event("optional-app-failed", &format!("{} {detail}", app.id()));
                set_entry(app, |entry| {
                    entry.state = AppState::Failed;
                    entry.message = detail;
                });
            }
        }
    }
    publish();
}

fn entry_progress(app: OptionalApp) -> Progress {
    Arc::new(move |progress, message| {
        set_entry(app, |entry| {
            entry.progress = entry.progress.max(progress);
            entry.message = message;
        });
    })
}

fn log_tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let tail: Vec<&str> = text.lines().rev().take(4).collect();
    tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
}

/// Run guest shell as root with apt status on fd 3 reported through
/// `progress`, scaled into `[from, to]`.
fn run_apt(command: &str, from: u16, to: u16, progress: Progress) -> anyhow::Result<()> {
    crate::android::proot::system_updates::sync_apt_policy(root())?;
    let last_error = Arc::new(Mutex::new(None::<String>));
    let callback_error = last_error.clone();
    let last_publish = Mutex::new(Instant::now() - PUBLISH_INTERVAL);
    let shell = format!(
        "exec 3>&1 >>{OPTIONAL_APPS_LOG} 2>&1; echo \"== Portal optional apps $(date -Is) ==\"; {command}"
    );
    let output = PRootRuntime::active().execute(
        ProcessSpec::new(shell)
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
            let Some(overall) = overall_progress(&status) else {
                return;
            };
            let scaled = from + (u32::from(overall) * u32::from(to - from) / 100) as u16;
            let message = match &status {
                AptStatus::Download(_) => "Downloading…".to_owned(),
                AptStatus::Install(_, message) => {
                    crate::core::system_updates::friendly_status(message)
                }
                AptStatus::Error(_) => return,
            };
            progress(scaled, message);
        })),
        None,
    );
    if !output.status.success() {
        let detail = last_error
            .lock()
            .ok()
            .and_then(|error| error.clone())
            .unwrap_or_else(|| {
                format!(
                    "apt exited with status {:?} {}",
                    output.status.code(),
                    log_tail(&output.stderr)
                )
            });
        anyhow::bail!("{detail}; see {OPTIONAL_APPS_LOG}");
    }
    Ok(())
}

fn desktop_user() -> String {
    get_application_context().local_config.user.username
}

/// Install one app: its Debian side, then Portal's per-app follow-up.
fn install(app: OptionalApp, progress: Progress) -> anyhow::Result<()> {
    let fs_root = root();
    crate::android::proot::setup::validate_optional_app_apt_setup(fs_root)?;
    let steam = app == OptionalApp::Steam;
    let apt_end = if steam { 40 } else { 96 };
    progress(3, "Refreshing package lists…".to_owned());
    run_apt(
        &format!("{} && {}", prepare_apt_command(), install_command(app)),
        3,
        apt_end,
        progress.clone(),
    )?;
    let installed = fs::read_to_string(fs_root.join("var/lib/dpkg/status"))
        .map(|status| installed_packages(&status))
        .context("could not read the Debian package status")?;
    anyhow::ensure!(
        app.required_packages()
            .iter()
            .all(|package| installed.contains(*package)),
        "{} is not fully installed after apt finished",
        app.name()
    );
    if app.is_electron() {
        prepare_electron_launcher(app)?;
    }
    if steam {
        install_steam(progress)?;
    }
    Ok(())
}

/// Official Electron apps ship their own launchers, which need Portal's copy
/// (Wayland, stdio and shortcut flags) in the user's applications directory.
fn prepare_electron_launcher(app: OptionalApp) -> anyhow::Result<()> {
    let desktop_file = app.desktop_file_id();
    anyhow::ensure!(
        root().join("usr/share/applications").join(desktop_file).is_file(),
        "The official {} desktop entry is missing",
        app.name()
    );
    let command = format!(
        "/usr/local/bin/localdesktop-no-sandbox-entries && grep -q '^X-LocalDesktop-NoSandbox=true$' \"$HOME/.local/share/applications/{desktop_file}\""
    );
    let output = PRootRuntime::active().execute(
        ProcessSpec::new(command).with_user(desktop_user()),
        None,
        None,
    );
    anyhow::ensure!(
        output.status.success(),
        "The official {} launcher could not be prepared for Portal's PRoot session: {}",
        app.name(),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

/// Mark Steam installed, sync Portal's files, then seed Valve's ARM64 client
/// so the first start goes straight to the client's own updater.
fn install_steam(progress: Progress) -> anyhow::Result<()> {
    let fs_root = root();
    write_file(&fs_root.join(STEAM_MARKER_REL), b"1\n", 0o644)?;
    sync_steam_files(fs_root)?;
    progress(42, "Downloading Steam for ARM64…".to_owned());
    let last_publish = Mutex::new(Instant::now() - PUBLISH_INTERVAL);
    let output = PRootRuntime::active().execute(
        ProcessSpec::new(format!(
            "test -x \"$HOME/{STEAM_ROOT_HOME_REL}/steamrtarm64/steam\" || \
             python3 {GUEST_STEAM_BOOTSTRAP} --root \"$HOME/{STEAM_ROOT_HOME_REL}\""
        ))
        .with_user(desktop_user()),
        Some(Arc::new(move |line: String| {
            let Some((done, total)) = parse_bootstrap_progress(&line) else {
                return;
            };
            let Ok(mut last) = last_publish.lock() else {
                return;
            };
            if last.elapsed() < PUBLISH_INTERVAL && done < total {
                return;
            }
            *last = Instant::now();
            let scaled = 42 + (done * 56 / total) as u16;
            progress(
                scaled,
                format!("Downloading Steam… {} / {} MB", done / 1_000_000, total / 1_000_000),
            );
        })),
        None,
    );
    anyhow::ensure!(
        output.status.success(),
        "The Steam client could not be downloaded: {}",
        log_tail(&output.stderr)
    );
    Ok(())
}

/// Remove one app. Debian apps go through apt; Steam's client and Portal's
/// files are deleted, but its game library (`steamapps`) is always kept.
fn remove(app: OptionalApp) -> anyhow::Result<()> {
    let fs_root = root();
    if let Some(command) = remove_command(app) {
        crate::android::proot::setup::validate_optional_app_apt_setup(fs_root)?;
        run_apt(&command, 5, 96, entry_progress(app))?;
        if app.is_electron() {
            let _ = fs::remove_file(
                user_home(fs_root)
                    .join(".local/share/applications")
                    .join(app.desktop_file_id()),
            );
        }
        return Ok(());
    }
    // Steam: stop the client, then remove everything but the games.
    let output = PRootRuntime::active().execute(
        ProcessSpec::new(format!(
            "pkill -f '/steamrtarm64/steam' ; sleep 1; \
             S=\"$HOME/{STEAM_ROOT_HOME_REL}\"; \
             [ -d \"$S\" ] && find \"$S\" -mindepth 1 -maxdepth 1 ! -name steamapps -exec rm -rf {{}} + ; \
             rm -f \"$HOME/.steam/steam\" \"$HOME/.steam/root\" \"$HOME/.steam/sdk64\" \"$HOME/.steam/steam.pid\"; true"
        ))
        .with_user(desktop_user()),
        None,
        None,
    );
    anyhow::ensure!(
        output.status.success(),
        "The Steam client could not be removed: {}",
        log_tail(&output.stderr)
    );
    fs::remove_file(fs_root.join(STEAM_MARKER_REL)).or_else(ignore_missing)?;
    sync_steam_integration(fs_root);
    Ok(())
}

fn ignore_missing(error: std::io::Error) -> std::io::Result<()> {
    if error.kind() == std::io::ErrorKind::NotFound {
        Ok(())
    } else {
        Err(error)
    }
}

fn user_home(fs_root: &Path) -> std::path::PathBuf {
    fs_root.join("home").join(desktop_user())
}

fn write_file(path: &Path, contents: &[u8], mode: u32) -> anyhow::Result<()> {
    if fs::read(path).is_ok_and(|current| current == contents)
        && fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o777 == mode)
    {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("portal-tmp");
    fs::write(&temporary, contents)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(mode))?;
    fs::rename(&temporary, path)?;
    // PRoot's fake-root sidecar would override the host mode we just set.
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        let _ = fs::remove_file(
            parent.join(format!(".proot-meta-file.{}.meta", name.to_string_lossy())),
        );
    }
    Ok(())
}

/// Guest text from a Windows checkout may carry CRLF; a shebang must not.
fn guest_text(text: &str) -> Vec<u8> {
    text.replace("\r\n", "\n").into_bytes()
}

fn sync_steam_files(fs_root: &Path) -> anyhow::Result<()> {
    write_file(&fs_root.join(STEAM_LAUNCHER_REL), &guest_text(STEAM_LAUNCHER), 0o755)?;
    write_file(&fs_root.join(STEAM_BOOTSTRAP_REL), &guest_text(STEAM_BOOTSTRAP), 0o755)?;
    write_file(&fs_root.join(STEAM_DESKTOP_REL), &guest_text(STEAM_DESKTOP), 0o644)?;
    write_file(&fs_root.join(STEAM_ICON_REL), STEAM_ICON, 0o644)?;
    let compat = fs_root.join(STEAM_COMPAT_DIR_REL);
    write_file(&compat.join("compatibilitytool.vdf"), &guest_text(STEAM_COMPAT_TOOL_VDF), 0o644)?;
    write_file(&compat.join("toolmanifest.vdf"), &guest_text(STEAM_COMPAT_MANIFEST_VDF), 0o644)?;
    write_file(&compat.join("portal-proton"), &guest_text(STEAM_COMPAT_SCRIPT), 0o755)?;
    let box64 = fs_root.join(STEAM_BOX64_DIR_REL);
    write_file(&box64.join("compatibilitytool.vdf"), &guest_text(STEAM_BOX64_TOOL_VDF), 0o644)?;
    write_file(&box64.join("toolmanifest.vdf"), &guest_text(STEAM_BOX64_MANIFEST_VDF), 0o644)?;
    write_file(&box64.join("portal-box64"), &guest_text(STEAM_BOX64_SCRIPT), 0o755)?;
    write_file(&fs_root.join(STEAM_COMPAT_MAPPER_REL), &guest_text(STEAM_COMPAT_MAPPER), 0o755)?;
    write_file(&fs_root.join(PORTAL_LSOF_REL), PORTAL_LSOF, 0o755)?;
    Ok(())
}

/// Keep Portal's Steam files current while Steam is installed and gone when
/// it is not. Runs before every session.
pub fn sync_steam_integration(fs_root: &Path) {
    if fs_root.join(STEAM_MARKER_REL).is_file() {
        if let Err(error) = sync_steam_files(fs_root) {
            log::warn!("optional-apps: could not sync Steam files: {error:#}");
        }
        return;
    }
    for rel in [
        STEAM_LAUNCHER_REL,
        STEAM_BOOTSTRAP_REL,
        STEAM_COMPAT_MAPPER_REL,
        STEAM_DESKTOP_REL,
        STEAM_ICON_REL,
        PORTAL_LSOF_REL,
    ] {
        let path = fs_root.join(rel);
        let ours = match rel {
            STEAM_ICON_REL => fs::read(&path).is_ok_and(|bytes| bytes == STEAM_ICON),
            PORTAL_LSOF_REL => fs::read(&path).is_ok_and(|bytes| bytes == PORTAL_LSOF),
            _ => fs::read_to_string(&path).is_ok_and(|text| {
                text.contains("Managed by Portal")
                    || text.contains("X-Portal-Managed=true")
                    || text.contains("Valve's native ARM64 Linux Steam client")
            }),
        };
        if ours {
            let _ = fs::remove_file(path);
        }
    }
    let _ = fs::remove_dir_all(fs_root.join(STEAM_COMPAT_DIR_REL));
    let _ = fs::remove_dir_all(fs_root.join(STEAM_BOX64_DIR_REL));
}

/// First-run setup: install `apps` (already known to be missing) in one
/// guest run, then their follow-ups. Fails on the first app that fails.
pub fn install_for_setup(apps: &[OptionalApp]) -> anyhow::Result<()> {
    if apps.is_empty() {
        return Ok(());
    }
    let quiet: Progress = Arc::new(|_, _| {});
    let commands = apps
        .iter()
        .map(|&app| install_command(app))
        .collect::<Vec<_>>()
        .join(" && ");
    run_apt(&format!("{} && {commands}", prepare_apt_command()), 0, 100, quiet.clone())?;
    let installed = fs::read_to_string(root().join("var/lib/dpkg/status"))
        .map(|status| installed_packages(&status))
        .context("could not read the Debian package status")?;
    anyhow::ensure!(
        apps.iter().all(|app| app
            .required_packages()
            .iter()
            .all(|package| installed.contains(*package))),
        "A requested optional app is not fully installed after apt completed"
    );
    if apps.contains(&OptionalApp::Steam) {
        install_steam(quiet)?;
    }
    Ok(())
}

/// Electron launcher follow-up for first-run setup.
pub fn prepare_electron_launchers_for_setup(apps: &[OptionalApp]) -> anyhow::Result<()> {
    for &app in apps.iter().filter(|app| app.is_electron()) {
        prepare_electron_launcher(app)?;
    }
    Ok(())
}
