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

/// Public `ANativeWindow` entry points, resolved via dlsym like the hidden
/// ones (avoids a build-time `libnativewindow.so` dependency the APK packager
/// cannot satisfy; the library is always present on-device).
type AcquireFn = unsafe extern "C" fn(*mut c_void);
type ReleaseFn = unsafe extern "C" fn(*mut c_void);
type GetWidthFn = unsafe extern "C" fn(*mut c_void) -> i32;
type GetHeightFn = unsafe extern "C" fn(*mut c_void) -> i32;
type SetGeometryFn = unsafe extern "C" fn(*mut c_void, i32, i32, i32) -> i32;

/// CPU-locked buffer out-param for the connect ritual below.
#[repr(C)]
pub struct ANativeWindowBufferLock {
    pub width: i32,
    pub height: i32,
    pub stride: i32,
    pub format: i32,
    pub bits: *mut c_void,
    pub reserved: [u32; 6],
}

type LockFn = unsafe extern "C" fn(*mut c_void, *mut ANativeWindowBufferLock, *mut c_void) -> i32;
type UnlockAndPostFn = unsafe extern "C" fn(*mut c_void) -> i32;

pub unsafe fn acquire(window: *mut c_void, api: &AnwApi) {
    (api.acquire)(window)
}

pub unsafe fn release(window: *mut c_void, api: &AnwApi) {
    (api.release)(window)
}

pub unsafe fn get_width(window: *mut c_void, api: &AnwApi) -> i32 {
    (api.get_width)(window)
}

pub unsafe fn get_height(window: *mut c_void, api: &AnwApi) -> i32 {
    (api.get_height)(window)
}

pub unsafe fn set_buffers_geometry(
    window: *mut c_void,
    api: &AnwApi,
    width: i32,
    height: i32,
    format: i32,
) -> i32 {
    (api.set_geometry)(window, width, height, format)
}

/// dlsym'd hidden entry points. `Send` (function pointers are).
pub struct AnwApi {
    acquire: libloading::Symbol<'static, AcquireFn>,
    release: libloading::Symbol<'static, ReleaseFn>,
    get_width: libloading::Symbol<'static, GetWidthFn>,
    get_height: libloading::Symbol<'static, GetHeightFn>,
    set_geometry: libloading::Symbol<'static, SetGeometryFn>,
    lock: libloading::Symbol<'static, LockFn>,
    unlock_and_post: libloading::Symbol<'static, UnlockAndPostFn>,
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
            acquire: sym!(b"ANativeWindow_acquire"),
            release: sym!(b"ANativeWindow_release"),
            get_width: sym!(b"ANativeWindow_getWidth"),
            get_height: sym!(b"ANativeWindow_getHeight"),
            set_geometry: sym!(b"ANativeWindow_setBuffersGeometry"),
            lock: sym!(b"ANativeWindow_lock"),
            unlock_and_post: sym!(b"ANativeWindow_unlockAndPost"),
            set_buffer_count: sym!(b"ANativeWindow_setBufferCount"),
            query: sym!(b"ANativeWindow_query"),
            dequeue_buffer: sym!(b"ANativeWindow_dequeueBuffer"),
            queue_buffer: sym!(b"ANativeWindow_queueBuffer"),
            cancel_buffer: sym!(b"ANativeWindow_cancelBuffer"),
        })
    }

    /// Direct struct perform() connect. Retained as ABI documentation only:
    /// a misdirected indirect call here crashed on the Pad 3 (OxygenOS
    /// table layout), so sessions must use [`Self::connect_cpu_ritual`].
    #[allow(dead_code)]
    pub unsafe fn api_connect(window: *mut c_void, api: c_int) -> c_int {
        let w = window as *mut AnwWindow;
        match (*w).perform {
            Some(perform) => perform(w, ANW_API_CONNECT, api),
            None => -1,
        }
    }

    /// Connect the window to the CPU API without touching the fragile
    /// struct `perform()` slot: the legacy lock/unlock pair connects
    /// internally. One unrendered (black) frame is posted and immediately
    /// replaced once the producer connects; the screen shows the app
    /// background until then either way.
    pub unsafe fn connect_cpu_ritual(&self, window: *mut c_void) -> Result<(), String> {
        let mut out: ANativeWindowBufferLock = std::mem::zeroed();
        let r = (self.lock)(window, &mut out, std::ptr::null_mut());
        if r != 0 {
            return Err(format!("ANativeWindow_lock connect ritual failed: {r}"));
        }
        let r = (self.unlock_and_post)(window);
        if r != 0 {
            return Err(format!("ANativeWindow_unlockAndPost failed: {r}"));
        }
        Ok(())
    }

    /// In-object table dump (read-only). Retained for bring-up forensics;
    /// not used by sessions: the supposed slot addresses disagree with the
    /// dlsym'd entries on OxygenOS, so nothing may call through the table.
    #[allow(dead_code)]
    pub unsafe fn dump_table(&self, window: *mut c_void) {
        let w = window as *const AnwWindow;
        let slots = [
            ("setSwapInterval", (*w).set_swap_interval.map(|f| f as usize)),
            ("query", (*w).query.map(|f| f as usize)),
            ("perform", (*w).perform.map(|f| f as usize)),
            ("dequeueBuffer", (*w).dequeue_buffer.map(|f| f as usize)),
            ("queueBuffer", (*w).queue_buffer.map(|f| f as usize)),
            ("cancelBuffer", (*w).cancel_buffer.map(|f| f as usize)),
        ];
        for (name, addr) in slots {
            match addr {
                Some(a) => log::info!("anland.anw_table {name}=0x{a:x}"),
                None => log::info!("anland.anw_table {name}=null"),
            }
        }
        log::info!(
            "anland.anw_sym setBufferCount=0x{:x} query=0x{:x} dequeue=0x{:x} queue=0x{:x} cancel=0x{:x} lock=0x{:x} unlockPost=0x{:x}",
            *self.set_buffer_count as usize,
            *self.query as usize,
            *self.dequeue_buffer as usize,
            *self.queue_buffer as usize,
            *self.cancel_buffer as usize,
            *self.lock as usize,
            *self.unlock_and_post as usize,
        );
    }

    #[allow(dead_code)]
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
