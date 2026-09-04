//! Software-keyboard bridge for the Android NativeActivity.
//!
//! Winit's NativeActivity backend does not expose an Android `InputConnection`, so simply
//! requesting a redraw cannot make the on-screen keyboard appear. `SoftKeyboardBridge` owns a
//! tiny editor view on Android's UI thread and forwards committed text to this queue. The winit
//! event loop drains the queue and translates the supported ASCII subset into physical key
//! events; unsupported Unicode commits are retained in bounded form and explicitly logged rather
//! than being mapped to a potentially wrong keyboard layout.

use jni::{
    objects::{JClass, JObject, JString},
    JNIEnv,
};
use std::sync::{
    atomic::{AtomicBool, AtomicI8, Ordering},
    Mutex, OnceLock,
};
use winit::{
    event_loop::{EventLoopClosed, EventLoopProxy},
    platform::android::activity::AndroidApp,
};

use crate::android::{accessibility::AppUserEvent, utils::ndk::run_in_jvm};
use crate::core::ime_policy::CommitQueue;

static COMMITS: OnceLock<Mutex<CommitQueue>> = OnceLock::new();
static EVENT_LOOP_PROXY: OnceLock<Mutex<Option<EventLoopProxy<AppUserEvent>>>> = OnceLock::new();
static WAYLAND_TEXT_INPUT_ACTIVE: AtomicBool = AtomicBool::new(false);
static HARDWARE_KEYBOARD_PRESENT: AtomicBool = AtomicBool::new(false);
// -1 hide, 0 unchanged, 1 show. The event-loop owns all JNI visibility calls.
static VISIBILITY_REQUEST: AtomicI8 = AtomicI8::new(0);

fn commits() -> &'static Mutex<CommitQueue> {
    COMMITS.get_or_init(|| Mutex::new(CommitQueue::default()))
}

fn event_loop_proxy() -> &'static Mutex<Option<EventLoopProxy<AppUserEvent>>> {
    EVENT_LOOP_PROXY.get_or_init(|| Mutex::new(None))
}

/// Register the winit proxy used to wake a waiting event loop after Android commits text.
/// Re-registering on a process-lifetime event loop is harmless and supports test/restart hosts.
pub fn register_event_loop_proxy(proxy: EventLoopProxy<AppUserEvent>) {
    if let Ok(mut current) = event_loop_proxy().lock() {
        *current = Some(proxy);
    }
}

/// Start authoritative Android InputManager monitoring. This is event-driven:
/// pogo/USB/Bluetooth keyboard hotplug wakes the existing winit loop.
pub fn start_hardware_keyboard_monitor(android_app: &AndroidApp) -> Result<(), String> {
    call_bridge(android_app, "startHardwareKeyboardMonitor")
}

pub fn set_wayland_text_input_active(active: bool) {
    WAYLAND_TEXT_INPUT_ACTIVE.store(active, Ordering::Release);
    request_visibility(active && !HARDWARE_KEYBOARD_PRESENT.load(Ordering::Acquire));
}

pub fn request_visibility(show: bool) {
    VISIBILITY_REQUEST.store(if show { 1 } else { -1 }, Ordering::Release);
    wake_event_loop();
}

pub fn take_visibility_request() -> Option<bool> {
    match VISIBILITY_REQUEST.swap(0, Ordering::AcqRel) {
        1 => Some(true),
        -1 => Some(false),
        _ => None,
    }
}

pub fn refresh_visibility() {
    request_visibility(
        WAYLAND_TEXT_INPUT_ACTIVE.load(Ordering::Acquire)
            && !HARDWARE_KEYBOARD_PRESENT.load(Ordering::Acquire),
    );
}

fn wake_event_loop() {
    let proxy = event_loop_proxy()
        .lock()
        .ok()
        .and_then(|proxy| proxy.clone());
    if let Some(proxy) = proxy {
        if let Err(error) = proxy.send_event(AppUserEvent::AccessibilityInputReady) {
            // A surface can be suspended while the IME sends its final commit. The queue is still
            // bounded and will be reset on suspend; this warning is useful without being fatal.
            log_proxy_error(error);
        }
    }
}

fn log_proxy_error(error: EventLoopClosed<AppUserEvent>) {
    log::debug!("Failed to wake event loop for software-keyboard input: {error}");
}

fn activity(android_app: &AndroidApp) -> JObject<'static> {
    unsafe { JObject::from_raw(android_app.activity_as_ptr() as *mut _) }
}

/// Resolve an application class through the Activity's class loader.
///
/// JNI calls made from a thread attached by the NativeActivity glue use the
/// bootstrap/native loader for string-based `call_static_method` lookups. That
/// loader cannot see classes packed in the app's classes.dex, so resolving the
/// bridge by its dotted name through `Activity.getClassLoader()` is required on
/// real Android devices (and still works with the test/runtime class loader).
fn bridge_class<'local>(
    env: &mut JNIEnv<'local>,
    activity: &JObject<'local>,
) -> Result<JClass<'local>, String> {
    let loader = env
        .call_method(activity, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(|value| value.l())
        .map_err(|error| format!("Activity.getClassLoader: {error}"))?;
    if loader.is_null() {
        return Err("Activity.getClassLoader returned null".to_string());
    }

    let class_name = env
        .new_string("app.polarbear.SoftKeyboardBridge")
        .map_err(|error| format!("allocate SoftKeyboardBridge class name: {error}"))?;
    let class = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[(&class_name).into()],
        )
        .and_then(|value| value.l())
        .map_err(|error| format!("ClassLoader.loadClass(SoftKeyboardBridge): {error}"))?;
    if class.is_null() {
        return Err("ClassLoader.loadClass returned null for SoftKeyboardBridge".to_string());
    }
    Ok(JClass::from(class))
}

fn clear_bridge_exception(env: &mut JNIEnv<'_>, context: &str) {
    if env.exception_check().unwrap_or(false) {
        log::error!("SoftKeyboardBridge {context} raised a Java exception");
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}

fn call_bridge(android_app: &AndroidApp, method: &str) -> Result<(), String> {
    run_in_jvm(
        |env, app| {
            let activity = activity(app);
            let class = match bridge_class(env, &activity) {
                Ok(class) => class,
                Err(error) => {
                    clear_bridge_exception(env, "class lookup");
                    return Err(error);
                }
            };
            env.call_static_method(
                &class,
                method,
                "(Landroid/app/Activity;)V",
                &[(&activity).into()],
            )
            .map(|_| ())
            .map_err(|error| {
                let message = format!("SoftKeyboardBridge.{method}: {error}");
                clear_bridge_exception(env, method);
                message
            })
        },
        android_app.clone(),
    )
}

/// Ask Android to create/focus the hidden editor and show the software keyboard.
pub fn show(android_app: &AndroidApp) -> Result<(), String> {
    call_bridge(android_app, "show")
}

/// Hide the software keyboard and release editor focus.
pub fn hide(android_app: &AndroidApp) -> Result<(), String> {
    call_bridge(android_app, "hide")
}

/// Clear any queued commits after a surface loss or focus transition.
pub fn reset() {
    if let Ok(mut queue) = commits().lock() {
        queue.clear();
    }
    VISIBILITY_REQUEST.store(0, Ordering::Release);
}

/// Drain committed text from Android's input connection.
pub fn drain_commits() -> Vec<String> {
    commits()
        .lock()
        .map(|mut queue| queue.drain())
        .unwrap_or_default()
}

fn enqueue_commit(text: String) -> bool {
    let Ok(mut queue) = commits().lock() else {
        return false;
    };
    queue.push(text)
}

/// JNI callback used by `SoftKeyboardBridge`.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_SoftKeyboardBridge_nativeOnTextCommit(
    mut env: JNIEnv,
    _bridge: JObject,
    text: JString,
) {
    let text = match env.get_string(&text) {
        Ok(text) => text.to_string_lossy().into_owned(),
        Err(error) => {
            log::warn!("Failed to decode software-keyboard commit: {error}");
            return;
        }
    };
    if enqueue_commit(text) {
        wake_event_loop();
    }
}

/// JNI callback from Android's InputDeviceListener. Device classification is
/// performed with InputDevice source/type flags, never device names.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_SoftKeyboardBridge_nativeOnHardwareKeyboardChanged(
    _env: JNIEnv,
    _bridge: JObject,
    present: jni::sys::jboolean,
) {
    let present = present != 0;
    log::info!("Android physical keyboard presence changed: {present}");
    HARDWARE_KEYBOARD_PRESENT.store(present, Ordering::Release);
    request_visibility(!present && WAYLAND_TEXT_INPUT_ACTIVE.load(Ordering::Acquire));
}
