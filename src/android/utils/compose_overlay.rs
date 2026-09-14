//! SPIKE-ONLY (branch `spike/game-activity-host`): Rust side of the Kotlin +
//! Jetpack Compose fullscreen setup overlay.
//!
//! The overlay is a `ComposeView` in a fullscreen sibling `FrameLayout` added
//! to GameActivity's own root (directly above the native
//! `InputEnabledSurfaceView`). Same window, same Activity: no `PopupWindow`,
//! no second window. The winit event loop is never recreated and the native
//! surface underneath is never touched: the overlay is a veil over the native
//! desktop, which starts through the normal event-loop resume path. The first
//! actually-presented desktop frame only latches readiness for Compose; the
//! hierarchy remains until Compose confirms a committed reveal has finished.
//!
//! Lifecycle/SavedState/ViewModel ownership comes from the real
//! AppCompatActivity owners, so Compose follows the Activity lifecycle
//! automatically; there is no manual owner and no host resume/suspend
//! forwarding. Input needs no custom routing: the overlay is the topmost
//! View while visible and normal Android hit-testing applies.
//!
//! Visibility is an explicit [`OverlayState`] machine driven by Kotlin
//! acknowledgements: Rust marks `Showing` when construction is requested and
//! only treats the overlay as `Visible` after Kotlin confirms successful
//! presentation; `Hidden` is entered only when Kotlin confirms removal (or
//! reports that showing failed, in which case startup falls through to the
//! normal fallback paths).
//!
//! Threading: lifecycle methods here run on the winit event-loop thread and
//! forward to the Android UI thread via `Activity.runOnUiThread` inside the
//! Kotlin object. Anland can publish readiness from its render thread through
//! the cached `AndroidApp`; renderer/Wayland lifecycle work remains owned by
//! the winit event loop.

use jni::{
    objects::{JClass, JObject, JValue},
    sys::_jobject,
    JNIEnv,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};
use winit::platform::android::activity::AndroidApp;

/// Use the Compose overlay for setup/recovery screens on this spike branch.
/// The HTML WebView code paths are retained as fallback but are not shown
/// while this is true.
pub const SPIKE_USE_COMPOSE: bool = true;

/// Overlay state strings shown in the spike UI (must match the Kotlin side).
pub const STATE_IDLE: &str = "Idle";
pub const STATE_STARTING: &str = "Starting";
pub const STATE_READY: &str = "Desktop ready";
pub const STATE_ERROR: &str = "Error";

/// Dotted class name used with the Activity class loader (see [`overlay_class`]).
const OVERLAY_DOTTED_CLASS: &str = "app.polarbear.ComposeOverlay";

/// Explicit native-side overlay lifecycle.
///
/// - `Hidden`: nothing on screen; normal startup/recovery paths apply.
/// - `Showing`: Kotlin is constructing the popup; treated as screen-owning,
///   but not yet confirmed.
/// - `Visible`: Kotlin confirmed successful presentation.
/// - `Dismissing`: the user committed the final upward reveal; the hierarchy
///   is still alive until Kotlin confirms that it is completely offscreen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayState {
    Hidden,
    Showing,
    Visible,
    Dismissing,
}

fn overlay_state() -> &'static Mutex<OverlayState> {
    static STATE: OnceLock<Mutex<OverlayState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(OverlayState::Hidden))
}

fn state() -> OverlayState {
    overlay_state()
        .lock()
        .ok()
        .map(|state| *state)
        .unwrap_or(OverlayState::Hidden)
}

fn set_state(next: OverlayState) {
    if let Ok(mut state) = overlay_state().lock() {
        *state = next;
    }
}

/// Whether the overlay currently owns the screen (under construction or
/// confirmed visible). False once dismissal starts.
pub fn is_up() -> bool {
    matches!(state(), OverlayState::Showing | OverlayState::Visible)
}

/// Whether the overlay hierarchy is fully gone. This is the only state in
/// which resume/recovery paths may act as if no overlay exists; in
/// particular `Dismissing` still counts as present.
pub fn is_hidden() -> bool {
    state() == OverlayState::Hidden
}

/// First-resume presentation latch. A failed show still consumes the request;
/// native startup never waits on this bit.
static SHOW_REQUESTED: AtomicBool = AtomicBool::new(false);
/// Authoritative first-presented-valid-KWin-frame latch. This is independent
/// from setup UI readiness and never dismisses the overlay by itself.
static DESKTOP_READY: AtomicBool = AtomicBool::new(false);
/// Final reveal commit acknowledgement, guarded once until either removal or
/// a recovery re-show returns the veil to `Showing`.
static REVEAL_COMMITTED: AtomicBool = AtomicBool::new(false);

fn activity_object(android_app: &AndroidApp) -> JObject<'static> {
    unsafe { JObject::from_raw(android_app.activity_as_ptr() as *mut _jobject) }
}

/// Cached activity handle from the last overlay show, so readiness paths that
/// run off the event loop (e.g. the Anland consumer thread) can dismiss the
/// overlay without owning an `AndroidApp`.
static CACHED_APP: OnceLock<Mutex<Option<AndroidApp>>> = OnceLock::new();

fn cached_app() -> &'static Mutex<Option<AndroidApp>> {
    CACHED_APP.get_or_init(|| Mutex::new(None))
}

/// Resolve the overlay class through the Activity's class loader.
///
/// The winit event-loop thread is attached to the JVM manually, so plain
/// `FindClass` only sees the system class loader and throws
/// `ClassNotFoundException` for app classes. Going through
/// `Activity.getClassLoader().loadClass()` uses the APK loader instead.
fn overlay_class<'local>(
    env: &mut JNIEnv<'local>,
    activity: &JObject,
) -> jni::errors::Result<JClass<'local>> {
    let loader = env
        .call_method(
            activity,
            "getClassLoader",
            "()Ljava/lang/ClassLoader;",
            &[],
        )?
        .l()?;
    let name = env.new_string(OVERLAY_DOTTED_CLASS)?;
    let class = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&name)],
        )?
        .l()?;
    Ok(JClass::from(class))
}

fn clear_exception(env: &mut JNIEnv<'_>, context: &str) {
    if env.exception_check().unwrap_or(false) {
        log::error!("Compose overlay {context} raised a Java exception");
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}

fn publish_desktop_ready(android_app: &AndroidApp, ready: bool) {
    super::ndk::run_in_jvm(
        |env, app| {
            let activity = activity_object(app);
            let class = match overlay_class(env, &activity) {
                Ok(class) => class,
                Err(error) => {
                    log::error!("Compose overlay class is unavailable: {error}");
                    clear_exception(env, "find ComposeOverlay");
                    return;
                }
            };
            if let Err(error) = env.call_static_method(
                class,
                "updateDesktopReady",
                "(Z)V",
                &[JValue::Bool(ready.into())],
            ) {
                log::error!("Compose overlay updateDesktopReady failed: {error}");
                clear_exception(env, "updateDesktopReady");
            }
        },
        android_app.clone(),
    );
}

/// Publish the process-lifetime native provisioning snapshot. The snapshot is
/// retained by Kotlin even when the Compose hierarchy is absent, so Activity
/// recreation only reattaches the observer; it never starts or restarts work.
pub fn publish_install_state(
    android_app: &AndroidApp,
    snapshot: &crate::core::provisioning::ProvisioningSnapshot,
) {
    let phase = snapshot.phase.label().to_owned();
    let message = snapshot.message.clone();
    let error = snapshot.error.clone();
    let progress = snapshot.progress as i32;
    let complete = snapshot.phase == crate::core::provisioning::ProvisioningPhase::Complete;
    super::ndk::run_in_jvm(
        move |env, app| {
            let activity = activity_object(app);
            let class = match overlay_class(env, &activity) {
                Ok(class) => class,
                Err(error) => {
                    log::error!("Compose overlay class is unavailable: {error}");
                    clear_exception(env, "find ComposeOverlay for install state");
                    return;
                }
            };
            let phase = match env.new_string(phase) {
                Ok(value) => value,
                Err(error) => {
                    log::error!("Failed to allocate install phase string: {error}");
                    return;
                }
            };
            let message = match env.new_string(message) {
                Ok(value) => value,
                Err(error) => {
                    log::error!("Failed to allocate install message string: {error}");
                    return;
                }
            };
            let error_value = match error {
                Some(error) => match env.new_string(error) {
                    Ok(value) => JObject::from(value),
                    Err(error) => {
                        log::error!("Failed to allocate install error string: {error}");
                        JObject::null()
                    }
                },
                None => JObject::null(),
            };
            if let Err(error) = env.call_static_method(
                class,
                "updateInstallState",
                "(Ljava/lang/String;ILjava/lang/String;Ljava/lang/String;Z)V",
                &[
                    JValue::Object(&phase),
                    JValue::Int(progress),
                    JValue::Object(&message),
                    JValue::Object(&error_value),
                    JValue::Bool(complete.into()),
                ],
            ) {
                log::error!("Compose overlay updateInstallState failed: {error}");
                clear_exception(env, "updateInstallState");
            }
        },
        android_app.clone(),
    );
}

/// First-resume presentation decision. Showing the overlay does not gate or
/// defer native startup.
///
/// Returns true only once per process, when nothing is rendering yet: either
/// the backend still needs setup (WebView) or the Wayland renderer has not
/// been bound. Later resumes (rotation, suspend/resume, retry) always use the
/// normal paths so surface lifecycle, input, IME, and recovery keep working.
pub fn spike_should_show(renderer_active: bool, is_webview: bool) -> bool {
    if !SPIKE_USE_COMPOSE || SHOW_REQUESTED.load(Ordering::Acquire) {
        return false;
    }
    is_webview || !renderer_active
}

/// Show the fullscreen Compose overlay (UI-thread posted).
///
/// Marks `Showing` and returns immediately; `Visible` is entered only when
/// Kotlin acknowledges successful presentation, and a reported failure
/// returns to `Hidden` so startup proceeds via the fallback paths.
pub fn show_compose_overlay(android_app: &AndroidApp) {
    show_compose_overlay_with(android_app, "show");
}

/// Show the overlay in Return-to-Plasma mode for already-installed launches.
/// Same state machine, same host, same veil — Kotlin swaps the installer
/// screen for the minimal return screen with no launch intro.
pub fn show_compose_return(android_app: &AndroidApp) {
    show_compose_overlay_with(android_app, "showReturn");
}

fn show_compose_overlay_with(android_app: &AndroidApp, method: &str) {
    if state() != OverlayState::Hidden {
        // Re-show during Dismissing (e.g. a runtime failure inside the fade)
        // cancels teardown on the Kotlin side; Showing/Visible need nothing.
        if state() == OverlayState::Dismissing {
            REVEAL_COMMITTED.store(false, Ordering::Release);
            set_state(OverlayState::Showing);
        } else {
            return;
        }
    } else {
        SHOW_REQUESTED.store(true, Ordering::Release);
        REVEAL_COMMITTED.store(false, Ordering::Release);
        set_state(OverlayState::Showing);
    }
    if let Ok(mut cached) = cached_app().lock() {
        *cached = Some(android_app.clone());
    }
    super::ndk::run_in_jvm(
        |env, app| {
            let activity = activity_object(app);
            match overlay_class(env, &activity) {
                Ok(class) => {
                    // Fire-and-forget: success or failure is reported back
                    // through nativeOnOverlayShown / nativeOnOverlayShowFailed.
                    if let Err(error) = env.call_static_method(
                        class,
                        method,
                        "(Landroid/app/Activity;)V",
                        &[JValue::Object(&activity)],
                    ) {
                        log::error!("Compose overlay {method} call failed: {error}");
                        clear_exception(env, method);
                        on_overlay_show_failed("JNI show call failed");
                    }
                }
                Err(error) => {
                    log::error!("Compose overlay class is unavailable: {error}");
                    clear_exception(env, "find ComposeOverlay");
                    on_overlay_show_failed("ComposeOverlay class not found");
                }
            }
        },
        android_app.clone(),
    );
    crate::android::diagnostics::host_event("compose-spike", "overlay-show-requested");
    // Preserve a readiness signal that raced ahead of Kotlin presentation.
    // Kotlin stores it even when the ComposeView does not exist yet.
    if DESKTOP_READY.load(Ordering::Acquire) {
        publish_desktop_ready(android_app, true);
    }
    crate::android::proot::setup::publish_current_install_state(android_app);
}

/// Update the small overlay state text (`Idle` / `Starting` / `Desktop ready` / `Error`).
pub fn set_compose_state(android_app: &AndroidApp, state: &str) {
    if state == STATE_ERROR {
        DESKTOP_READY.store(false, Ordering::Release);
    }
    super::ndk::run_in_jvm(
        |env, app| {
            let activity = activity_object(app);
            let class = match overlay_class(env, &activity) {
                Ok(class) => class,
                Err(error) => {
                    log::error!("Compose overlay class is unavailable: {error}");
                    clear_exception(env, "find ComposeOverlay");
                    return;
                }
            };
            let value = match env.new_string(state) {
                Ok(value) => value,
                Err(error) => {
                    log::error!("Failed to allocate overlay state string: {error}");
                    return;
                }
            };
            if let Err(error) = env.call_static_method(
                class,
                "updateState",
                "(Ljava/lang/String;)V",
                &[JValue::Object(&value)],
            ) {
                log::error!("Compose overlay updateState failed: {error}");
                clear_exception(env, "updateState");
            }
        },
        android_app.clone(),
    );
}

/// Remove the Compose veil before showing the existing HTML runtime recovery
/// page. This is used only when installation has already committed but the
/// first Wayland/Anland handoff fails; it keeps that launch failure separate
/// from provisioning failure and does not touch the native surface.
pub fn dismiss_for_runtime_recovery(android_app: &AndroidApp) {
    if is_hidden() {
        return;
    }
    REVEAL_COMMITTED.store(false, Ordering::Release);
    set_state(OverlayState::Dismissing);
    let requested = super::ndk::run_in_jvm(
        |env, app| {
            let activity = activity_object(app);
            let class = match overlay_class(env, &activity) {
                Ok(class) => class,
                Err(error) => {
                    log::error!("Compose overlay class is unavailable: {error}");
                    clear_exception(env, "find ComposeOverlay for runtime recovery");
                    return false;
                }
            };
            if let Err(error) = env.call_static_method(
                class,
                "dismissForRuntimeRecovery",
                "(Landroid/app/Activity;)V",
                &[JValue::Object(&activity)],
            ) {
                log::error!("Compose overlay runtime recovery dismissal failed: {error}");
                clear_exception(env, "dismissForRuntimeRecovery");
                false
            } else {
                true
            }
        },
        android_app.clone(),
    );
    if !requested {
        set_state(OverlayState::Hidden);
        super::webview_handoff::wake_event_loop();
    }
}

/// Latch the authoritative native readiness proof: Portal produced and
/// presented a valid KWin desktop frame (generation-safe readiness plus the
/// Android present proof). This only publishes state to Compose. It never
/// translates, fades, dismisses, or removes the overlay.
pub fn notify_desktop_ready(android_app: &AndroidApp) {
    if DESKTOP_READY.swap(true, Ordering::AcqRel) {
        return;
    }
    log::info!("compose-spike: native desktop ready latched; overlay remains fully visible");
    crate::android::diagnostics::host_event("compose-spike", "desktop-ready overlay-retained");
    publish_desktop_ready(android_app, true);
}

/// Invalidate readiness when Android destroys the native window. If the
/// setup veil is still present, it returns to the quiet finishing state until
/// the replacement surface presents a new valid desktop frame.
pub fn notify_desktop_suspended(android_app: &AndroidApp) {
    if !DESKTOP_READY.swap(false, Ordering::AcqRel) {
        return;
    }
    log::info!("compose-spike: native desktop readiness cleared for surface suspend");
    crate::android::diagnostics::host_event("compose-spike", "desktop-ready-cleared suspend");
    publish_desktop_ready(android_app, false);
}

/// Event-loop-free variant for readiness paths that run off the winit thread
/// (Anland consumer). Uses the activity cached at overlay request time.
pub fn notify_desktop_ready_cached() {
    let app = cached_app().lock().ok().and_then(|app| app.clone());
    if let Some(app) = app {
        notify_desktop_ready(&app);
    }
}

/// JNI acknowledgement that the user committed the final reveal. Rust records
/// `Dismissing` here; native startup is already running and is not touched.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeOnRevealCommitted(
    _env: JNIEnv,
    _class: JObject,
) {
    if REVEAL_COMMITTED.swap(true, Ordering::AcqRel) {
        return;
    }
    if matches!(state(), OverlayState::Showing | OverlayState::Visible) {
        set_state(OverlayState::Dismissing);
        log::info!("compose-spike: final reveal committed; awaiting offscreen removal");
        crate::android::diagnostics::host_event("compose-spike", "reveal-committed");
    }
}

/// JNI acknowledgement: Kotlin successfully created and showed the sibling
/// overlay. Only this transitions `Showing` to `Visible`.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeOnOverlayShown(
    _env: JNIEnv,
    _class: JObject,
) {
    if state() == OverlayState::Showing {
        set_state(OverlayState::Visible);
        log::info!("compose-spike: overlay presentation confirmed");
        crate::android::diagnostics::host_event("compose-spike", "overlay-visible");
    }
    if DESKTOP_READY.load(Ordering::Acquire) {
        // A native frame can arrive between the show request and this
        // acknowledgement. Keep the overlay visible and publish the latch.
        if let Some(app) = cached_app().lock().ok().and_then(|app| app.clone()) {
            publish_desktop_ready(&app, true);
        }
    }
}

/// JNI acknowledgement: Kotlin failed to show the sibling overlay. Returns to `Hidden`
/// (without consuming anything else) so startup proceeds via the normal
/// fallback paths, and wakes the loop so it proceeds immediately.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeOnOverlayShowFailed(
    mut env: JNIEnv,
    _class: JObject,
    reason: jni::objects::JString,
) {
    let reason = env
        .get_string(&reason)
        .map(|reason| reason.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown".to_string());
    on_overlay_show_failed(&reason);
}

/// JNI button bridge for the first-run installer. This only asks the native
/// coordinator to start/attach; it does not perform any filesystem work on
/// the UI thread and cannot create a duplicate operation.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeBeginInstall(
    _env: JNIEnv,
    _class: JObject,
) -> jni::sys::jboolean {
    if crate::android::proot::setup::begin_install() {
        1
    } else {
        0
    }
}

/// JNI bridge for the explicit existing-install Anland graphics repair. The
/// native coordinator owns the worker and the event-loop handoff; this call
/// never performs Debian or graphics filesystem work on the Android UI thread.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeRepairEnableAnland(
    _env: JNIEnv,
    _class: JObject,
) -> jni::sys::jboolean {
    if crate::android::proot::setup::repair_enable_anland() {
        1
    } else {
        0
    }
}

fn on_overlay_show_failed(reason: &str) {
    log::error!("compose-spike: overlay show failed: {reason}");
    crate::android::diagnostics::host_event(
        "compose-spike",
        &format!("overlay-show-failed reason={reason}"),
    );
    if state() == OverlayState::Showing {
        // The show request stays consumed; native startup has already taken
        // the normal WebView/Wayland path and never waits on the overlay.
        set_state(OverlayState::Hidden);
    }
    super::webview_handoff::wake_event_loop();
}

/// JNI confirmation that the Compose hierarchy was removed on the UI thread.
/// This is the only transition to `Hidden` after a dismissal started.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeOnOverlayRemoved(
    _env: JNIEnv,
    _class: JObject,
) {
    set_state(OverlayState::Hidden);
    REVEAL_COMMITTED.store(false, Ordering::Release);
    log::info!("compose-spike: overlay hierarchy removed; native surface undisturbed");
    crate::android::diagnostics::host_event("compose-spike", "overlay-removed");
    super::webview_handoff::wake_event_loop();
}
