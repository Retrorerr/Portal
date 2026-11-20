//! Counts the buffers Portal presents to the desktop surface.
//!
//! The Compose glass reads the desktop with `PixelCopy`, which runs on the
//! UI's RenderThread. Sampling on a timer alone copied the whole desktop 30
//! times a second while a Portal control was dragged over a still desktop,
//! delaying the control's own frames. The samplers compare this count
//! instead and copy only after the desktop changed.

use std::ffi::c_void;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

use jni::objects::JClass;
use jni::sys::jlong;
use jni::JNIEnv;

static PRESENTED: AtomicI64 = AtomicI64::new(0);

/// The window the last rendered desktop frame went to, and how often that
/// changed. When the desktop comes back from a monitor, the phone's touchpad
/// page opens onto the phone's own window only once the desktop has rendered
/// a frame into it: until then that window still shows what it held before.
static RENDERED_WINDOW: AtomicUsize = AtomicUsize::new(0);
static WINDOW_CHANGES: AtomicI64 = AtomicI64::new(0);

/// Records one buffer queued to the desktop surface by either renderer.
pub fn presented() {
    PRESENTED.fetch_add(1, Ordering::Release);
}

/// Records a frame the desktop rendered, queued to `window`. Buffers queued
/// only to set a window up hold no desktop and are not recorded.
pub fn rendered_on(window: *mut c_void) {
    let window = window as usize;
    if RENDERED_WINDOW.swap(window, Ordering::AcqRel) != window {
        WINDOW_CHANGES.fetch_add(1, Ordering::Release);
    }
}

/// How often the desktop's rendered frames moved to another window.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_SessionControls_nativeDesktopWindowChanges(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    WINDOW_CHANGES.load(Ordering::Acquire)
}

/// Zero until the first frame; callers then sample as if the desktop changed.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_SessionControls_nativeDesktopFrames(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    PRESENTED.load(Ordering::Acquire)
}
