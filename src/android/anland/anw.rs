//! Hidden `ANativeWindow` dequeue/queue API (Android-only).
//!
//! Rust mirror of `third_party/anland/consumer/anw_hidden.h`
//! (lfdevs/anland-termux, GPL-3.0). The NDK exposes `ANativeWindow` as opaque;
//! the per-slot BufferQueue API (`dequeueBuffer`/`queueBuffer` with sync-file
//! fences) is reached through explicitly resolved `libnativewindow.so`
//! symbols. The fragile private `ANativeWindow::perform()` table is not used.
//!
//! Layouts must match the platform ABI exactly; see the vendored header.

use std::ffi::c_void;
use std::os::raw::c_int;

pub const QUERY_MIN_UNDEQUEUED_BUFFERS: c_int = 3;
pub const FORMAT_RGBA_8888: c_int = 1; // == AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM
/// Public Android dataspace value for the ordinary full-range sRGB pipeline.
/// This is the NDK `ADATASPACE_SRGB` value from the generated Android FFI.
pub const DATASPACE_SRGB: c_int = 142_671_872;

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

/// Public `ANativeWindow` entry points, resolved via dlsym like the hidden
/// ones (avoids a build-time `libnativewindow.so` dependency the APK packager
/// cannot satisfy; the library is always present on-device).
type AcquireFn = unsafe extern "C" fn(*mut c_void);
type ReleaseFn = unsafe extern "C" fn(*mut c_void);
type GetWidthFn = unsafe extern "C" fn(*mut c_void) -> i32;
type GetHeightFn = unsafe extern "C" fn(*mut c_void) -> i32;
type SetGeometryFn = unsafe extern "C" fn(*mut c_void, i32, i32, i32) -> i32;
type SetDataspaceFn = unsafe extern "C" fn(*mut c_void, c_int) -> i32;

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
    set_dataspace: libloading::Symbol<'static, SetDataspaceFn>,
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
            set_dataspace: sym!(b"ANativeWindow_setBuffersDataSpace"),
            lock: sym!(b"ANativeWindow_lock"),
            unlock_and_post: sym!(b"ANativeWindow_unlockAndPost"),
            set_buffer_count: sym!(b"ANativeWindow_setBufferCount"),
            query: sym!(b"ANativeWindow_query"),
            dequeue_buffer: sym!(b"ANativeWindow_dequeueBuffer"),
            queue_buffer: sym!(b"ANativeWindow_queueBuffer"),
            cancel_buffer: sym!(b"ANativeWindow_cancelBuffer"),
        })
    }

    /// Connect the window to the CPU API without touching the fragile
    /// struct `perform()` slot: the legacy lock/unlock pair connects
    /// internally. The first CPU-owned buffer is explicitly cleared before it
    /// is posted, so the surface never presents whatever stale bytes happened
    /// to be in a recycled gralloc allocation.
    pub unsafe fn connect_cpu_ritual(&self, window: *mut c_void) -> Result<(), String> {
        self.clear_cpu_buffer(window)
    }

    /// Lock one CPU buffer, clear the complete stride-padded RGBA allocation,
    /// and post it. The hidden dequeue path is not allowed to invent a pixel
    /// layout: only the exact format and checked stride/size returned by
    /// `ANativeWindow_lock` are accepted.
    pub unsafe fn clear_cpu_buffer(&self, window: *mut c_void) -> Result<(), String> {
        let mut out: ANativeWindowBufferLock = std::mem::zeroed();
        let r = (self.lock)(window, &mut out, std::ptr::null_mut());
        if r != 0 {
            return Err(format!("ANativeWindow_lock black-buffer init failed: {r}"));
        }

        let validation = (|| {
            if out.format != FORMAT_RGBA_8888 {
                return Err(format!(
                    "black-buffer init returned unsupported format {} (expected {})",
                    out.format, FORMAT_RGBA_8888
                ));
            }
            if out.width <= 0 || out.height <= 0 || out.stride < out.width {
                return Err(format!(
                    "black-buffer init returned invalid geometry {}x{} stride={}",
                    out.width, out.height, out.stride
                ));
            }
            if out.bits.is_null() {
                return Err("black-buffer init returned null CPU mapping".into());
            }
            let pixels = (out.stride as usize)
                .checked_mul(out.height as usize)
                .ok_or_else(|| "black-buffer byte size overflow".to_string())?;
            let bytes = pixels
                .checked_mul(4)
                .ok_or_else(|| "black-buffer byte size overflow".to_string())?;
            // Avoid turning a corrupt platform out-param into an unbounded
            // memset while still allowing the largest supported Pad 3 mode.
            if bytes > 512 * 1024 * 1024 {
                return Err(format!(
                    "black-buffer mapping is implausibly large: {bytes} bytes"
                ));
            }
            std::ptr::write_bytes(out.bits.cast::<u8>(), 0, bytes);
            Ok(())
        })();

        let unlock = (self.unlock_and_post)(window);
        if unlock != 0 {
            return Err(format!("ANativeWindow_unlockAndPost failed: {unlock}"));
        }
        validation
    }

    pub unsafe fn set_buffer_count(&self, window: *mut c_void, count: usize) -> c_int {
        (self.set_buffer_count)(window, count)
    }

    /// Set the public Android buffer dataspace. Portal intentionally supports
    /// one boring SDR contract: RGBA_8888 pixels in full-range sRGB. Failing
    /// this call aborts the surface setup instead of allowing an implicit or
    /// stale HDR/wide-colour dataspace to reinterpret KWin's bytes.
    pub unsafe fn set_buffers_dataspace(&self, window: *mut c_void, dataspace: c_int) -> c_int {
        (self.set_dataspace)(window, dataspace)
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
    ///
    /// A successful dequeue transfers ownership of the acquire fence to the
    /// caller. The caller must either wait for it and close it, or transfer
    /// that exact unsignalled fd back through `cancelBuffer`; closing it and
    /// substituting `-1` loses SurfaceFlinger's dependency.
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
        // Android's ANativeWindow contract consumes/closes `fence` at this
        // call boundary, including an error return.
        (self.queue_buffer)(window, buf, fence)
    }

    pub unsafe fn cancel(
        &self,
        window: *mut c_void,
        buf: *mut ANativeWindowBuffer,
        fence: c_int,
    ) -> c_int {
        // Same ownership rule as queueBuffer.
        (self.cancel_buffer)(window, buf, fence)
    }

    /// Read the dma-buf layout exposed by a dequeued Android buffer.
    ///
    /// The Anland wire format is intentionally limited to one linear RGBA
    /// plane. Rejecting all other native-handle shapes here prevents the
    /// caller from silently fabricating a modifier/plane description that the
    /// ANativeWindow ABI did not provide. The rejection carries the platform
    /// metadata into the log so a device-specific gralloc layout can be fixed
    /// at the boundary instead of being guessed from a generic error.
    pub unsafe fn buffer_dma_info(
        buf: *mut ANativeWindowBuffer,
    ) -> Result<BufferDmaInfo, BufferDmaReject> {
        if buf.is_null() {
            return Err(BufferDmaReject::NullBuffer);
        }
        let b = &*buf;
        let header = BufferHeader {
            width: b.width,
            height: b.height,
            stride_px: b.stride,
            format: b.format,
            layer_count: b.layer_count,
            usage: b.usage,
            usage_deprecated: b.usage_deprecated,
        };
        if b.width <= 0 || b.height <= 0 || b.stride < b.width {
            return Err(BufferDmaReject::InvalidGeometry(header));
        }
        if b.format != FORMAT_RGBA_8888 {
            return Err(BufferDmaReject::UnsupportedFormat(header));
        }
        if b.layer_count != 1 {
            return Err(BufferDmaReject::UnsupportedLayerCount(header));
        }
        let Some(handle) = b.handle.as_ref() else {
            return Err(BufferDmaReject::MissingHandle(header));
        };
        if handle.num_fds < 0
            || handle.num_ints < 0
            || (handle.num_fds as u32).saturating_add(handle.num_ints as u32)
                > MAX_NATIVE_HANDLE_ELEMENTS
        {
            return Err(BufferDmaReject::InvalidHandleCounts {
                header,
                version: handle.version,
                num_fds: handle.num_fds,
                num_ints: handle.num_ints,
            });
        }
        let elements = handle.num_fds as usize + handle.num_ints as usize;
        let data = std::slice::from_raw_parts(handle.data_ptr(), elements);
        let (fd, modifier) = if handle.num_fds == 1 && handle.num_ints == 0 {
            // A plain one-fd native handle is already an explicit linear
            // single-plane allocation. There is no vendor metadata to infer.
            (data[0], DRM_FORMAT_MOD_LINEAR)
        } else if handle.version == 12
            && handle.num_fds == QCOM_HANDLE_NUM_FDS
            && handle.num_ints == QCOM_HANDLE_NUM_INTS
        {
            // Pad 3's Android 16 QTI gralloc handle is the packed QCOM
            // layout: two fds (pixel + metadata) followed by the stable
            // geometry/format/usage prefix. The metadata fd is not a second
            // image plane. It is intentionally retained by Android's native
            // handle; only the validated pixel fd crosses the Anland wire.
            //
            // Do not infer a modifier from num_fds alone. The UBWC flag and
            // exact allocated byte size below prove that this particular
            // handle is linear before modifier=LINEAR is emitted.
            //
            // The QCOM width field carries the gralloc-aligned width, which
            // equals the ANativeWindow stride. When the logical width is not
            // a multiple of the alignment (portrait 2400 -> stride 2432),
            // qcom_width is the stride, not the width; when already aligned
            // (landscape 3392) all three agree. Accept either, and let the
            // exact byte-size check below remain the hard linearity proof.
            let ints = &data[QCOM_HANDLE_NUM_FDS as usize..];
            let qcom_flags = ints[QCOM_FLAGS_INDEX];
            let qcom_width = ints[QCOM_WIDTH_INDEX];
            let qcom_height = ints[QCOM_HEIGHT_INDEX];
            let qcom_unaligned_width = ints[QCOM_UNALIGNED_WIDTH_INDEX];
            let qcom_unaligned_height = ints[QCOM_UNALIGNED_HEIGHT_INDEX];
            let qcom_format = ints[QCOM_FORMAT_INDEX];
            let qcom_layer_count = ints[QCOM_LAYER_COUNT_INDEX];
            let qcom_usage = (ints[QCOM_USAGE_HIGH_INDEX] as u64) << 32
                | ints[QCOM_USAGE_LOW_INDEX] as u32 as u64;
            let qcom_size = ints[QCOM_SIZE_INDEX] as u32 as u64;
            let width_matches = qcom_width == b.width || qcom_width == b.stride;
            if !width_matches
                || qcom_height != b.height
                || qcom_unaligned_width != b.width
                || qcom_unaligned_height != b.height
                || qcom_format != b.format
                || qcom_layer_count != 1
                || qcom_usage != b.usage
            {
                let preview_len = elements.min(NATIVE_HANDLE_PREVIEW_ELEMENTS);
                return Err(BufferDmaReject::UnsupportedHandleShape {
                    header,
                    version: handle.version,
                    num_fds: handle.num_fds,
                    num_ints: handle.num_ints,
                    preview: data[..preview_len].to_vec(),
                });
            }
            if qcom_flags & QCOM_PRIV_FLAGS_UBWC_ALIGNED != 0 {
                return Err(BufferDmaReject::UnsupportedModifier {
                    header,
                    flags: qcom_flags,
                    modifier: DRM_FORMAT_MOD_QCOM_COMPRESSED,
                });
            }
            let expected_bytes = (b.stride as u64)
                .checked_mul(b.height as u64)
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or(BufferDmaReject::InvalidLinearSize {
                    header,
                    allocated_bytes: qcom_size,
                    expected_bytes: u64::MAX,
                })?;
            if qcom_size != expected_bytes || data[1] < 0 {
                return Err(BufferDmaReject::InvalidLinearSize {
                    header,
                    allocated_bytes: qcom_size,
                    expected_bytes,
                });
            }
            (data[0], DRM_FORMAT_MOD_LINEAR)
        } else {
            let preview_len = elements.min(NATIVE_HANDLE_PREVIEW_ELEMENTS);
            return Err(BufferDmaReject::UnsupportedHandleShape {
                header,
                version: handle.version,
                num_fds: handle.num_fds,
                num_ints: handle.num_ints,
                preview: data[..preview_len].to_vec(),
            });
        };
        if fd < 0 {
            return Err(BufferDmaReject::InvalidFd { header, fd });
        }
        Ok(BufferDmaInfo {
            fd,
            stride_px: b.stride,
            width: b.width,
            height: b.height,
            format: b.format,
            num_fds: handle.num_fds,
            num_ints: handle.num_ints,
            modifier,
        })
    }
}

const DRM_FORMAT_MOD_LINEAR: u64 = 0;
// fourcc_mod_code(QCOM, 1), as used by the Qualcomm DRM/gralloc sources.
const DRM_FORMAT_MOD_QCOM_COMPRESSED: u64 = 0x0100_0000_0000_0001;
const MAX_NATIVE_HANDLE_ELEMENTS: u32 = 128;
const NATIVE_HANDLE_PREVIEW_ELEMENTS: usize = 16;

// Android 16 Pad 3 observation, cross-checked against the QTI packed handle
// prefix. Keep this exact shape fail-closed: a future vendor handle must not
// be reinterpreted as a linear buffer merely because it has two fds.
const QCOM_HANDLE_NUM_FDS: c_int = 2;
const QCOM_HANDLE_NUM_INTS: c_int = 34;
const QCOM_FLAGS_INDEX: usize = 1;
const QCOM_WIDTH_INDEX: usize = 2;
const QCOM_HEIGHT_INDEX: usize = 3;
const QCOM_UNALIGNED_WIDTH_INDEX: usize = 4;
const QCOM_UNALIGNED_HEIGHT_INDEX: usize = 5;
const QCOM_FORMAT_INDEX: usize = 6;
const QCOM_LAYER_COUNT_INDEX: usize = 8;
const QCOM_USAGE_LOW_INDEX: usize = 11;
const QCOM_USAGE_HIGH_INDEX: usize = 12;
const QCOM_SIZE_INDEX: usize = 13;
const QCOM_PRIV_FLAGS_UBWC_ALIGNED: i32 = 0x0800_0000;

#[derive(Clone, Copy, Debug)]
pub struct BufferHeader {
    pub width: i32,
    pub height: i32,
    pub stride_px: i32,
    pub format: c_int,
    pub layer_count: usize,
    pub usage: u64,
    pub usage_deprecated: i32,
}

#[derive(Debug)]
pub enum BufferDmaReject {
    NullBuffer,
    InvalidGeometry(BufferHeader),
    UnsupportedFormat(BufferHeader),
    UnsupportedLayerCount(BufferHeader),
    MissingHandle(BufferHeader),
    InvalidHandleCounts {
        header: BufferHeader,
        version: c_int,
        num_fds: c_int,
        num_ints: c_int,
    },
    UnsupportedHandleShape {
        header: BufferHeader,
        version: c_int,
        num_fds: c_int,
        num_ints: c_int,
        preview: Vec<c_int>,
    },
    UnsupportedModifier {
        header: BufferHeader,
        flags: c_int,
        modifier: u64,
    },
    InvalidLinearSize {
        header: BufferHeader,
        allocated_bytes: u64,
        expected_bytes: u64,
    },
    InvalidFd {
        header: BufferHeader,
        fd: c_int,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct BufferDmaInfo {
    pub fd: c_int,
    pub stride_px: i32,
    pub width: i32,
    pub height: i32,
    pub format: c_int,
    pub num_fds: c_int,
    pub num_ints: c_int,
    pub modifier: u64,
}

impl NativeHandle {
    unsafe fn data_ptr(&self) -> *const c_int {
        (self as *const NativeHandle).add(1) as *const c_int
    }
}
