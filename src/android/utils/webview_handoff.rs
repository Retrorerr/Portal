//! In-process control for the provisioning WebView.
//!
//! The NativeActivity does not need to be recreated when setup finishes. The popup's Looper is
//! retained as a JVM global reference and can be asked to quit from the setup worker; the winit
//! event-loop proxy then swaps to the Wayland backend. Keeping the references here also makes the
//! handoff safe across a configuration change: the old popup is closed before a new one is shown.

use super::ndk::run_in_jvm;
use crate::android::accessibility::AppUserEvent;
use jni::{
    objects::{GlobalRef, JObject},
    JNIEnv,
};
use std::sync::{atomic::{AtomicBool, Ordering}, Mutex, OnceLock};
use winit::{
    event_loop::EventLoopProxy,
    platform::android::activity::AndroidApp,
};

#[derive(Default)]
struct PopupControl {
    looper: Option<GlobalRef>,
    popup: Option<GlobalRef>,
}

static CONTROL: OnceLock<Mutex<PopupControl>> = OnceLock::new();
static EVENT_LOOP_PROXY: OnceLock<Mutex<Option<EventLoopProxy<AppUserEvent>>>> = OnceLock::new();
static SETUP_COMPLETE: AtomicBool = AtomicBool::new(false);

fn control() -> &'static Mutex<PopupControl> {
    CONTROL.get_or_init(|| Mutex::new(PopupControl::default()))
}

fn event_loop_proxy() -> &'static Mutex<Option<EventLoopProxy<AppUserEvent>>> {
    EVENT_LOOP_PROXY.get_or_init(|| Mutex::new(None))
}

/// Register the process-lifetime proxy before setup starts. The completion callback can then
/// wake the waiting winit loop even while the WebView owns a separate Android Looper.
pub fn register_event_loop_proxy(proxy: EventLoopProxy<AppUserEvent>) {
    if let Ok(mut current) = event_loop_proxy().lock() {
        *current = Some(proxy);
    }
}

/// Wake the lifecycle owner after a popup or WebSocket state transition.
///
/// The same proxy is used for setup completion and runtime recovery actions, so an event-loop in
/// `ControlFlow::Wait` cannot miss a button press received by the WebSocket reader thread.
pub fn wake_event_loop() {
    let proxy = event_loop_proxy()
        .lock()
        .ok()
        .and_then(|proxy| proxy.clone());
    if let Some(proxy) = proxy {
        if let Err(error) = proxy.send_event(AppUserEvent::AccessibilityInputReady) {
            log::debug!("Failed to wake event loop after WebView handoff: {error}");
        }
    }
}

/// Mark setup complete and ask the popup Looper to return. The handoff event is sent only after
/// the popup thread has dismissed its window, so a newly created Wayland surface cannot be hidden
/// by a stale WebView. If no popup exists (for example, a very fast test/setup path), wake the
/// event loop immediately.
pub fn complete_setup(android_app: AndroidApp) {
    SETUP_COMPLETE.store(true, Ordering::Release);
    if !request_close(android_app) {
        wake_event_loop();
    }
}

/// Consume the completion bit from the event-loop thread.
pub fn take_setup_complete() -> bool {
    SETUP_COMPLETE.swap(false, Ordering::AcqRel)
}

/// Register the Looper and PopupWindow owned by the WebView thread.
///
/// Replacing the old references is deliberate. Android may deliver `resumed` more than once;
/// stale global references must not prevent the current activity from being closed.
pub fn install(env: &mut JNIEnv<'_>, looper: &JObject<'_>, popup: &JObject<'_>) -> bool {
    let Ok(looper) = env.new_global_ref(looper) else {
        log::error!("Failed to retain WebView Looper global reference");
        return false;
    };
    let Ok(popup) = env.new_global_ref(popup) else {
        log::error!("Failed to retain WebView PopupWindow global reference");
        return false;
    };
    if let Ok(mut control) = control().lock() {
        control.looper = Some(looper);
        control.popup = Some(popup);
        if SETUP_COMPLETE.load(Ordering::Acquire) {
            if let Some(looper) = control.looper.as_ref() {
                let _ = env.call_method(looper, "quitSafely", "()V", &[]);
            }
        }
        true
    } else {
        false
    }
}

/// Whether a provisioning/recovery popup currently owns a Looper.
pub fn is_open() -> bool {
    control()
        .lock()
        .map(|control| control.looper.is_some())
        .unwrap_or(false)
}

/// Ask the popup thread's Looper to return. The popup remains owned by the WebView thread and is
/// dismissed there, after all Java calls are serialized on that thread.
pub fn request_close(android_app: AndroidApp) -> bool {
    let looper = control()
        .lock()
        .ok()
        .and_then(|control| control.looper.clone());
    let Some(looper) = looper else { return false };
    run_in_jvm(
        move |env, _| {
            env.call_method(&looper, "quitSafely", "()V", &[])
                .map(|_| true)
                .unwrap_or_else(|error| {
                    log::warn!("Failed to close WebView Looper: {error}");
                    false
                })
        },
        android_app,
    )
}

/// Clear global references after the WebView Looper exits.
pub fn clear() {
    if let Ok(mut control) = control().lock() {
        control.looper = None;
        control.popup = None;
    }
    if SETUP_COMPLETE.load(Ordering::Acquire) {
        wake_event_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_control_starts_closed() {
        // This remains a state-policy test; popup creation and closure are exercised on-device.
        assert!(!is_open());
    }
}
