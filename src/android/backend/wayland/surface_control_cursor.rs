//! Android SurfaceControl cursor layer.
//!
//! The desktop remains on the proven Smithay GLES/EGL path. Cursor-sized
//! `wl_shm` images are copied into a small AHardwareBuffer pool only when the
//! image changes; pointer motion thereafter is a position-only transaction.
//! Android 16's per-buffer release callback/fence is required, so older or
//! incomplete implementations transparently retain the GLES cursor.

use crate::android::utils::frame_pacing::AndroidFrameTimeline;
use libloading::Library;
use smithay::{
    backend::renderer::utils::{with_renderer_surface_state, CommitCounter},
    reexports::wayland_server::protocol::{wl_buffer::WlBuffer, wl_shm},
    reexports::wayland_server::Resource,
    wayland::shm::with_buffer_contents,
};
use std::{
    ffi::{c_char, c_void, CString},
    ptr::NonNull,
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

const CPU_WRITE_OFTEN: u64 = 3 << 4;
const GPU_SAMPLED_IMAGE: u64 = 1 << 8;
const COMPOSER_OVERLAY: u64 = 1 << 11;
const FORMAT_RGBA_8888: u32 = 1;
const VISIBILITY_HIDE: i8 = 0;
const VISIBILITY_SHOW: i8 = 1;
const TRANSPARENCY_TRANSLUCENT: i8 = 1;

#[repr(C)]
struct SurfaceControl;
#[repr(C)]
struct SurfaceTransaction;
#[repr(C)]
struct HardwareBuffer;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct HardwareBufferDesc {
    width: u32,
    height: u32,
    layers: u32,
    format: u32,
    usage: u64,
    stride: u32,
    rfu0: u32,
    rfu1: u64,
}

type CreateFromWindow = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut SurfaceControl;
type ReleaseSurface = unsafe extern "C" fn(*mut SurfaceControl);
type CreateTransaction = unsafe extern "C" fn() -> *mut SurfaceTransaction;
type DeleteTransaction = unsafe extern "C" fn(*mut SurfaceTransaction);
type ApplyTransaction = unsafe extern "C" fn(*mut SurfaceTransaction);
type SetVisibility = unsafe extern "C" fn(*mut SurfaceTransaction, *mut SurfaceControl, i8);
type SetZOrder = unsafe extern "C" fn(*mut SurfaceTransaction, *mut SurfaceControl, i32);
type SetPosition = unsafe extern "C" fn(*mut SurfaceTransaction, *mut SurfaceControl, i32, i32);
type SetScale = unsafe extern "C" fn(*mut SurfaceTransaction, *mut SurfaceControl, f32, f32);
type SetTransparency = unsafe extern "C" fn(*mut SurfaceTransaction, *mut SurfaceControl, i8);
type SetFrameTimeline = unsafe extern "C" fn(*mut SurfaceTransaction, i64);
type SetDesiredPresentTime = unsafe extern "C" fn(*mut SurfaceTransaction, i64);
type BufferReleaseCallback = extern "C" fn(*mut c_void, i32);
type SetBufferWithRelease = unsafe extern "C" fn(
    *mut SurfaceTransaction,
    *mut SurfaceControl,
    *mut HardwareBuffer,
    i32,
    *mut c_void,
    BufferReleaseCallback,
);
type AllocateBuffer =
    unsafe extern "C" fn(*const HardwareBufferDesc, *mut *mut HardwareBuffer) -> i32;
type DescribeBuffer = unsafe extern "C" fn(*const HardwareBuffer, *mut HardwareBufferDesc);
type ReleaseBuffer = unsafe extern "C" fn(*mut HardwareBuffer);
type LockBuffer =
    unsafe extern "C" fn(*mut HardwareBuffer, u64, i32, *const c_void, *mut *mut c_void) -> i32;
type UnlockBuffer = unsafe extern "C" fn(*mut HardwareBuffer, *mut i32) -> i32;

struct Api {
    create_from_window: CreateFromWindow,
    release_surface: ReleaseSurface,
    create_transaction: CreateTransaction,
    delete_transaction: DeleteTransaction,
    apply_transaction: ApplyTransaction,
    set_visibility: SetVisibility,
    set_z_order: SetZOrder,
    set_position: SetPosition,
    set_scale: SetScale,
    set_transparency: SetTransparency,
    set_frame_timeline: Option<SetFrameTimeline>,
    set_desired_present_time: SetDesiredPresentTime,
    set_buffer_with_release: SetBufferWithRelease,
    allocate_buffer: AllocateBuffer,
    describe_buffer: DescribeBuffer,
    release_buffer: ReleaseBuffer,
    lock_buffer: LockBuffer,
    unlock_buffer: UnlockBuffer,
    _library: Library,
}

impl Api {
    unsafe fn load() -> Option<Self> {
        let library = Library::new("libandroid.so").ok()?;
        macro_rules! required {
            ($name:literal, $ty:ty) => {
                *library.get::<$ty>(concat!($name, "\0").as_bytes()).ok()?
            };
        }
        let api = Self {
            create_from_window: required!("ASurfaceControl_createFromWindow", CreateFromWindow),
            release_surface: required!("ASurfaceControl_release", ReleaseSurface),
            create_transaction: required!("ASurfaceTransaction_create", CreateTransaction),
            delete_transaction: required!("ASurfaceTransaction_delete", DeleteTransaction),
            apply_transaction: required!("ASurfaceTransaction_apply", ApplyTransaction),
            set_visibility: required!("ASurfaceTransaction_setVisibility", SetVisibility),
            set_z_order: required!("ASurfaceTransaction_setZOrder", SetZOrder),
            set_position: required!("ASurfaceTransaction_setPosition", SetPosition),
            set_scale: required!("ASurfaceTransaction_setScale", SetScale),
            set_transparency: required!(
                "ASurfaceTransaction_setBufferTransparency",
                SetTransparency
            ),
            set_frame_timeline: library
                .get::<SetFrameTimeline>(b"ASurfaceTransaction_setFrameTimeline\0")
                .ok()
                .map(|symbol| *symbol),
            set_desired_present_time: required!(
                "ASurfaceTransaction_setDesiredPresentTime",
                SetDesiredPresentTime
            ),
            // API 36 is deliberate: without a per-buffer release callback the
            // cursor pool cannot prove safe reuse independently of EGL.
            set_buffer_with_release: required!(
                "ASurfaceTransaction_setBufferWithRelease",
                SetBufferWithRelease
            ),
            allocate_buffer: required!("AHardwareBuffer_allocate", AllocateBuffer),
            describe_buffer: required!("AHardwareBuffer_describe", DescribeBuffer),
            release_buffer: required!("AHardwareBuffer_release", ReleaseBuffer),
            lock_buffer: required!("AHardwareBuffer_lock", LockBuffer),
            unlock_buffer: required!("AHardwareBuffer_unlock", UnlockBuffer),
            _library: library,
        };
        Some(api)
    }
}

struct ReleaseState {
    available: AtomicBool,
    fence: AtomicI32,
}

extern "C" fn buffer_released(context: *mut c_void, release_fence: i32) {
    if context.is_null() {
        if release_fence >= 0 {
            unsafe { libc::close(release_fence) };
        }
        return;
    }
    let state = unsafe { Box::from_raw(context.cast::<Arc<ReleaseState>>()) };
    let previous = state.fence.swap(release_fence, Ordering::AcqRel);
    if previous >= 0 {
        unsafe { libc::close(previous) };
    }
    state.available.store(true, Ordering::Release);
}

struct CursorBuffer {
    raw: NonNull<HardwareBuffer>,
    stride_pixels: usize,
    release: Arc<ReleaseState>,
    release_buffer: ReleaseBuffer,
}

impl CursorBuffer {
    fn new(api: &Api, width: u32, height: u32) -> Result<Self, String> {
        let desc = HardwareBufferDesc {
            width,
            height,
            layers: 1,
            format: FORMAT_RGBA_8888,
            usage: CPU_WRITE_OFTEN | GPU_SAMPLED_IMAGE | COMPOSER_OVERLAY,
            ..Default::default()
        };
        let mut raw = std::ptr::null_mut();
        let result = unsafe { (api.allocate_buffer)(&desc, &mut raw) };
        let raw = NonNull::new(raw)
            .filter(|_| result == 0)
            .ok_or_else(|| format!("AHardwareBuffer_allocate failed: {result}"))?;
        let mut actual = HardwareBufferDesc::default();
        unsafe { (api.describe_buffer)(raw.as_ptr(), &mut actual) };
        Ok(Self {
            raw,
            stride_pixels: actual.stride as usize,
            release: Arc::new(ReleaseState {
                available: AtomicBool::new(true),
                fence: AtomicI32::new(-1),
            }),
            release_buffer: api.release_buffer,
        })
    }
}

struct CursorPool {
    width: u32,
    height: u32,
    buffers: Vec<CursorBuffer>,
}

pub struct SurfaceControlCursor {
    api: Api,
    surface: NonNull<SurfaceControl>,
    pool: Option<CursorPool>,
    last_commit: Option<CommitCounter>,
    last_surface_id: Option<u32>,
    last_scale: Option<f32>,
    visible: bool,
    last_position: Option<(i32, i32)>,
    stats_started: Instant,
    image_updates: u64,
    position_updates: u64,
    pool_busy: u64,
    pointer_latency_samples: u64,
    pointer_latency_ns: u128,
}

impl SurfaceControlCursor {
    pub fn new(native_window: *mut c_void) -> Result<Self, String> {
        if native_window.is_null() {
            return Err("Android native window is null".into());
        }
        if presenter_mode_is_egl() {
            return Err("disabled by presenter-mode=egl".into());
        }
        let api = unsafe { Api::load() }
            .ok_or_else(|| "Android 16 SurfaceControl release-fence API unavailable".to_string())?;
        let name = CString::new("Portal hardware cursor").unwrap();
        let surface =
            NonNull::new(unsafe { (api.create_from_window)(native_window, name.as_ptr()) })
                .ok_or_else(|| "ASurfaceControl_createFromWindow failed".to_string())?;
        let mut cursor = Self {
            api,
            surface,
            pool: None,
            last_commit: None,
            last_surface_id: None,
            last_scale: None,
            visible: false,
            last_position: None,
            stats_started: Instant::now(),
            image_updates: 0,
            position_updates: 0,
            pool_busy: 0,
            pointer_latency_samples: 0,
            pointer_latency_ns: 0,
        };
        cursor.configure_initial()?;
        Ok(cursor)
    }

    fn transaction(&self) -> Result<NonNull<SurfaceTransaction>, String> {
        NonNull::new(unsafe { (self.api.create_transaction)() })
            .ok_or_else(|| "ASurfaceTransaction_create failed".to_string())
    }

    fn apply(&self, transaction: NonNull<SurfaceTransaction>) {
        unsafe {
            (self.api.apply_transaction)(transaction.as_ptr());
            (self.api.delete_transaction)(transaction.as_ptr());
        }
    }

    fn configure_initial(&mut self) -> Result<(), String> {
        let transaction = self.transaction()?;
        unsafe {
            (self.api.set_z_order)(transaction.as_ptr(), self.surface.as_ptr(), i32::MAX);
            (self.api.set_transparency)(
                transaction.as_ptr(),
                self.surface.as_ptr(),
                TRANSPARENCY_TRANSLUCENT,
            );
            (self.api.set_visibility)(transaction.as_ptr(), self.surface.as_ptr(), VISIBILITY_HIDE);
        }
        self.apply(transaction);
        Ok(())
    }

    /// Upload a changed, simple SHM cursor and position it. Returns true only
    /// when the hardware layer owns cursor presentation for this frame.
    pub fn update(
        &mut self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        position: (i32, i32),
        scale: f32,
        timeline: Option<AndroidFrameTimeline>,
    ) -> bool {
        let snapshot = with_renderer_surface_state(surface, |state| {
            let view = state.view()?;
            let size = state.buffer_size()?;
            if state.buffer_scale() != 1
                || state.buffer_transform() != smithay::utils::Transform::Normal
                || view.offset != (0, 0).into()
                || view.src.loc != (0.0, 0.0).into()
                || view.src.size != (size.w as f64, size.h as f64).into()
                || view.dst != size
            {
                return None;
            }
            Some((state.buffer()?.clone(), state.current_commit()))
        })
        .flatten();
        let Some((buffer, commit)) = snapshot else {
            self.hide();
            return false;
        };

        let surface_id = surface.id().protocol_id();
        if self.last_surface_id != Some(surface_id) || self.last_commit != Some(commit) {
            if self.upload(&buffer, commit, position, scale, timeline) {
                self.last_surface_id = Some(surface_id);
            } else {
                // Keep the previous sprite active while all three tiny buffers
                // are still owned by SurfaceFlinger. A later dirty callback
                // will retry; never overwrite an in-use buffer.
                return self.visible;
            }
        } else {
            self.move_to(position, scale, timeline);
        }
        self.report_stats();
        self.visible
    }

    fn upload(
        &mut self,
        buffer: &WlBuffer,
        commit: CommitCounter,
        position: (i32, i32),
        scale: f32,
        timeline: Option<AndroidFrameTimeline>,
    ) -> bool {
        let result = with_buffer_contents(buffer, |source, length, data| {
            if !matches!(
                data.format,
                wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888
            ) || data.width <= 0
                || data.height <= 0
                || data.stride < data.width.saturating_mul(4)
            {
                return Err("unsupported cursor SHM layout".to_string());
            }
            let width = data.width as u32;
            let height = data.height as u32;
            let required = data.offset as isize
                + (data.height as isize - 1) * data.stride as isize
                + data.width as isize * 4;
            if data.offset < 0 || required < 0 || required as usize > length {
                return Err("cursor SHM range is invalid".to_string());
            }
            if self
                .pool
                .as_ref()
                .is_none_or(|pool| pool.width != width || pool.height != height)
            {
                let mut buffers = Vec::with_capacity(3);
                for _ in 0..3 {
                    buffers.push(CursorBuffer::new(&self.api, width, height)?);
                }
                self.pool = Some(CursorPool {
                    width,
                    height,
                    buffers,
                });
            }
            let pool = self.pool.as_mut().unwrap();
            let Some(slot) = pool
                .buffers
                .iter_mut()
                .find(|slot| slot.release.available.swap(false, Ordering::AcqRel))
            else {
                self.pool_busy += 1;
                return Ok(false);
            };
            let release_fence = slot.release.fence.swap(-1, Ordering::AcqRel);
            let mut destination = std::ptr::null_mut();
            let lock_result = unsafe {
                (self.api.lock_buffer)(
                    slot.raw.as_ptr(),
                    CPU_WRITE_OFTEN,
                    release_fence,
                    std::ptr::null(),
                    &mut destination,
                )
            };
            if release_fence >= 0 {
                unsafe { libc::close(release_fence) };
            }
            if lock_result != 0 || destination.is_null() {
                slot.release.available.store(true, Ordering::Release);
                return Err(format!("AHardwareBuffer_lock failed: {lock_result}"));
            }
            let alpha = data.format == wl_shm::Format::Argb8888;
            unsafe {
                copy_bgra_to_rgba(
                    source.add(data.offset as usize),
                    data.stride as usize,
                    destination.cast::<u8>(),
                    slot.stride_pixels * 4,
                    width as usize,
                    height as usize,
                    alpha,
                );
            }
            let mut acquire_fence = -1;
            let unlock_result =
                unsafe { (self.api.unlock_buffer)(slot.raw.as_ptr(), &mut acquire_fence) };
            if unlock_result != 0 {
                slot.release.available.store(true, Ordering::Release);
                return Err(format!("AHardwareBuffer_unlock failed: {unlock_result}"));
            }
            let slot_raw = slot.raw;
            let slot_release = slot.release.clone();
            let transaction = match self.transaction() {
                Ok(transaction) => transaction,
                Err(error) => {
                    slot_release.available.store(true, Ordering::Release);
                    if acquire_fence >= 0 {
                        unsafe { libc::close(acquire_fence) };
                    }
                    return Err(error);
                }
            };
            unsafe {
                (self.api.set_buffer_with_release)(
                    transaction.as_ptr(),
                    self.surface.as_ptr(),
                    slot_raw.as_ptr(),
                    acquire_fence,
                    Box::into_raw(Box::new(slot_release)).cast(),
                    buffer_released,
                );
                (self.api.set_position)(
                    transaction.as_ptr(),
                    self.surface.as_ptr(),
                    position.0,
                    position.1,
                );
                (self.api.set_scale)(transaction.as_ptr(), self.surface.as_ptr(), scale, scale);
                (self.api.set_visibility)(
                    transaction.as_ptr(),
                    self.surface.as_ptr(),
                    VISIBILITY_SHOW,
                );
                self.apply_timeline(transaction, timeline);
            }
            self.apply(transaction);
            self.visible = true;
            self.last_position = Some(position);
            self.last_scale = Some(scale);
            self.image_updates += 1;
            Ok(true)
        });
        match result {
            Ok(Ok(true)) => {
                self.last_commit = Some(commit);
                true
            }
            Ok(Ok(false)) => false,
            Ok(Err(error)) => {
                log::debug!("SurfaceControl cursor upload skipped: {error}");
                self.hide();
                false
            }
            Err(error) => {
                log::debug!("SurfaceControl cursor is not SHM-backed: {error}");
                self.hide();
                false
            }
        }
    }

    fn move_to(
        &mut self,
        position: (i32, i32),
        scale: f32,
        timeline: Option<AndroidFrameTimeline>,
    ) {
        if !self.visible || (self.last_position == Some(position) && self.last_scale == Some(scale))
        {
            return;
        }
        let Ok(transaction) = self.transaction() else {
            return;
        };
        unsafe {
            (self.api.set_position)(
                transaction.as_ptr(),
                self.surface.as_ptr(),
                position.0,
                position.1,
            );
            if self.last_scale != Some(scale) {
                (self.api.set_scale)(transaction.as_ptr(), self.surface.as_ptr(), scale, scale);
            }
            self.apply_timeline(transaction, timeline);
        }
        self.apply(transaction);
        self.last_position = Some(position);
        self.last_scale = Some(scale);
        self.position_updates += 1;
        self.report_stats();
    }

    unsafe fn apply_timeline(
        &self,
        transaction: NonNull<SurfaceTransaction>,
        timeline: Option<AndroidFrameTimeline>,
    ) {
        let Some(timeline) = timeline else { return };
        if timeline.is_precise() {
            if let Some(set_timeline) = self.api.set_frame_timeline {
                set_timeline(transaction.as_ptr(), timeline.vsync_id);
                return;
            }
        }
        if timeline.expected_present_ns > 0 {
            (self.api.set_desired_present_time)(transaction.as_ptr(), timeline.expected_present_ns);
        }
    }

    pub fn hide(&mut self) {
        if !self.visible {
            return;
        }
        if let Ok(transaction) = self.transaction() {
            unsafe {
                (self.api.set_visibility)(
                    transaction.as_ptr(),
                    self.surface.as_ptr(),
                    VISIBILITY_HIDE,
                );
            }
            self.apply(transaction);
        }
        self.visible = false;
        self.last_commit = None;
        self.last_surface_id = None;
        self.last_position = None;
        self.last_scale = None;
    }

    pub fn note_pointer_latency(&mut self, latency: Duration) {
        self.pointer_latency_samples += 1;
        self.pointer_latency_ns += latency.as_nanos();
        self.report_stats();
    }

    fn report_stats(&mut self) {
        if self.stats_started.elapsed() < Duration::from_secs(5) {
            return;
        }
        let average_pointer_us = if self.pointer_latency_samples == 0 {
            0
        } else {
            (self.pointer_latency_ns / self.pointer_latency_samples as u128 / 1_000) as u64
        };
        log::info!(
            "cursor.surface_control image_updates={} position_transactions={} pool_busy={} pointer_samples={} avg_pointer_to_transaction_us={}",
            self.image_updates,
            self.position_updates,
            self.pool_busy,
            self.pointer_latency_samples,
            average_pointer_us,
        );
        self.stats_started = Instant::now();
        self.image_updates = 0;
        self.position_updates = 0;
        self.pool_busy = 0;
        self.pointer_latency_samples = 0;
        self.pointer_latency_ns = 0;
    }
}

impl Drop for SurfaceControlCursor {
    fn drop(&mut self) {
        self.hide();
        self.pool = None;
        unsafe { (self.api.release_surface)(self.surface.as_ptr()) };
    }
}

impl Drop for CursorBuffer {
    fn drop(&mut self) {
        let fence = self.release.fence.swap(-1, Ordering::AcqRel);
        if fence >= 0 {
            unsafe { libc::close(fence) };
        }
        // The framework owns its own reference while a submitted buffer is in
        // flight; releasing Portal's pool reference is lifecycle-safe.
        unsafe { (self.release_buffer)(self.raw.as_ptr()) };
    }
}

fn presenter_mode_is_egl() -> bool {
    let path = format!("{}/presenter-mode", crate::core::config::APP_FILES_ROOT);
    std::fs::read_to_string(path)
        .ok()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("egl"))
}

unsafe fn copy_bgra_to_rgba(
    source: *const u8,
    source_stride: usize,
    destination: *mut u8,
    destination_stride: usize,
    width: usize,
    height: usize,
    preserve_alpha: bool,
) {
    for y in 0..height {
        let src = source.add(y * source_stride);
        let dst = destination.add(y * destination_stride);
        for x in 0..width {
            let s = src.add(x * 4);
            let d = dst.add(x * 4);
            *d = *s.add(2);
            *d.add(1) = *s.add(1);
            *d.add(2) = *s;
            *d.add(3) = if preserve_alpha { *s.add(3) } else { 0xff };
        }
    }
}
