//! Switching workspaces inside the running Portal.
//!
//! Portal stays open: a worker stops the guest session, points the process
//! at the other workspace and hands back to the event loop, which rebuilds
//! the Wayland backend and starts that workspace's Plasma the way a system
//! update does. The veil stays up throughout and lifts once the new desktop
//! has presented a frame.

use crate::android::utils::{compose_overlay, webview_handoff};
use crate::core::{config, workspaces};
use jni::objects::{JObject, JString};
use jni::sys::{jboolean, jstring};
use jni::JNIEnv;
use std::path::Path;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// How long the old session may take to stop before the switch gives up and
/// that workspace starts again.
const STOP_BUDGET: Duration = Duration::from_secs(15);

pub enum SwitchResult {
    Switched(workspaces::Workspace),
    /// The old workspace is still the active one.
    Failed(String),
}

#[derive(Default)]
struct SwitchState {
    /// The workspace being opened, from the request until the event loop
    /// takes the result.
    target: Option<String>,
    result: Option<SwitchResult>,
    /// Shown on the card until the next request.
    error: Option<String>,
}

static STATE: Mutex<SwitchState> = Mutex::new(SwitchState {
    target: None,
    result: None,
    error: None,
});

fn app_files() -> &'static Path {
    Path::new(config::APP_FILES_ROOT)
}

/// Whether a switch owns the guest: no session, update, repair or app
/// install may start meanwhile.
pub fn in_progress() -> bool {
    STATE
        .lock()
        .map(|state| state.target.is_some())
        .unwrap_or(true)
}

/// Why a switch can't start now, in words for the card.
fn blocker() -> Option<&'static str> {
    if crate::android::proot::system_updates::update_in_progress() {
        Some("Wait for the update to finish first.")
    } else if crate::android::proot::optional_apps::is_busy() {
        Some("Wait for the apps to finish installing first.")
    } else if crate::android::proot::setup::anland_repair_running() {
        Some("Wait for the graphics repair to finish first.")
    } else {
        None
    }
}

/// Start switching to `id`. Returns false, with the reason on the card, when
/// it can't start.
pub fn request(id: &str) -> bool {
    let target = {
        let Ok(mut state) = STATE.lock() else {
            return false;
        };
        if state.target.is_some() {
            return false;
        }
        let checked = match blocker() {
            Some(reason) => Err(reason.to_owned()),
            None => workspaces::Registry::read(app_files())
                .map_err(|error| {
                    log::error!("Workspace registry unreadable for a switch: {error:#}");
                    "Portal couldn't read its workspace list.".to_owned()
                })
                .and_then(|registry| {
                    registry
                        .switch_target(app_files(), id)
                        .map_err(|error| error.to_string())
                }),
        };
        match checked {
            Ok(target) => {
                state.target = Some(target.id.clone());
                state.error = None;
                target
            }
            Err(reason) => {
                log::warn!("Workspace switch to {id} refused: {reason}");
                state.error = Some(reason);
                drop(state);
                publish();
                return false;
            }
        }
    };
    crate::android::diagnostics::host_event("workspace", &format!("switch-start id={}", target.id));
    publish();
    let spawned = thread::Builder::new()
        .name("workspace-switch".into())
        .spawn(move || {
            let result = stop_and_switch(&target.id);
            if let Ok(mut state) = STATE.lock() {
                state.result = Some(result);
            }
            webview_handoff::wake_event_loop();
        });
    if let Err(error) = spawned {
        log::error!("Could not start the workspace switch: {error}");
        if let Ok(mut state) = STATE.lock() {
            state.target = None;
            state.error = Some("Portal couldn't start the switch.".into());
        }
        publish();
        return false;
    }
    true
}

fn stop_and_switch(id: &str) -> SwitchResult {
    let started = Instant::now();
    let app = crate::android::utils::application_context::get_application_context().android_app;
    compose_overlay::notify_desktop_suspended(&app);
    crate::android::proot::launch::stop();
    // `stop` bounds its own wait; the session must be gone before the
    // process points at another system.
    while crate::android::proot::launch::is_running() {
        if started.elapsed() > STOP_BUDGET {
            return SwitchResult::Failed("Plasma didn't stop, so Portal stayed here.".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    let stopped = started.elapsed();
    match workspaces::switch_to(app_files(), id, workspaces::now_unix()) {
        Ok(workspace) => {
            crate::android::diagnostics::host_event(
                "workspace",
                &format!(
                    "switched id={} stop_ms={}",
                    workspace.id,
                    stopped.as_millis()
                ),
            );
            SwitchResult::Switched(workspace)
        }
        Err(error) => {
            log::error!("Workspace switch to {id} failed: {error:#}");
            SwitchResult::Failed(match error.downcast_ref::<workspaces::WorkspaceError>() {
                Some(error) => error.to_string(),
                None => "Portal couldn't save the switch.".into(),
            })
        }
    }
}

/// Consume the worker's result. From here a session may start again.
pub fn take_result() -> Option<SwitchResult> {
    let mut state = STATE.lock().ok()?;
    let result = state.result.take()?;
    state.target = None;
    if let SwitchResult::Failed(reason) = &result {
        state.error = Some(reason.clone());
    }
    Some(result)
}

/// Point the host's libxkbcommon at the new workspace's keyboard data, as
/// `main` does at startup. Runs on the event loop while no guest session
/// runs and before the next backend builds its keyboard.
pub fn point_host_libraries_at_rootfs() {
    let rootfs = config::rootfs();
    for (name, path) in [
        ("XKB_CONFIG_ROOT", rootfs.join("usr/share/X11/xkb")),
        ("XLOCALEDIR", rootfs.join("usr/share/X11/locale")),
    ] {
        if std::env::var_os(name).as_deref() != Some(path.as_os_str()) {
            std::env::set_var(name, path);
        }
    }
}

/// The Workspaces card's state: the registry listing plus the switch in
/// flight and the last refusal.
fn state_json() -> String {
    let mut listing: serde_json::Value =
        serde_json::from_str(&workspaces::Registry::load(app_files()).listing_json())
            .unwrap_or_else(|_| serde_json::json!({ "workspaces": [] }));
    if let Ok(state) = STATE.lock() {
        listing["switching"] = serde_json::json!(state.target);
        listing["error"] = serde_json::json!(state.error);
    }
    listing.to_string()
}

/// Send the card its state.
pub fn publish() {
    compose_overlay::publish_workspaces_state(&state_json());
}

fn read_string(env: &mut JNIEnv, value: &JString) -> Option<String> {
    match env.get_string(value) {
        Ok(value) => Some(value.to_string_lossy().into_owned()),
        Err(error) => {
            log::error!("Could not read a workspace argument: {error}");
            let _ = env.exception_clear();
            None
        }
    }
}

/// JNI: the card's state, read when it opens.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeWorkspaces(
    env: JNIEnv,
    _class: JObject,
) -> jstring {
    match env.new_string(state_json()) {
        Ok(value) => value.into_raw(),
        Err(error) => {
            log::error!("Could not allocate the workspace listing: {error}");
            let _ = env.exception_clear();
            std::ptr::null_mut()
        }
    }
}

/// JNI: open another workspace. The card shows progress from the state
/// updates that follow.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeSwitchWorkspace(
    mut env: JNIEnv,
    _class: JObject,
    id: JString,
) -> jboolean {
    let Some(id) = read_string(&mut env, &id) else {
        return 0;
    };
    u8::from(request(&id))
}

/// JNI: rename a workspace. Returns null on success, or the reason it was
/// refused.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeRenameWorkspace(
    mut env: JNIEnv,
    _class: JObject,
    id: JString,
    name: JString,
) -> jstring {
    let (Some(id), Some(name)) = (read_string(&mut env, &id), read_string(&mut env, &name)) else {
        return std::ptr::null_mut();
    };
    let refusal = match workspaces::edit(app_files(), |registry| registry.rename(&id, &name)) {
        Ok(_) => None,
        Err(error) => Some(match error.downcast_ref::<workspaces::WorkspaceError>() {
            Some(error) => error.to_string(),
            None => {
                log::error!("Could not rename workspace {id}: {error:#}");
                "Portal couldn't save the name.".to_owned()
            }
        }),
    };
    publish();
    match refusal {
        None => std::ptr::null_mut(),
        Some(reason) => env
            .new_string(reason)
            .map(|value| value.into_raw())
            .unwrap_or_else(|_| {
                let _ = env.exception_clear();
                std::ptr::null_mut()
            }),
    }
}

/// JNI: choose the workspace Portal opens at launch; an empty id means the
/// last one used.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeSetOpenAtLaunch(
    mut env: JNIEnv,
    _class: JObject,
    id: JString,
) -> jboolean {
    let Some(id) = read_string(&mut env, &id) else {
        return 0;
    };
    let target = (!id.is_empty()).then_some(id.as_str());
    let saved = workspaces::edit(app_files(), |registry| registry.set_open_at_launch(target));
    if let Err(error) = &saved {
        log::error!("Could not set the launch workspace: {error:#}");
    }
    publish();
    u8::from(saved.is_ok())
}
