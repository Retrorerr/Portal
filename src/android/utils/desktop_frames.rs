//! Counts the buffers Portal presents to the desktop surface.
//!
//! The Compose glass reads the desktop with `PixelCopy`, which runs on the
//! UI's RenderThread. Sampling on a timer alone copied the whole desktop 30
//! times a second while a Portal control was dragged over a still desktop,
//! delaying the control's own frames. The samplers compare this count
//! instead and copy only after the desktop changed.

use std::ffi::c_void;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

use jni::objects::{JClass, JObject};
use jni::sys::{jboolean, jlong};
use jni::JNIEnv;
use winit::platform::android::activity::ndk::native_window::NativeWindow;

static PRESENTED: AtomicI64 = AtomicI64::new(0);

/// The window that received the most recent desktop frame.
static RENDERED_WINDOW: AtomicUsize = AtomicUsize::new(0);

/// Records one buffer queued to the desktop surface by either renderer.
pub fn presented() {
    PRESENTED.fetch_add(1, Ordering::Release);
}

/// Records a frame the desktop rendered, queued to `window`. Buffers queued
/// only to set a window up hold no desktop and are not recorded.
pub fn rendered_on(window: *mut c_void) {
    RENDERED_WINDOW.store(window as usize, Ordering::Release);
}

/// Whether the return animation can reveal this surface's desktop.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_SessionControls_nativeDesktopRenderedOn(
    env: JNIEnv,
    _class: JClass,
    surface: JObject,
) -> jboolean {
    // SAFETY: Kotlin passes a live android.view.Surface reference. NativeWindow
    // acquires and releases its own reference to the underlying window.
    let window = unsafe { NativeWindow::from_surface(env.get_raw(), surface.as_raw()) };
    u8::from(window.is_some_and(|window| {
        RENDERED_WINDOW.load(Ordering::Acquire) == window.ptr().as_ptr() as usize
    }))
}

/// Zero until the first frame; callers then sample as if the desktop changed.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_SessionControls_nativeDesktopFrames(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    PRESENTED.load(Ordering::Acquire)
}
