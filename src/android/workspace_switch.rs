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
                        .switch_target(app_files(), id, &workspaces::system_ready)
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

/// Stops the session for a switch: how long it took, or `None` when it was
/// still running at the end of [`STOP_BUDGET`].
fn stop_session() -> Option<Duration> {
    let started = Instant::now();
    let app = crate::android::utils::application_context::get_application_context().android_app;
    compose_overlay::notify_desktop_suspended(&app);
    crate::android::proot::launch::stop();
    // `stop` bounds its own wait; the session must be gone before the
    // process points at another system.
    while crate::android::proot::launch::is_running() {
        if started.elapsed() > STOP_BUDGET {
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
    Some(started.elapsed())
}

fn stop_and_switch(id: &str) -> SwitchResult {
    let Some(stopped) = stop_session() else {
        return SwitchResult::Failed("Plasma didn't stop, so Portal stayed here.".into());
    };
    match workspaces::switch_to(
        app_files(),
        id,
        workspaces::now_unix(),
        &workspaces::system_ready,
    ) {
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
    let listing = workspaces::Registry::load(app_files())
        .listing_json(app_files(), &workspaces::system_ready);
    let mut listing: serde_json::Value =
        serde_json::from_str(&listing).unwrap_or_else(|_| serde_json::json!({ "workspaces": [] }));
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
    let refusal = workspaces::edit(app_files(), |registry| registry.rename(&id, &name))
        .err()
        .map(|error| refusal(&error, "Portal couldn't save the name."));
    publish();
    string_result(&mut env, refusal)
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

/// Holds the guest for a restart into another workspace: from here no
/// session, update or app install starts in this process, which ends soon.
fn reserve_for_restart(target: &str) -> Result<(), String> {
    let Ok(mut state) = STATE.lock() else {
        return Err("Portal couldn't start the switch.".into());
    };
    if state.target.is_some() {
        return Err("Wait for the switch to finish first.".into());
    }
    if let Some(reason) = blocker() {
        return Err(reason.into());
    }
    state.target = Some(target.to_owned());
    state.error = None;
    Ok(())
}

/// A restart that couldn't be reserved: only the reason goes on the card,
/// since whatever holds the guest is someone else's.
fn refuse(reason: String) -> String {
    if let Ok(mut state) = STATE.lock() {
        state.error = Some(reason.clone());
    }
    publish();
    reason
}

/// Gives the guest back after a reserved restart was refused, with the
/// reason on the card.
fn release_restart(reason: String) -> String {
    if let Ok(mut state) = STATE.lock() {
        state.target = None;
        state.error = Some(reason.clone());
    }
    publish();
    reason
}

/// The words for a failed registry change.
fn refusal(error: &anyhow::Error, fallback: &str) -> String {
    match error.downcast_ref::<workspaces::WorkspaceError>() {
        Some(error) => error.to_string(),
        None => {
            log::error!("{fallback} ({error:#})");
            fallback.to_owned()
        }
    }
}

fn string_result(env: &mut JNIEnv, refusal: Option<String>) -> jstring {
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

/// Make a new workspace: an empty folder the next launch opens, where the
/// first-run installer sets up a fresh Debian of its own.
fn create(name: &str) -> Option<String> {
    if let Err(reason) = reserve_for_restart("new") {
        return Some(reason);
    }
    match workspaces::create(app_files(), name, workspaces::now_unix()) {
        Ok(workspace) => {
            if let Ok(mut state) = STATE.lock() {
                state.target = Some(workspace.id.clone());
            }
            crate::android::diagnostics::host_event(
                "workspace",
                &format!("create id={}", workspace.id),
            );
            publish();
            None
        }
        Err(error) => {
            // The name field shows why, so the card's error line stays clear.
            let reason = refusal(&error, "Portal couldn't make the workspace.");
            if let Ok(mut state) = STATE.lock() {
                state.target = None;
            }
            publish();
            Some(reason)
        }
    }
}

/// Open a workspace whose install hasn't finished, through a restart into
/// its installer.
fn set_up(id: &str) -> Option<String> {
    if let Err(reason) = reserve_for_restart(id) {
        return Some(refuse(reason));
    }
    match workspaces::request_setup(
        app_files(),
        id,
        workspaces::now_unix(),
        &workspaces::system_ready,
    ) {
        Ok(workspace) => {
            crate::android::diagnostics::host_event(
                "workspace",
                &format!("setup-resume id={}", workspace.id),
            );
            publish();
            None
        }
        Err(error) => Some(release_restart(refusal(
            &error,
            "Portal couldn't open that workspace.",
        ))),
    }
}

/// From the installer: give up the running workspace's unfinished setup,
/// delete its files and go back to a workspace that is set up. Blocking:
/// the delete can take a while.
fn abandon_setup() -> Option<String> {
    if !crate::android::proot::setup::installation_idle_or_failed() {
        return Some("Wait for the install to stop first.".into());
    }
    if let Err(reason) = reserve_for_restart("back") {
        return Some(refuse(reason));
    }
    match workspaces::abandon_setup(app_files(), &workspaces::system_ready) {
        Ok(back) => {
            crate::android::diagnostics::host_event(
                "workspace",
                &format!("setup-abandoned back={}", back.id),
            );
            None
        }
        Err(error) => Some(release_restart(refusal(
            &error,
            "Portal couldn't remove that workspace.",
        ))),
    }
}

/// Stop the running session cleanly before Portal restarts, so Plasma and
/// its apps save their settings. Blocking, bounded like a switch's stop.
/// False when it was still running at the end of that bound.
fn stop_for_restart() -> bool {
    if !crate::android::proot::launch::is_running() {
        return true;
    }
    let started = Instant::now();
    let app = crate::android::utils::application_context::get_application_context().android_app;
    compose_overlay::notify_desktop_suspended(&app);
    crate::android::proot::launch::stop();
    while crate::android::proot::launch::is_running() {
        if started.elapsed() > STOP_BUDGET {
            log::warn!("Plasma didn't stop before the restart; it ends with the process");
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
    crate::android::diagnostics::host_event(
        "workspace",
        &format!("restart-stop stop_ms={}", started.elapsed().as_millis()),
    );
    true
}

/// Delete a workspace, files and all. Deleting the open one moves Portal to
/// another workspace, or, as the last one, empties it and restarts into its
/// installer; nothing is deleted under a session that didn't stop.
fn delete(id: &str) -> Option<String> {
    let registry = match workspaces::Registry::read(app_files()) {
        Ok(registry) => registry,
        Err(error) => {
            log::error!("Workspace registry unreadable for a delete: {error:#}");
            return Some("Portal couldn't read its workspace list.".into());
        }
    };
    if id != registry.active {
        // A switch or restart in flight owns the registry's next step.
        if in_progress() {
            return Some("Wait for the switch to finish first.".into());
        }
        return match workspaces::delete(app_files(), id) {
            Ok(removed) => {
                crate::android::diagnostics::host_event(
                    "workspace",
                    &format!("deleted id={}", removed.id),
                );
                None
            }
            Err(error) => Some(refusal(&error, "Portal couldn't delete that workspace.")),
        };
    }
    // The open one: Portal moves to another workspace, in place when that
    // one is set up, else through a restart.
    let next = registry
        .after_deleting_active(app_files(), &workspaces::system_ready)
        .cloned();
    if let Some(next) = next
        .as_ref()
        .filter(|next| workspaces::is_set_up(app_files(), &next.id, &workspaces::system_ready))
    {
        return delete_open_and_switch(id, next.clone());
    }
    let expected = next.as_ref().map(|next| next.id.as_str());
    if let Err(reason) = reserve_for_restart(id) {
        return Some(refuse(reason));
    }
    if !stop_for_restart() {
        // The session still runs; it is the user's to keep.
        return Some(release_restart(
            "Plasma didn't stop, so nothing was deleted.".into(),
        ));
    }
    match workspaces::delete_running(
        app_files(),
        id,
        expected,
        workspaces::now_unix(),
        &workspaces::system_ready,
    ) {
        Ok(after) => {
            crate::android::diagnostics::host_event(
                "workspace",
                &format!("deleted-open id={id} after={after:?}"),
            );
            None
        }
        // Past the stop there is no session to go back to: Portal restarts
        // either way, into whatever the registry now says.
        Err(error) => {
            log::error!("Could not delete the open workspace {id}: {error:#}");
            None
        }
    }
}

/// Deletes the open workspace by switching to `next` as any switch does:
/// Portal stays open, the session stops, the registry drops the deleted
/// one and the event loop starts `next`. Its files go once the switch is
/// under way. Null when the switch has started, else why not.
fn delete_open_and_switch(id: &str, next: workspaces::Workspace) -> Option<String> {
    {
        let Ok(mut state) = STATE.lock() else {
            return Some("Portal couldn't start the switch.".into());
        };
        if state.target.is_some() {
            return Some("Wait for the switch to finish first.".into());
        }
        if let Some(reason) = blocker() {
            state.error = Some(reason.to_owned());
            drop(state);
            publish();
            return Some(reason.to_owned());
        }
        state.target = Some(next.id.clone());
        state.error = None;
    }
    crate::android::diagnostics::host_event(
        "workspace",
        &format!("delete-open-start id={id} next={}", next.id),
    );
    publish();
    let deleted = id.to_owned();
    let spawned = thread::Builder::new()
        .name("workspace-delete".into())
        .spawn(move || {
            let result = match stop_session() {
                None => SwitchResult::Failed("Plasma didn't stop, so nothing was deleted.".into()),
                Some(stopped) => match workspaces::delete_running(
                    app_files(),
                    &deleted,
                    Some(&next.id),
                    workspaces::now_unix(),
                    &workspaces::system_ready,
                ) {
                    Ok(workspaces::AfterDelete::Switched(workspace)) => {
                        workspaces::set_active_dir(workspaces::workspace_dir(
                            app_files(),
                            &workspace.id,
                        ));
                        crate::android::diagnostics::host_event(
                            "workspace",
                            &format!(
                                "deleted-open id={deleted} switched={} stop_ms={}",
                                workspace.id,
                                stopped.as_millis()
                            ),
                        );
                        SwitchResult::Switched(workspace)
                    }
                    Ok(after) => {
                        // Only a registry changed under the plan gets here.
                        log::error!("Deleting {deleted} did not switch as planned: {after:?}");
                        SwitchResult::Failed("Portal couldn't delete that workspace.".into())
                    }
                    Err(error) => SwitchResult::Failed(refusal(
                        &error,
                        "Portal couldn't delete that workspace.",
                    )),
                },
            };
            let switched = matches!(result, SwitchResult::Switched(_));
            if let Ok(mut state) = STATE.lock() {
                state.result = Some(result);
            }
            webview_handoff::wake_event_loop();
            if switched {
                let started = Instant::now();
                match workspaces::delete_files(app_files(), &deleted) {
                    Ok(()) => crate::android::diagnostics::host_event(
                        "workspace",
                        &format!(
                            "deleted-files id={deleted} ms={}",
                            started.elapsed().as_millis()
                        ),
                    ),
                    Err(error) => log::warn!(
                        "Workspace {deleted} was deleted but its files remain: {error:#}"
                    ),
                }
            }
        });
    if let Err(error) = spawned {
        log::error!("Could not start deleting the open workspace: {error}");
        if let Ok(mut state) = STATE.lock() {
            state.target = None;
            state.error = Some("Portal couldn't start the switch.".into());
        }
        publish();
        return Some("Portal couldn't start the switch.".into());
    }
    None
}

/// JNI: make a workspace. Null when Portal should now restart into it,
/// else why it can't.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeCreateWorkspace(
    mut env: JNIEnv,
    _class: JObject,
    name: JString,
) -> jstring {
    let refusal = match read_string(&mut env, &name) {
        Some(name) => create(&name),
        None => Some("Portal couldn't read the name.".into()),
    };
    string_result(&mut env, refusal)
}

/// JNI: open a workspace that isn't set up. Null when Portal should now
/// restart into its installer, else why it can't.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeSetUpWorkspace(
    mut env: JNIEnv,
    _class: JObject,
    id: JString,
) -> jstring {
    let refusal = match read_string(&mut env, &id) {
        Some(id) => set_up(&id),
        None => Some("Portal couldn't open that workspace.".into()),
    };
    string_result(&mut env, refusal)
}

/// JNI, off the main thread: give up the running workspace's setup. Null
/// when Portal should now restart into the workspace it goes back to.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeAbandonWorkspaceSetup(
    mut env: JNIEnv,
    _class: JObject,
) -> jstring {
    let refusal = abandon_setup();
    string_result(&mut env, refusal)
}

/// JNI, off the main thread: delete a workspace, files and all. Null when
/// deleted; for the open one, when the switch away has started (the state
/// names it in `switching`) or else Portal should now restart. Else why not.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeDeleteWorkspace(
    mut env: JNIEnv,
    _class: JObject,
    id: JString,
) -> jstring {
    let refusal = match read_string(&mut env, &id) {
        Some(id) => delete(&id),
        None => Some("Portal couldn't delete that workspace.".into()),
    };
    publish();
    string_result(&mut env, refusal)
}

/// JNI, off the main thread: stop the session before Portal restarts.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeStopForWorkspaceRestart(
    _env: JNIEnv,
    _class: JObject,
) {
    stop_for_restart();
}
