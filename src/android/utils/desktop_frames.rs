//! Counts the buffers Portal presents to the desktop surface.
//!
//! The Compose glass reads the desktop with `PixelCopy`, which runs on the
//! UI's RenderThread. Sampling on a timer alone copied the whole desktop 30
//! times a second while a Portal control was dragged over a still desktop,
//! delaying the control's own frames. The samplers compare this count
//! instead and copy only after the desktop changed.

use std::sync::atomic::{AtomicI64, Ordering};

use jni::objects::JClass;
use jni::sys::jlong;
use jni::JNIEnv;

static PRESENTED: AtomicI64 = AtomicI64::new(0);

/// Records one buffer queued to the desktop surface by either renderer.
pub fn presented() {
    PRESENTED.fetch_add(1, Ordering::Release);
}

/// Zero until the first frame; callers then sample as if the desktop changed.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_SessionControls_nativeDesktopFrames(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    PRESENTED.load(Ordering::Acquire)
}
