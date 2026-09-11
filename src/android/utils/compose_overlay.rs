//! SPIKE-ONLY (branch `compose-setup-spike`): Rust side of the native Kotlin +
//! Jetpack Compose fullscreen setup overlay.
//!
//! The overlay is a `ComposeView` added to the existing NativeActivity content
//! root (same Activity, same window). The winit event loop is never recreated
//! and the native Wayland surface underneath is never touched: showing the
//! overlay defers the first resume behind an explicit `Start Plasma` action,
//! and the first actually-presented desktop frame dismisses it with a fade.
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

/// Whether the overlay has ever been shown in this process (first-resume gate latch).
static SHOWN: AtomicBool = AtomicBool::new(false);
/// Rust-side view of overlay visibility. Cleared when dismissal is requested
/// and confirmed by the Kotlin removal callback.
static OPEN: AtomicBool = AtomicBool::new(false);
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
    let name = env.new_string("app.polarbear.ComposeOverlay")?;
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

/// Whether the overlay is currently expected on screen.
pub fn is_open() -> bool {
    OPEN.load(Ordering::Acquire)
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
    if !SPIKE_USE_COMPOSE || SHOWN.load(Ordering::Acquire) || has_start_requested() {
        return false;
    }
    is_webview || !renderer_active
}

/// Show the fullscreen Compose overlay (idempotent; UI-thread posted).
pub fn show_compose_overlay(android_app: &AndroidApp) {
    SHOWN.store(true, Ordering::Release);
    OPEN.store(true, Ordering::Release);
    READY_NOTIFIED.store(false, Ordering::Release);
    if let Ok(mut cached) = cached_app().lock() {
        *cached = Some(android_app.clone());
    }
    super::ndk::run_in_jvm(
        |env, app| {
            let activity = activity_object(app);
            match overlay_class(env, &activity) {
                Ok(class) => {
                    if let Err(error) = env.call_static_method(
                        class,
                        "show",
                        "(Landroid/app/Activity;)Z",
                        &[JValue::Object(&activity)],
                    ) {
                        log::error!("Compose overlay show failed: {error}");
                        clear_exception(env, "show");
                    }
                }
                Err(error) => {
                    log::error!("Compose overlay class is unavailable: {error}");
                    clear_exception(env, "find ComposeOverlay");
                }
            }
        },
        android_app.clone(),
    );
    log_blur_support(android_app);
    crate::android::diagnostics::host_event("compose-spike", "overlay-shown");
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

/// Immediately remove the overlay without animation (recovery swaps).
pub fn hide_compose_overlay(android_app: &AndroidApp) {
    OPEN.store(false, Ordering::Release);
    super::ndk::run_in_jvm(
        |env, app| {
            let activity = activity_object(app);
            match overlay_class(env, &activity) {
                Ok(class) => {
                    if let Err(error) = env.call_static_method(
                        class,
                        "hide",
                        "(Landroid/app/Activity;)V",
                        &[JValue::Object(&activity)],
                    ) {
                        log::error!("Compose overlay hide failed: {error}");
                        clear_exception(env, "hide");
                    }
                }
                Err(error) => {
                    log::error!("Compose overlay class is unavailable: {error}");
                    clear_exception(env, "find ComposeOverlay");
                }
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
    if !is_open() || READY_NOTIFIED.swap(true, Ordering::AcqRel) {
        return;
    }
    log::info!("compose-spike: desktop frame presented; overlay to Desktop ready");
    crate::android::diagnostics::host_event("compose-spike", "desktop-ready overlay-dismissing");
    set_compose_state(android_app, STATE_READY);
    // The Kotlin side fades (600ms) then removes the hierarchy and confirms
    // via nativeOnOverlayRemoved. Clear the Rust gate now so later resumes
    // take the normal native path while the fade finishes.
    OPEN.store(false, Ordering::Release);
}

/// Event-loop-free variant for readiness paths that run off the winit thread
/// (Anland consumer). Uses the activity cached at show time; no-op if the
/// overlay was never shown or already dismissed.
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

/// JNI confirmation that the Compose hierarchy was removed on the UI thread.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ComposeOverlay_nativeOnOverlayRemoved(
    _env: JNIEnv,
    _class: JObject,
) {
    OPEN.store(false, Ordering::Release);
    log::info!("compose-spike: overlay hierarchy removed; native surface undisturbed");
    crate::android::diagnostics::host_event("compose-spike", "overlay-removed");
}
