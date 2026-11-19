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
use jni::sys::{_jobject, jboolean, jfloat, jint, jlong};
use jni::JNIEnv;
use winit::dpi::PhysicalPosition;
use winit::event::{
    DeviceId, ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
use winit::platform::android::activity::AndroidApp;

use crate::android::accessibility::{event_loop_proxy, AppUserEvent};
use crate::android::anland::AnlandPointerSink;
use crate::core::android_input::android_keycode_to_scancode;

fn app_slot() -> &'static Mutex<Option<AndroidApp>> {
    static APP: Mutex<Option<AndroidApp>> = Mutex::new(None);
    &APP
}

fn sink_slot() -> &'static Mutex<Option<AnlandPointerSink>> {
    static SINK: Mutex<Option<AnlandPointerSink>> = Mutex::new(None);
    &SINK
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
        // Android's positive vertical delta means up, and its positive
        // horizontal delta means right. Match the uncaptured wheel path.
        if vertical != 0.0 {
            sink.send_wheel(0, -vertical);
        }
        if horizontal != 0.0 {
            sink.send_wheel(1, horizontal);
        }
    });
}

/// Input from the tablet control window follows the renderer's normal mouse path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProjectedPointer {
    Motion(f32, f32),
    Button(MouseButton, ElementState),
    Wheel(f32, f32),
    FingerScroll(f32, f32, TouchPhase),
    Pinch(TouchPhase, f32),
}

impl ProjectedPointer {
    pub fn window_event(self) -> WindowEvent {
        let device_id = DeviceId::dummy();
        match self {
            Self::Motion(x, y) => WindowEvent::CursorMoved {
                device_id,
                position: PhysicalPosition::new(x.into(), y.into()),
            },
            Self::Button(button, state) => WindowEvent::MouseInput {
                device_id,
                state,
                button,
            },
            // winit flips the Android horizontal axis before forwarding it.
            Self::Wheel(x, y) => WindowEvent::MouseWheel {
                device_id,
                delta: MouseScrollDelta::LineDelta(-x, y),
                phase: TouchPhase::Moved,
            },
            Self::FingerScroll(x, y, phase) => WindowEvent::MouseWheel {
                device_id,
                delta: MouseScrollDelta::PixelDelta(PhysicalPosition::new(x.into(), y.into())),
                phase,
            },
            Self::Pinch(phase, scale) => WindowEvent::AndroidTouchpadPinch {
                device_id,
                phase,
                scale: scale.into(),
            },
        }
    }
}

fn queue(event: AppUserEvent) {
    if let Some(proxy) = event_loop_proxy() {
        let _ = proxy.send_event(event);
    }
}

fn queue_pointer(pointer: ProjectedPointer, time_ns: jlong) {
    queue(AppUserEvent::ProjectedPointer { pointer, time_ns });
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_DesktopControlActivity_nativeControlMotion(
    _env: JNIEnv,
    _class: JClass,
    x: jfloat,
    y: jfloat,
    time_ns: jlong,
) {
    if x.is_finite() && y.is_finite() {
        queue_pointer(ProjectedPointer::Motion(x, y), time_ns);
    }
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_DesktopControlActivity_nativeControlButton(
    _env: JNIEnv,
    _class: JClass,
    code: jint,
    pressed: jboolean,
    time_ns: jlong,
) {
    let button = match code {
        0x110 => MouseButton::Left,
        0x111 => MouseButton::Right,
        0x112 => MouseButton::Middle,
        0x113 => MouseButton::Back,
        0x114 => MouseButton::Forward,
        _ => return,
    };
    let state = if pressed != 0 {
        ElementState::Pressed
    } else {
        ElementState::Released
    };
    queue_pointer(ProjectedPointer::Button(button, state), time_ns);
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_DesktopControlActivity_nativeControlWheel(
    _env: JNIEnv,
    _class: JClass,
    horizontal: jfloat,
    vertical: jfloat,
    time_ns: jlong,
) {
    if horizontal.is_finite() && vertical.is_finite() {
        queue_pointer(ProjectedPointer::Wheel(horizontal, vertical), time_ns);
    }
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_DesktopControlActivity_nativeControlFingerScroll(
    _env: JNIEnv,
    _class: JClass,
    horizontal: jfloat,
    vertical: jfloat,
    finished: jboolean,
    time_ns: jlong,
) {
    let phase = if finished == 0 {
        TouchPhase::Moved
    } else {
        TouchPhase::Ended
    };
    if horizontal.is_finite() && vertical.is_finite() {
        queue_pointer(
            ProjectedPointer::FingerScroll(horizontal, vertical, phase),
            time_ns,
        );
    }
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_DesktopControlActivity_nativeControlPinch(
    _env: JNIEnv,
    _class: JClass,
    phase: jint,
    scale: jfloat,
    time_ns: jlong,
) {
    let phase = match phase {
        0 => TouchPhase::Started,
        1 => TouchPhase::Moved,
        2 => TouchPhase::Ended,
        3 => TouchPhase::Cancelled,
        _ => return,
    };
    if scale.is_finite() {
        queue_pointer(ProjectedPointer::Pinch(phase, scale), time_ns);
    }
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_DesktopControlActivity_nativeControlKey(
    _env: JNIEnv,
    _class: JClass,
    keycode: jint,
    scan_code: jint,
    pressed: jboolean,
) {
    let scancode = android_keycode_to_scancode(keycode.max(0) as u32)
        .or_else(|| (1..=0x2ff).contains(&scan_code).then_some(scan_code as u32));
    if let Some(scancode) = scancode {
        queue(AppUserEvent::DesktopKey {
            scancode,
            down: pressed != 0,
        });
    }
}
