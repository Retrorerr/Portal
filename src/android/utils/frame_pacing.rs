//! Choreographer-aligned redraw scheduling for the Android-owned surface.
//!
//! Wayland traffic remains event-driven. This layer only coalesces an already
//! requested redraw onto Android's next display frame callback so Portal does
//! not enter a vsynced EGL swap arbitrarily early and wait in the buffer queue.

use crate::android::accessibility::AppUserEvent;
use libloading::Library;
use std::{
    ffi::{c_longlong, c_void},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use winit::event_loop::EventLoopProxy;

type Choreographer = c_void;
type GetInstance = unsafe extern "C" fn() -> *mut Choreographer;
type PostFrameCallback64 =
    unsafe extern "C" fn(*mut Choreographer, extern "C" fn(c_longlong, *mut c_void), *mut c_void);

struct CallbackState {
    pending: AtomicBool,
    proxy: EventLoopProxy<AppUserEvent>,
}

/// Main-thread Android frame scheduler. If the NDK Choreographer API is not
/// available, callers transparently fall back to winit's immediate redraw.
pub struct AndroidFramePacer {
    choreographer: *mut Choreographer,
    post_frame_callback: PostFrameCallback64,
    state: Arc<CallbackState>,
    // Retain the symbol owner for the lifetime of the function pointer.
    _library: Library,
}

impl AndroidFramePacer {
    /// Resolve the API dynamically so the APK remains loadable below API 29.
    /// Portal's Pad 3 path uses `AChoreographer_postFrameCallback64`; older
    /// Android versions retain the immediate event-driven fallback.
    pub fn new(proxy: EventLoopProxy<AppUserEvent>) -> Option<Self> {
        let library = unsafe { Library::new("libandroid.so") }.ok()?;
        let get_instance = unsafe {
            *library
                .get::<GetInstance>(b"AChoreographer_getInstance\0")
                .ok()?
        };
        let post_frame_callback = unsafe {
            *library
                .get::<PostFrameCallback64>(b"AChoreographer_postFrameCallback64\0")
                .ok()?
        };
        let choreographer = unsafe { get_instance() };
        if choreographer.is_null() {
            return None;
        }
        Some(Self {
            choreographer,
            post_frame_callback,
            state: Arc::new(CallbackState {
                pending: AtomicBool::new(false),
                proxy,
            }),
            _library: library,
        })
    }

    /// Queue at most one callback. Returns true when Choreographer owns the
    /// redraw request, including when a callback was already pending.
    pub fn request_redraw(&self) -> bool {
        if self
            .state
            .pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return true;
        }
        let callback_state = Arc::into_raw(self.state.clone()) as *mut c_void;
        unsafe {
            (self.post_frame_callback)(
                self.choreographer,
                choreographer_frame_callback,
                callback_state,
            );
        }
        true
    }
}

extern "C" fn choreographer_frame_callback(frame_time_ns: c_longlong, data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let state = unsafe { Arc::from_raw(data.cast::<CallbackState>()) };
    state.pending.store(false, Ordering::Release);
    let _ = state.proxy.send_event(AppUserEvent::ChoreographerFrame {
        frame_time_ns: frame_time_ns.max(0),
    });
}

/// Give the NativeActivity thread Android's standard display priority after
/// guest/audio workers have been spawned, so they retain normal scheduling.
/// Failure is harmless and leaves the platform default unchanged.
pub fn prioritize_current_render_thread() {
    const THREAD_PRIORITY_DISPLAY: libc::c_int = -4;
    let result = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, THREAD_PRIORITY_DISPLAY) };
    if result != 0 {
        log::warn!(
            "Could not apply Android display-thread priority: {}",
            std::io::Error::last_os_error()
        );
    }
}
