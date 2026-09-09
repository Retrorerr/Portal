//! Hidden `ANativeWindow` dequeue/queue API (Android-only).
//!
//! Rust mirror of `third_party/anland/consumer/anw_hidden.h`
//! (lfdevs/anland-termux, GPL-3.0). The NDK exposes `ANativeWindow` as opaque;
//! the per-slot BufferQueue API (`dequeueBuffer`/`queueBuffer` with sync-file
//! fences) is reached through dlsym'd `libnativewindow.so` symbols plus the
//! struct `perform()` op, exactly like the reference consumer.
//!
//! Layouts must match the platform ABI exactly; see the vendored header.

use std::ffi::c_void;
use std::os::raw::c_int;

pub const QUERY_MIN_UNDEQUEUED_BUFFERS: c_int = 3;
pub const FORMAT_RGBA_8888: c_int = 1; // == AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM

const ANW_API_CONNECT: c_int = 13;
const ANW_API_DISCONNECT: c_int = 14;
const ANW_API_CPU: c_int = 2;

#[repr(C)]
pub struct NativeHandle {
    pub version: c_int,
    pub num_fds: c_int,
    pub num_ints: c_int,
    // data[numFds + numInts] follows inline; fds first.
}

#[repr(C)]
struct AndroidNativeBase {
    magic: c_int,
    version: c_int,
    reserved: [*mut c_void; 4],
    incref: Option<unsafe extern "C" fn(*mut AndroidNativeBase)>,
    decref: Option<unsafe extern "C" fn(*mut AndroidNativeBase)>,
}

#[repr(C)]
pub struct ANativeWindowBuffer {
    common: AndroidNativeBase,
    pub width: i32,
    pub height: i32,
    pub stride: i32,
    pub format: i32,
    pub usage_deprecated: i32,
    pub layer_count: usize,
    reserved: [*mut c_void; 1],
    pub handle: *const NativeHandle,
    pub usage: u64,
    _reserved_proc: [*mut c_void; 7],
}

type PerformFn = Option<unsafe extern "C" fn(*mut AnwWindow, c_int, ...) -> c_int>;

#[repr(C)]
struct AnwWindow {
    common: AndroidNativeBase,
    flags: u32,
    min_swap_interval: c_int,
    max_swap_interval: c_int,
    xdpi: f32,
    ydpi: f32,
    oem: [isize; 4],
    set_swap_interval: Option<unsafe extern "C" fn(*mut AnwWindow, c_int) -> c_int>,
    _dequeue_deprecated: Option<*mut c_void>,
    _lock_deprecated: Option<*mut c_void>,
    _queue_deprecated: Option<*mut c_void>,
    query: Option<unsafe extern "C" fn(*const AnwWindow, c_int, *mut c_int) -> c_int>,
    perform: PerformFn,
    _cancel_deprecated: Option<*mut c_void>,
    dequeue_buffer: Option<
        unsafe extern "C" fn(*mut AnwWindow, *mut *mut ANativeWindowBuffer, *mut c_int) -> c_int,
    >,
    queue_buffer:
        Option<unsafe extern "C" fn(*mut AnwWindow, *mut ANativeWindowBuffer, c_int) -> c_int>,
    cancel_buffer:
        Option<unsafe extern "C" fn(*mut AnwWindow, *mut ANativeWindowBuffer, c_int) -> c_int>,
}

#[link(name = "nativewindow")]
extern "C" {
    pub fn ANativeWindow_acquire(window: *mut c_void);
    pub fn ANativeWindow_release(window: *mut c_void);
    pub fn ANativeWindow_getWidth(window: *mut c_void) -> i32;
    pub fn ANativeWindow_getHeight(window: *mut c_void) -> i32;
    pub fn ANativeWindow_setBuffersGeometry(
        window: *mut c_void,
        width: i32,
        height: i32,
        format: i32,
    ) -> i32;
}

/// dlsym'd hidden entry points. `Send` (function pointers are).
pub struct AnwApi {
    set_buffer_count:
        libloading::Symbol<'static, unsafe extern "C" fn(*mut c_void, usize) -> c_int>,
    query: libloading::Symbol<
        'static,
        unsafe extern "C" fn(*const c_void, c_int, *mut c_int) -> c_int,
    >,
    dequeue_buffer: libloading::Symbol<
        'static,
        unsafe extern "C" fn(*mut c_void, *mut *mut ANativeWindowBuffer, *mut c_int) -> c_int,
    >,
    queue_buffer: libloading::Symbol<
        'static,
        unsafe extern "C" fn(*mut c_void, *mut ANativeWindowBuffer, c_int) -> c_int,
    >,
    cancel_buffer: libloading::Symbol<
        'static,
        unsafe extern "C" fn(*mut c_void, *mut ANativeWindowBuffer, c_int) -> c_int,
    >,
}

unsafe impl Send for AnwApi {}
unsafe impl Sync for AnwApi {}

impl AnwApi {
    /// Load the hidden symbols. The library handle is intentionally leaked
    /// into `'static` (same rationale as `gl_import.rs`): the symbols must
    /// outlive every session and Android never unloads libnativewindow.
    pub unsafe fn load() -> Result<Self, String> {
        let lib = libloading::Library::new("libnativewindow.so")
            .map_err(|e| format!("dlopen libnativewindow.so: {e}"))?;
        let leaked: &'static libloading::Library = Box::leak(Box::new(lib));
        macro_rules! sym {
            ($name:literal) => {
                leaked
                    .get($name)
                    .map_err(|e| format!("dlsym {:?}: {e}", $name))?
            };
        }
        Ok(Self {
            set_buffer_count: sym!(b"ANativeWindow_setBufferCount"),
            query: sym!(b"ANativeWindow_query"),
            dequeue_buffer: sym!(b"ANativeWindow_dequeueBuffer"),
            queue_buffer: sym!(b"ANativeWindow_queueBuffer"),
            cancel_buffer: sym!(b"ANativeWindow_cancelBuffer"),
        })
    }

    pub unsafe fn api_connect(window: *mut c_void, api: c_int) -> c_int {
        let w = window as *mut AnwWindow;
        match (*w).perform {
            Some(perform) => perform(w, ANW_API_CONNECT, api),
            None => -1,
        }
    }

    pub unsafe fn api_disconnect(window: *mut c_void, api: c_int) -> c_int {
        let w = window as *mut AnwWindow;
        match (*w).perform {
            Some(perform) => perform(w, ANW_API_DISCONNECT, api),
            None => -1,
        }
    }

    pub unsafe fn set_buffer_count(&self, window: *mut c_void, count: usize) -> c_int {
        (self.set_buffer_count)(window, count)
    }

    pub unsafe fn query_min_undequeued(&self, window: *mut c_void) -> Result<c_int, String> {
        let mut value: c_int = 0;
        let r = (self.query)(
            window as *const c_void,
            QUERY_MIN_UNDEQUEUED_BUFFERS,
            &mut value,
        );
        if r != 0 {
            return Err(format!("ANativeWindow_query failed: {r}"));
        }
        Ok(value)
    }

    /// Blocking dequeue. Returns (buffer, acquire fence or -1).
    /// The acquire fence must be waited on (or closed) by the caller.
    pub unsafe fn dequeue(
        &self,
        window: *mut c_void,
    ) -> Result<(*mut ANativeWindowBuffer, c_int), c_int> {
        let mut buf: *mut ANativeWindowBuffer = std::ptr::null_mut();
        let mut fence: c_int = -1;
        let r = (self.dequeue_buffer)(window, &mut buf, &mut fence);
        if r != 0 || buf.is_null() {
            if fence >= 0 {
                libc::close(fence);
            }
            return Err(r);
        }
        Ok((buf, fence))
    }

    pub unsafe fn queue(
        &self,
        window: *mut c_void,
        buf: *mut ANativeWindowBuffer,
        fence: c_int,
    ) -> c_int {
        (self.queue_buffer)(window, buf, fence)
    }

    pub unsafe fn cancel(
        &self,
        window: *mut c_void,
        buf: *mut ANativeWindowBuffer,
        fence: c_int,
    ) -> c_int {
        (self.cancel_buffer)(window, buf, fence)
    }

    /// Read the dma-buf fd + stride out of a dequeued buffer.
    /// Returns (dup-able fd value, stride in pixels, w, h).
    pub unsafe fn buffer_dma_info(buf: *mut ANativeWindowBuffer) -> Option<(c_int, i32, i32, i32)> {
        let b = &*buf;
        if b.handle.is_null() {
            return None;
        }
        let h = &*b.handle;
        if h.num_fds < 1 {
            return None;
        }
        let fd = *(h.data_ptr());
        Some((fd, b.stride, b.width, b.height))
    }
}

impl NativeHandle {
    unsafe fn data_ptr(&self) -> *const c_int {
        (self as *const NativeHandle).add(1) as *const c_int
    }
}
