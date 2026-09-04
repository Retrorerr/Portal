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
static DESKTOP_INPUT_PRESENT: AtomicBool = AtomicBool::new(true);
// -1 hide, 0 unchanged, 1 show. The event-loop owns all JNI visibility calls.
static VISIBILITY_REQUEST: AtomicI8 = AtomicI8::new(0);

pub fn is_hardware_keyboard_present() -> bool {
    HARDWARE_KEYBOARD_PRESENT.load(Ordering::Acquire)
}

pub fn is_desktop_input_present() -> bool {
    DESKTOP_INPUT_PRESENT.load(Ordering::Acquire)
}

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
    let hw = HARDWARE_KEYBOARD_PRESENT.load(Ordering::Acquire);
    log::info!("Portal ime: set_wayland_text_input_active({active}), hw_present={hw}");
    WAYLAND_TEXT_INPUT_ACTIVE.store(active, Ordering::Release);
    request_visibility(active && !hw);
}

pub fn request_visibility(show: bool) {
    log::info!("Portal ime: request_visibility({show})");
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

pub fn is_wayland_text_input_active() -> bool {
    WAYLAND_TEXT_INPUT_ACTIVE.load(Ordering::Acquire)
}

static IME_CONTEXT_ACTIVE: AtomicBool = AtomicBool::new(false);
static IME_CMD_FIFO: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub fn is_ime_context_active() -> bool {
    IME_CONTEXT_ACTIVE.load(Ordering::Acquire)
}

pub fn set_ime_context_active_for_test(active: bool) {
    IME_CONTEXT_ACTIVE.store(active, Ordering::Release);
}

pub fn set_ime_cmd_file_for_test(file: Option<std::fs::File>) {
    if let Ok(mut guard) = IME_CMD_FIFO.lock() {
        *guard = file;
    }
}

pub fn send_ime_command(cmd: &str) -> bool {
    let mut guard = match IME_CMD_FIFO.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    if let Some(file) = guard.as_mut() {
        use std::io::Write;
        if file.write_all(cmd.as_bytes()).is_ok() && file.flush().is_ok() {
            return true;
        }
    }
    false
}

pub fn handle_fifo_line(line: &str) {
    let trimmed = line.trim();
    log::info!("Portal IME FIFO line received: '{trimmed}'");
    if trimmed == "1" || trimmed == "ACTIVATE" {
        IME_CONTEXT_ACTIVE.store(true, Ordering::Release);
        set_wayland_text_input_active(true);
    } else if trimmed == "0" || trimmed == "DEACTIVATE" {
        IME_CONTEXT_ACTIVE.store(false, Ordering::Release);
        set_wayland_text_input_active(false);
    }
}

pub fn dispatch_committed_text(text: String) -> bool {
    // 1. If an input-method context is active in KWin, dispatch via zwp_input_method_context_v1!
    if is_ime_context_active() {
        if text.chars().all(|c| c == '\x08') && !text.is_empty() {
            let count = text.len();
            if send_ime_command(&format!("DELETE:{count}\n")) {
                log::info!("Dispatched {count} backspace(s) via input-method context protocol");
                return true;
            }
        } else if text == "\n" || text == "\r\n" {
            if send_ime_command("ENTER\n") {
                log::info!("Dispatched enter key via input-method context protocol");
                return true;
            }
        } else {
            if let Ok(b64) = crate::core::clipboard_broker::encode_base64(text.as_bytes()) {
                if send_ime_command(&format!("COMMIT:{b64}\n")) {
                    log::info!("Dispatched commit_string via input-method context protocol: {text:?}");
                    return true;
                }
            }
        }
    }

    // 2. Fallback to evdev key synthesis for non-text clients or if protocol bridge is unready
    log::info!("Falling back to evdev key synthesis for text: {text:?}");
    if enqueue_commit(text) {
        wake_event_loop();
        true
    } else {
        false
    }
}

#[cfg(target_os = "android")]
static FIFO_LISTENER_STARTED: AtomicBool = AtomicBool::new(false);

pub fn start_ime_fifo_listener(rootfs_path: &std::path::Path) {
    #[cfg(target_os = "android")]
    {
        let events_fifo_path = rootfs_path.join("tmp/portal-ime-events.fifo");
        let commands_fifo_path = rootfs_path.join("tmp/portal-ime-commands.fifo");
        let legacy_fifo_path = rootfs_path.join("tmp/portal-ime.fifo");

        log::info!(
            "Ensuring Portal IME FIFOs (events: {}, commands: {})",
            events_fifo_path.display(),
            commands_fifo_path.display()
        );

        for path in [&events_fifo_path, &commands_fifo_path, &legacy_fifo_path] {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Some(path_str) = path.to_str() {
                if let Ok(c_path) = std::ffi::CString::new(path_str) {
                    unsafe {
                        libc::mkfifo(c_path.as_ptr(), 0o666);
                        libc::chmod(c_path.as_ptr(), 0o666);
                    }
                }
            }
        }

        if let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&commands_fifo_path)
        {
            if let Ok(mut guard) = IME_CMD_FIFO.lock() {
                *guard = Some(file);
            }
        }

        if !FIFO_LISTENER_STARTED.swap(true, Ordering::SeqCst) {
            std::thread::Builder::new()
                .name("portal-ime-fifo".to_string())
                .spawn(move || {
                    run_fifo_listener(&events_fifo_path);
                })
                .expect("Failed to spawn IME FIFO listener thread");
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = rootfs_path;
    }
}

#[cfg(target_os = "android")]
fn run_fifo_listener(fifo_path: &std::path::Path) {
    use std::io::BufRead;

    loop {
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(fifo_path)
        {
            Ok(f) => f,
            Err(err) => {
                log::warn!("Failed to open IME FIFO {}: {err}", fifo_path.display());
                std::thread::sleep(std::time::Duration::from_millis(500));
                continue;
            }
        };

        let mut reader = std::io::BufReader::new(file);
        let mut line = String::new();

        while let Ok(n) = reader.read_line(&mut line) {
            if n == 0 {
                break;
            }
            handle_fifo_line(&line);
            line.clear();
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    }
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
    dispatch_committed_text(text);
}

/// JNI callback from Android's InputDeviceListener. Device classification is
/// performed with InputDevice source/type flags, never device names.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_SoftKeyboardBridge_nativeOnInputDevicesChanged(
    _env: JNIEnv,
    _bridge: JObject,
    has_physical_keyboard: jni::sys::jboolean,
    has_desktop_input: jni::sys::jboolean,
) {
    let has_physical_keyboard = has_physical_keyboard != 0;
    let has_desktop_input = has_desktop_input != 0;
    log::info!(
        "Android input devices changed: has_physical_keyboard={has_physical_keyboard}, has_desktop_input={has_desktop_input}"
    );

    let prev_keyboard = HARDWARE_KEYBOARD_PRESENT.swap(has_physical_keyboard, Ordering::AcqRel);
    if prev_keyboard != has_physical_keyboard {
        request_visibility(!has_physical_keyboard && WAYLAND_TEXT_INPUT_ACTIVE.load(Ordering::Acquire));
    }

    let prev_desktop = DESKTOP_INPUT_PRESENT.swap(has_desktop_input, Ordering::AcqRel);
    if prev_desktop != has_desktop_input {
        crate::android::tablet_mode_manager::apply_kwin_tablet_mode(has_desktop_input);
    }
}

/// Backward-compatible JNI callback for older callers or tests.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_SoftKeyboardBridge_nativeOnHardwareKeyboardChanged(
    env: JNIEnv,
    bridge: JObject,
    present: jni::sys::jboolean,
) {
    Java_app_polarbear_SoftKeyboardBridge_nativeOnInputDevicesChanged(env, bridge, present, present);
}
