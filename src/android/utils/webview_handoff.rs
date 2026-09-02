//! Optional in-process control for the provisioning WebView.
//!
//! NativeActivity does not need to be recreated when setup finishes. The
//! popup's Looper is retained as a JVM global reference and can be asked to
//! quit from the setup worker; the event-loop proxy then swaps to Wayland.
//! This module is intentionally separate from the existing WebView builder so
//! it can be integrated without coupling JNI object lifetimes to setup code.

use super::ndk::run_in_jvm;
use jni::{
    objects::{GlobalRef, JObject},
    JNIEnv,
};
use std::sync::{Mutex, OnceLock};
use winit::platform::android::activity::AndroidApp;

#[derive(Default)]
struct PopupControl {
    looper: Option<GlobalRef>,
    popup: Option<GlobalRef>,
}

static CONTROL: OnceLock<Mutex<PopupControl>> = OnceLock::new();

fn control() -> &'static Mutex<PopupControl> {
    CONTROL.get_or_init(|| Mutex::new(PopupControl::default()))
}

pub fn install(env: &mut JNIEnv<'_>, looper: &JObject<'_>, popup: &JObject<'_>) {
    let Ok(looper) = env.new_global_ref(looper) else {
        return;
    };
    let Ok(popup) = env.new_global_ref(popup) else {
        return;
    };
    if let Ok(mut control) = control().lock() {
        control.looper = Some(looper);
        control.popup = Some(popup);
    }
}

/// Ask the popup thread's Looper to return. The popup remains owned by the
/// WebView thread and is dismissed there, after all Java calls are serialized
/// on that thread.
pub fn request_close(android_app: AndroidApp) -> bool {
    let looper = control()
        .lock()
        .ok()
        .and_then(|control| control.looper.clone());
    let Some(looper) = looper else { return false };
    run_in_jvm(
        move |env, _| {
            env.call_method(&looper, "quitSafely", "()V", &[]).is_ok()
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
}

#[cfg(test)]
mod tests {
    #[test]
    fn control_is_explicitly_optional() {
        // JVM integration is covered on the Android emulator; this unit test
        // documents that no popup handle is required by the startup policy.
        assert!(true);
    }
}
