//! Debug-only pointer automation for UI testing (`portal-debug` feature).
//!
//! Injects semantic pointer commands (move, button, finger scroll) into the
//! SAME production path as the physical touchpad: the event-loop thread
//! synthesizes the identical [`winit::event::WindowEvent`] values that real
//! hardware produces and feeds them to `forward_anland_input`, so automation
//! traverses the same evdev mapping, anchor-motion delta synthesis, Anland
//! wire encoding, and KWin `PointerInputRedirection` handling.
//!
//! Transport: the Kotlin debug receiver (`PortalActivity`, debuggable builds
//! only) calls the JNI entry points below, which enqueue and wake the winit
//! loop via [`AppUserEvent::DebugPointerReady`]. The queue is drained on the
//! event-loop thread next to the accessibility drain. Release builds never
//! construct commands (every JNI entry returns false) and never drain, so
//! normal input is untouched and the queue cannot grow.

use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
};

use jni::{
    objects::JObject,
    sys::{jboolean, jdouble, jint, JNI_FALSE, JNI_TRUE},
    JNIEnv,
};

use super::accessibility::{event_loop_proxy, AppUserEvent};

/// One semantic pointer operation in Android buffer px (unscaled, exactly
/// like the physical touchpad path; density/rotation transforms downstream
/// are reused, never duplicated here).
#[derive(Clone, Copy, Debug)]
pub enum DebugPointerCmd {
    Move { x: f64, y: f64 },
    Button { button: u8, pressed: bool },
    FingerScroll { x: f64, y: f64 },
    FingerStop,
}

#[derive(Default)]
struct DebugPointerState {
    pending: VecDeque<DebugPointerCmd>,
}

static DEBUG_POINTER: OnceLock<Mutex<DebugPointerState>> = OnceLock::new();

fn state() -> &'static Mutex<DebugPointerState> {
    DEBUG_POINTER.get_or_init(|| Mutex::new(DebugPointerState::default()))
}

/// Drain queued commands. Called on the winit event-loop thread.
pub fn drain_pending() -> VecDeque<DebugPointerCmd> {
    match state().lock() {
        Ok(mut guard) => std::mem::take(&mut guard.pending),
        Err(_) => VecDeque::new(),
    }
}

fn wake() {
    if let Some(proxy) = event_loop_proxy() {
        let _ = proxy.send_event(AppUserEvent::DebugPointerReady);
    }
}

fn push(cmd: DebugPointerCmd) -> bool {
    match state().lock() {
        Ok(mut guard) => {
            // Bounded: automation glitches must not pile unbounded input.
            if guard.pending.len() < 512 {
                guard.pending.push_back(cmd);
            }
        }
        Err(_) => return false,
    }
    wake();
    true
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_PortalActivity_nativeDebugPointerMove(
    _env: JNIEnv,
    _activity: JObject,
    x: jdouble,
    y: jdouble,
) -> jboolean {
    if push(DebugPointerCmd::Move {
        x: x as f64,
        y: y as f64,
    }) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_PortalActivity_nativeDebugPointerButton(
    _env: JNIEnv,
    _activity: JObject,
    button: jint,
    pressed: jboolean,
) -> jboolean {
    if push(DebugPointerCmd::Button {
        button: button.max(0) as u8,
        pressed: pressed == JNI_TRUE,
    }) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_PortalActivity_nativeDebugPointerScroll(
    _env: JNIEnv,
    _activity: JObject,
    x: jdouble,
    y: jdouble,
) -> jboolean {
    if push(DebugPointerCmd::FingerScroll {
        x: x as f64,
        y: y as f64,
    }) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_PortalActivity_nativeDebugPointerScrollStop(
    _env: JNIEnv,
    _activity: JObject,
) -> jboolean {
    if push(DebugPointerCmd::FingerStop) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}
