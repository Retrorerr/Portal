//! Android pointer capture while a guest game holds a pointer lock.
//!
//! The host cursor is absolute: it stops at the screen edges (so a game's
//! view stops turning there) and stays visible over the game. KWin sends
//! `CONSUMER_VAR_CAPTURE_MOUSE` while a client holds a zwp_locked_pointer_v1
//! lock; Portal then takes Android pointer capture, which hides the system
//! cursor and delivers raw relative mouse motion with no edges.
//! `PortalActivity` hands the captured motion, buttons and wheel back through
//! the natives below, straight into the Anland session.
//!
//! Requests come from the Anland event thread, so the JNI call runs on a
//! dedicated worker, like `display_mode_request`.

use std::sync::{mpsc, Mutex, OnceLock};

use jni::objects::{JClass, JObject, JValue};
use jni::sys::{_jobject, jboolean, jfloat, jint};
use jni::JNIEnv;
use winit::platform::android::activity::AndroidApp;

use crate::android::anland::AnlandPointerSink;

fn app_slot() -> &'static Mutex<Option<AndroidApp>> {
    static APP: OnceLock<Mutex<Option<AndroidApp>>> = OnceLock::new();
    APP.get_or_init(|| Mutex::new(None))
}

fn sink_slot() -> &'static Mutex<Option<AnlandPointerSink>> {
    static SINK: OnceLock<Mutex<Option<AnlandPointerSink>>> = OnceLock::new();
    SINK.get_or_init(|| Mutex::new(None))
}

/// Logical px per raw mouse count: KWin divides host deltas by its output
/// scale, so scaling them up first gives games one logical px per count, as
/// on a desktop. Read when capture starts.
static GAIN: Mutex<f32> = Mutex::new(1.0);

fn worker() -> Option<&'static Mutex<mpsc::Sender<bool>>> {
    static WORKER: OnceLock<Option<Mutex<mpsc::Sender<bool>>>> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<bool>();
            std::thread::Builder::new()
                .name("portal-capture".into())
                .spawn(move || {
                    while let Ok(mut capture) = rx.recv() {
                        // Only the newest request matters.
                        while let Ok(next) = rx.try_recv() {
                            capture = next;
                        }
                        apply(capture);
                    }
                })
                .map_err(|error| log::warn!("pointer capture worker unavailable: {error}"))
                .ok()
                .map(|_| Mutex::new(tx))
        })
        .as_ref()
}

/// Remember the activity and session that captured input goes to.
pub fn attach(android_app: &AndroidApp, sink: AnlandPointerSink) {
    if let Ok(mut slot) = app_slot().lock() {
        *slot = Some(android_app.clone());
    }
    if let Ok(mut slot) = sink_slot().lock() {
        *slot = Some(sink);
    }
}

/// Take (or release) Android pointer capture. Non-blocking.
pub fn request(capture: bool) {
    if capture {
        let scale = crate::android::backend::wayland::output_state::read_kwin_output_scale()
            .unwrap_or(1.0) as f32;
        if let Ok(mut gain) = GAIN.lock() {
            *gain = scale.max(1.0);
        }
    }
    if let Some(tx) = worker().and_then(|tx| tx.lock().ok()) {
        let _ = tx.send(capture);
    }
}

fn apply(capture: bool) {
    let Some(app) = app_slot().lock().ok().and_then(|slot| slot.clone()) else {
        return;
    };
    super::ndk::run_in_jvm(
        |env, app| {
            let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as *mut _jobject) };
            if let Err(error) = env.call_method(
                activity,
                "setPointerCaptureWanted",
                "(Z)V",
                &[JValue::Bool(capture.into())],
            ) {
                log::warn!("pointer capture request (capture={capture}) failed: {error}");
                let _ = env.exception_clear();
            } else {
                log::info!("anland.input pointer capture wanted={capture}");
            }
        },
        app,
    );
}

fn with_sink(send: impl FnOnce(&AnlandPointerSink)) {
    if let Some(sink) = sink_slot().lock().ok().and_then(|slot| slot.clone()) {
        send(&sink);
    }
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_PortalActivity_nativeCapturedMotion(
    _env: JNIEnv,
    _class: JClass,
    dx: jfloat,
    dy: jfloat,
) {
    let gain = GAIN.lock().map(|gain| *gain).unwrap_or(1.0);
    with_sink(|sink| sink.send_relative(dx * gain, dy * gain));
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_PortalActivity_nativeCapturedButton(
    _env: JNIEnv,
    _class: JClass,
    code: jint,
    pressed: jboolean,
) {
    with_sink(|sink| sink.send_button(code as u32, pressed != 0));
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_PortalActivity_nativeCapturedWheel(
    _env: JNIEnv,
    _class: JClass,
    horizontal: jfloat,
    vertical: jfloat,
) {
    with_sink(|sink| {
        // Android counts wheel-up as positive; Wayland scrolls down on positive.
        if vertical != 0.0 {
            sink.send_wheel(0, -vertical);
        }
        if horizontal != 0.0 {
            sink.send_wheel(1, horizontal);
        }
    });
}
