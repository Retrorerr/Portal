//! SPIKE-ONLY (branch `spike/game-activity-host`): Rust side of the Kotlin +
//! Jetpack Compose fullscreen setup overlay.
//!
//! The overlay is a `ComposeView` in a fullscreen sibling `FrameLayout` added
//! to GameActivity's own root (directly above the native
//! `InputEnabledSurfaceView`). Same window, same Activity: no `PopupWindow`,
//! no second window. The winit event loop is never recreated and the native
//! surface underneath is never touched: showing the overlay defers the first
//! resume behind an explicit `Start Plasma` action, and the first
//! actually-presented desktop frame dismisses it with a fade.
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
//! Threading: every method here runs on the winit event-loop thread and
//! forwards to the Android UI thread via `Activity.runOnUiThread` inside the
//! Kotlin object. The button callback runs on the UI thread and does nothing
//! but set an atomic and wake the event loop through the already-registered
//! [`super::webview_handoff`] proxy.

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
/// - `Dismissing`: fade-out requested; the hierarchy is still alive until
///   Kotlin confirms removal, so resume/recovery paths must not treat this
///   as fully gone.
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

/// First-resume gate latch: the deferred-Start protocol runs at most once per
/// process. A failed show still consumes the gate so startup falls through
/// to the normal fallback paths instead of stalling.
static GATE_FIRED: AtomicBool = AtomicBool::new(false);
/// Explicit `Start Plasma` action from the overlay button (consumed once).
static START_REQUESTED: AtomicBool = AtomicBool::new(false);
/// Desktop-ready dismissal already sent (sent exactly once per overlay session).
static READY_NOTIFIED: AtomicBool = AtomicBool::new(false);

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

/// Peek without consuming: has the explicit Start action arrived?
pub fn has_start_requested() -> bool {
    START_REQUESTED.load(Ordering::Acquire)
}

/// Consume one explicit `Start Plasma` action from the overlay button.
pub fn take_start_requested() -> bool {
    START_REQUESTED.swap(false, Ordering::AcqRel)
}

/// First-resume gate: show the overlay instead of auto-starting the runtime.
///
/// Returns true only once per process, when nothing is rendering yet: either
/// the backend still needs setup (WebView) or the Wayland renderer has not
/// been bound. Later resumes (rotation, suspend/resume, retry) always use the
/// normal paths so surface lifecycle, input, IME, and recovery keep working.
pub fn spike_should_gate(renderer_active: bool, is_webview: bool) -> bool {
    if !SPIKE_USE_COMPOSE || GATE_FIRED.load(Ordering::Acquire) || has_start_requested() {
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
    if state() != OverlayState::Hidden {
        // Re-show during Dismissing (e.g. a runtime failure inside the fade)
        // cancels teardown on the Kotlin side; Showing/Visible need nothing.
        if state() == OverlayState::Dismissing {
            set_state(OverlayState::Showing);
        } else {
            return;
        }
    } else {
        GATE_FIRED.store(true, Ordering::Release);
        set_state(OverlayState::Showing);
    }
    READY_NOTIFIED.store(false, Ordering::Release);
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
                        "show",
                        "(Landroid/app/Activity;)V",
                        &[JValue::Object(&activity)],
                    ) {
                        log::error!("Compose overlay show call failed: {error}");
                        clear_exception(env, "show");
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
    log_blur_support(android_app);
    crate::android::diagnostics::host_event("compose-spike", "overlay-show-requested");
}

/// Update the small overlay state text (`Idle` / `Starting` / `Desktop ready` / `Error`).
pub fn set_compose_state(android_app: &AndroidApp, state: &str) {
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

/// Smallest reliable desktop-ready signal consumer: the compositor calls this
/// exactly when Portal has produced AND presented a valid KWin desktop frame
/// (generation-safe readiness + Android EGL present). The overlay updates to
/// `Desktop ready`, fades away on the UI thread, and removes itself without
/// touching the native surface.
pub fn notify_desktop_ready(android_app: &AndroidApp) {
    if !is_up() || READY_NOTIFIED.swap(true, Ordering::AcqRel) {
        return;
    }
    log::info!("compose-spike: desktop frame presented; overlay to Desktop ready");
    crate::android::diagnostics::host_event("compose-spike", "desktop-ready overlay-dismissing");
    set_compose_state(android_app, STATE_READY);
    // The Kotlin side fades (600ms) then removes the hierarchy and confirms
    // via nativeOnOverlayRemoved, which is the only transition to Hidden.
    // Resumes during the fade take the normal native path; recovery paths
    // still observe the overlay as present until removal is confirmed.
    set_state(OverlayState::Dismissing);
}

/// Event-loop-free variant for readiness paths that run off the winit thread
/// (Anland consumer). Uses the activity cached at show time; no-op unless
/// the overlay currently owns the screen.
pub fn notify_desktop_ready_cached() {
    let app = cached_app().lock().ok().and_then(|app| app.clone());
    if let Some(app) = app {
        notify_desktop_ready(&app);
    }
}

/// Probe (and log) Android 12+ cross-window blur availability for the future
/// final transition. Investigative only; the spike never depends on it.
pub fn log_blur_support(android_app: &AndroidApp) -> Option<bool> {
    let supported: Option<bool> = super::ndk::run_in_jvm(
        |env, app| {
            let activity = activity_object(app);
            let class = match overlay_class(env, &activity) {
                Ok(class) => class,
                Err(_) => {
                    let _ = env.exception_clear();
                    return None;
                }
            };
            match env.call_static_method(
                class,
                "queryBlur",
                "(Landroid/app/Activity;)Z",
                &[JValue::Object(&activity)],
            ) {
                Ok(value) => value.z().ok(),
                Err(_) => {
                    let _ = env.exception_clear();
                    None
                }
            }
        },
        android_app.clone(),
    );
    match supported {
        Some(enabled) => {
            log::info!("compose-spike: WindowManager.isCrossWindowBlurEnabled()={enabled}");
            crate::android::diagnostics::host_event(
                "compose-spike",
                &format!("cross-window-blur={enabled}"),
            );
            Some(enabled)
        }
        None => {
            log::info!("compose-spike: cross-window blur probe unavailable (pre-31 or probe failed)");
            crate::android::diagnostics::host_event(
                "compose-spike",
                "cross-window-blur=unknown",
            );
            None
        }
    }
}

/// JNI callback from the overlay `Start Plasma` button (runs on the UI
/// thread: only set the action bit and wake the event loop).
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeOnStartPlasma(
    _env: JNIEnv,
    _class: JObject,
) {
    START_REQUESTED.store(true, Ordering::Release);
    log::info!("compose-spike: Start Plasma pressed; waking event loop");
    crate::android::diagnostics::host_event("compose-spike", "start-pressed");
    super::webview_handoff::wake_event_loop();
}

/// JNI acknowledgement: Kotlin successfully created and showed the sibling
/// overlay. Only this transitions `Showing` to `Visible`.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeOnOverlayShown(
    _env: JNIEnv,
    _class: JObject,
) {
    if READY_NOTIFIED.load(Ordering::Acquire) {
        // The desktop became ready before presentation completed; dismiss
        // immediately now that the hierarchy exists.
        set_state(OverlayState::Dismissing);
        if let Some(app) = cached_app().lock().ok().and_then(|app| app.clone()) {
            set_compose_state(&app, STATE_READY);
        }
        return;
    }
    if state() == OverlayState::Showing {
        set_state(OverlayState::Visible);
        log::info!("compose-spike: overlay presentation confirmed");
        crate::android::diagnostics::host_event("compose-spike", "overlay-visible");
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

fn on_overlay_show_failed(reason: &str) {
    log::error!("compose-spike: overlay show failed: {reason}");
    crate::android::diagnostics::host_event(
        "compose-spike",
        &format!("overlay-show-failed reason={reason}"),
    );
    if state() == OverlayState::Showing {
        // The gate stays consumed: the next resume/user event takes the
        // normal (HTML WebView / Wayland) path instead of stalling startup.
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
    log::info!("compose-spike: overlay hierarchy removed; native surface undisturbed");
    crate::android::diagnostics::host_event("compose-spike", "overlay-removed");
}
