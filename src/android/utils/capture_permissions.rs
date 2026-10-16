//! Android RECORD_AUDIO / CAMERA for the guest's microphone and cameras.
//!
//! Portal asks only when a Linux app first records or opens a camera: the
//! capture helper (`localdesktop_pipewire_capture`) reports `demand`, and
//! `CapturePermissions.kt` shows Android's prompt, or answers at once when
//! the permission is already granted. The answer goes back to the helper
//! through `pipewire_standalone_aaudio::set_capture_permission`; without a
//! grant guest apps get silence or black frames instead of an error.
//!
//! Demands arrive on the helpers' log threads, so the JNI call runs on a
//! dedicated worker, like `pointer_capture`.

use std::sync::{mpsc, Mutex, OnceLock};

use jni::objects::{JClass, JObject, JValue};
use jni::sys::{_jobject, jboolean, jint};
use jni::JNIEnv;
use winit::platform::android::activity::AndroidApp;

use crate::android::backend::pipewire_standalone_aaudio::{self, CaptureKind};

fn app_slot() -> &'static Mutex<Option<AndroidApp>> {
    static APP: OnceLock<Mutex<Option<AndroidApp>>> = OnceLock::new();
    APP.get_or_init(|| Mutex::new(None))
}

fn worker() -> Option<&'static Mutex<mpsc::Sender<CaptureKind>>> {
    static WORKER: OnceLock<Option<Mutex<mpsc::Sender<CaptureKind>>>> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<CaptureKind>();
            std::thread::Builder::new()
                .name("portal-capture-perm".into())
                .spawn(move || {
                    while let Ok(kind) = rx.recv() {
                        ask(kind);
                    }
                })
                .map_err(|error| log::warn!("capture permission worker unavailable: {error}"))
                .ok()
                .map(|_| Mutex::new(tx))
        })
        .as_ref()
}

/// Remember the activity that shows the prompts.
pub fn attach(android_app: &AndroidApp) {
    if let Ok(mut slot) = app_slot().lock() {
        *slot = Some(android_app.clone());
    }
}

/// A guest app started using `kind`: ask Android (non-blocking).
pub fn request(kind: CaptureKind) {
    if let Some(tx) = worker().and_then(|tx| tx.lock().ok()) {
        let _ = tx.send(kind);
    }
}

fn ask(kind: CaptureKind) {
    let Some(app) = app_slot().lock().ok().and_then(|slot| slot.clone()) else {
        log::warn!("capture permission for {kind:?} requested before the activity attached");
        return;
    };
    super::ndk::run_in_jvm(
        |env, app| {
            let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as *mut _jobject) };
            if let Err(error) = env.call_method(
                activity,
                "requestCapturePermission",
                "(I)V",
                &[JValue::Int(kind as jint)],
            ) {
                log::warn!("capture permission request for {kind:?} failed: {error}");
                let _ = env.exception_clear();
            } else {
                log::info!("capture permission requested for {kind:?}");
            }
        },
        app,
    );
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_CapturePermissions_nativeCapturePermission(
    _env: JNIEnv,
    _class: JClass,
    kind: jint,
    granted: jboolean,
) {
    let Some(kind) = CaptureKind::from_index(kind) else {
        return;
    };
    log::info!("capture permission {kind:?} granted={}", granted != 0);
    pipewire_standalone_aaudio::set_capture_permission(kind, granted != 0);
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_CapturePermissions_nativeDisplayRotation(
    _env: JNIEnv,
    _class: JClass,
    degrees: jint,
) {
    pipewire_standalone_aaudio::set_display_rotation(degrees.rem_euclid(360) as u32);
}
