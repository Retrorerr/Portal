//! Software-keyboard bridge for the Android NativeActivity.
//!
//! Winit's NativeActivity backend does not expose an Android `InputConnection`, so simply
//! calling `request_redraw` cannot make the on-screen keyboard appear. `SoftKeyboardBridge` owns a
//! 1x1 editor view on Android's UI thread and forwards committed text to this queue. The Wayland
//! event loop drains the queue and translates the supported ASCII subset into key events; the
//! queue is also useful to a future text-input-v3 implementation for full Unicode commits.

use jni::{
    objects::{JObject, JString},
    JNIEnv,
};
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use winit::platform::android::activity::AndroidApp;

use crate::android::utils::ndk::run_in_jvm;

static COMMITS: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();

fn commits() -> &'static Mutex<VecDeque<String>> {
    COMMITS.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn activity(android_app: &AndroidApp) -> JObject<'static> {
    unsafe { JObject::from_raw(android_app.activity_as_ptr() as *mut _) }
}

fn call_bridge(android_app: &AndroidApp, method: &str) -> Result<(), String> {
    run_in_jvm(
        |env, app| {
            let activity = activity(app);
            env.call_static_method(
                "app/polarbear/SoftKeyboardBridge",
                method,
                "(Landroid/app/Activity;)V",
                &[(&activity).into()],
            )
            .map(|_| ())
            .map_err(|error| format!("SoftKeyboardBridge.{method}: {error}"))
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
}

/// Drain committed text from Android's input connection.
pub fn drain_commits() -> Vec<String> {
    commits()
        .lock()
        .map(|mut queue| queue.drain(..).collect())
        .unwrap_or_default()
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
    if text.is_empty() {
        return;
    }
    if let Ok(mut queue) = commits().lock() {
        // Prevent a stuck IME from growing the queue without bound. Individual commits can be
        // large (e.g. a pasted paragraph), so bound by entries rather than bytes and preserve
        // the newest text, which is the one Android just committed.
        const MAX_PENDING_COMMITS: usize = 128;
        if queue.len() >= MAX_PENDING_COMMITS {
            queue.pop_front();
        }
        queue.push_back(text);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    #[test]
    fn commit_queue_capacity_policy_is_bounded() {
        let mut queue = VecDeque::new();
        for i in 0..129 {
            if queue.len() >= 128 {
                queue.pop_front();
            }
            queue.push_back(i);
        }
        assert_eq!(queue.len(), 128);
        assert_eq!(queue.front(), Some(&1));
        assert_eq!(queue.back(), Some(&128));
    }
}
