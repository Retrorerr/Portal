//! Anland GPU consumer session (Android-only).
//!
//! Portal absorbs the reference consumer role (`display_consumer.c`) and the
//! daemon role ([`crate::anland::broker`]) in-process. Buffers are owned by
//! the `ANativeWindow` itself: the session dequeues window slots, hands their
//! dma-buf fds to the guest KWin (`BUFS_READY`), then per frame
//! dequeue → select → fence → `queueBuffer`, handing KWin's render fence to
//! SurfaceFlinger GPU-side. No `wl_shm`, no `glReadPixels`, no `glFinish`.
//!
//! Design mirror: `lfdevs/anland-termux` `native_consumer.c`
//! (see `third_party/anland/ATTRIBUTION.md`).

use std::ffi::c_void;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::anw::{self, ANativeWindowBuffer, AnwApi};
use super::broker::{Broker, Deposit};
use super::protocol::*;
use super::sys;
use crate::core::surface_geometry;

const FENCE_WAIT_MS: i32 = 5000;
/// Cold-start patience, software-fallback sessions ONLY (see
/// `Inner::software_gl`): llvmpipe plus cold shader-cache compilation can
/// need minutes before anything is queued. The production GPU path keeps
/// the tight 5s detector unconditionally: a presenting session that stalls
/// is genuinely wedged within seconds, and a cold GPU boot that cannot
/// produce a frame in seconds is broken, not slow.
const COLD_FIRST_FRAME_WAIT_MS: i64 = 150_000;
/// Acquire-fence wait: blocking, generous. The dequeue only returns slots
/// SurfaceFlinger considers free, and the fence is near-always signaled
/// already; the wait sleeps in the fence ioctl on this dedicated thread
/// (no CPU spin). A short quantum + queue-back was tried and REVERTED: it
/// presents stale buffers early, collapsing the SF/KWin phase separation —
/// KWin then renders into scanout-active dma-bufs (SKIP_IMPLICIT_SYNC_WAIT
/// disables the implicit guard), hanging KGSL so later fences never signal
/// and every subsequent acquire pends forever. Invariant restored: every
/// buffer handed to `queueBuffer` was just rendered by KWin into a properly
/// released slot. On (near-impossible) timeout the buffer is CANCELLED back
/// to the free pool, never presented.
const ACQUIRE_WAIT_MS: i32 = 1000;

/// Instrumentation window for the work-gated `anland.fps` line.
const FPS_WINDOW_NS: u64 = 2_000_000_000;

/// One collected window slot handed to the producer.
struct SlotInfo {
    anb: *mut ANativeWindowBuffer,
    fd: OwnedFd,
    info: BufInfo,
}

// SAFETY: slots are only touched by the render thread while their surface
// generation lives, and the spare is cancelled before the window is released.
unsafe impl Send for SlotInfo {}

/// Ownership state for one successful `dequeueBuffer` call.
///
/// The acquire fence remains inside this lease until it has actually
/// signalled or the lease transfers it to `cancelBuffer`.  In particular,
/// timeout is not a reason to close the fd: Android must receive the original
/// unsignalled dependency when the slot is abandoned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DequeuedState {
    AcquirePending,
    AcquireSatisfied,
    Rendering,
    RenderComplete,
    Queued,
    Cancelled,
    Held,
    /// The render fence was lost and the native surface is being retired.
    /// There is no safe fence to pass to cancelBuffer in this state.
    Abandoned,
}

trait BufferQueueOps {
    fn queue(&self, anb: *mut ANativeWindowBuffer, fence: i32) -> i32;
    fn cancel(&self, anb: *mut ANativeWindowBuffer, fence: i32) -> i32;
}

struct NativeBufferQueue<'a> {
    window: *mut c_void,
    api: &'a AnwApi,
}

impl BufferQueueOps for NativeBufferQueue<'_> {
    fn queue(&self, anb: *mut ANativeWindowBuffer, fence: i32) -> i32 {
        unsafe { self.api.queue(self.window, anb, fence) }
    }

    fn cancel(&self, anb: *mut ANativeWindowBuffer, fence: i32) -> i32 {
        unsafe { self.api.cancel(self.window, anb, fence) }
    }
}

/// A dequeued ANativeWindow slot with one-owner fence semantics.
///
/// The lease borrows the dynamically resolved API for exactly as long as the
/// dequeued slot can be live.  Dropping an unfinished lease performs the safe
/// default operation—`cancelBuffer` with the still-owned acquire fence—so an
/// early return cannot silently strand a BufferQueue slot or lose its fence.
struct DequeuedBuffer<'a, Q: BufferQueueOps + ?Sized> {
    anb: *mut ANativeWindowBuffer,
    queue: &'a Q,
    acquire_fence: Option<OwnedFd>,
    state: DequeuedState,
}

impl<'a, Q: BufferQueueOps + ?Sized> DequeuedBuffer<'a, Q> {
    fn new(anb: *mut ANativeWindowBuffer, acquire_fence: i32, queue: &'a Q) -> Self {
        let acquire_fence = if acquire_fence >= 0 {
            // SAFETY: ANativeWindow returned ownership of this fence fd with
            // the successful dequeue call.
            Some(unsafe { OwnedFd::from_raw_fd(acquire_fence) })
        } else {
            None
        };
        let state = if acquire_fence.is_some() {
            DequeuedState::AcquirePending
        } else {
            DequeuedState::AcquireSatisfied
        };
        Self {
            anb,
            queue,
            acquire_fence,
            state,
        }
    }

    fn wait_for_acquire(&mut self, timeout_ms: i32) -> std::io::Result<()> {
        if self.state != DequeuedState::AcquirePending {
            return Ok(());
        }
        let Some(fence) = self.acquire_fence.as_ref() else {
            self.state = DequeuedState::AcquireSatisfied;
            return Ok(());
        };
        // `wait_fence` borrows the fd.  On timeout/error the lease therefore
        // still owns the exact fd for a later cancelBuffer transfer.
        sys::wait_fence(fence, timeout_ms)?;
        drop(self.acquire_fence.take()); // signalled: close exactly once here
        self.state = DequeuedState::AcquireSatisfied;
        Ok(())
    }

    fn begin_render(&mut self) {
        debug_assert_eq!(self.state, DequeuedState::AcquireSatisfied);
        self.state = DequeuedState::Rendering;
    }

    fn complete_render(&mut self) {
        debug_assert_eq!(self.state, DequeuedState::Rendering);
        self.state = DequeuedState::RenderComplete;
    }

    /// Queue a buffer whose contents were initialized by the synchronous CPU
    /// black-buffer ritual. This is used only while enumerating BufferQueue
    /// slots before the producer handshake; it is not a render-loop frame and
    /// is never counted as a bare presentation.
    fn queue_initialized(mut self) -> i32 {
        debug_assert_eq!(self.state, DequeuedState::AcquireSatisfied);
        self.submit_queue(None)
    }

    fn queue_rendered(mut self, render_fence: Option<OwnedFd>) -> i32 {
        debug_assert_eq!(self.state, DequeuedState::RenderComplete);
        self.submit_queue(render_fence)
    }

    fn into_held(mut self) -> *mut ANativeWindowBuffer {
        debug_assert_eq!(self.state, DequeuedState::AcquireSatisfied);
        self.state = DequeuedState::Held;
        let anb = self.anb;
        // The held spare is represented by the existing raw pointer field in
        // Inner.  Its acquire fence has already been waited and dropped, so
        // the lifecycle owner later cancels it with -1.  No fd or lease state
        // is leaked by forgetting this zero-fd value.
        std::mem::forget(self);
        anb
    }

    /// Retire this slot without calling cancelBuffer. This is only valid
    /// after producer rendering has started and its render-done fence has
    /// been lost; the caller must stop using this native window and let the
    /// lifecycle owner release/rebind it.
    fn abandon_for_surface_reset(mut self) {
        debug_assert!(matches!(
            self.state,
            DequeuedState::Rendering | DequeuedState::RenderComplete
        ));
        debug_assert!(self.acquire_fence.is_none());
        self.state = DequeuedState::Abandoned;
        std::mem::forget(self);
    }

    /// Cancel the slot.  With no explicit render fence, the pending acquire
    /// fence (if any) is transferred.  After a successful acquire wait there
    /// is intentionally no acquire fd and `-1` is correct.
    fn cancel(mut self, render_fence: Option<OwnedFd>) -> i32 {
        let fence = match (render_fence, self.acquire_fence.take()) {
            (Some(render), None) => Some(render),
            (None, acquire) => acquire,
            (Some(render), Some(acquire)) => {
                // This indicates a caller tried to abandon a slot with a
                // render fence before satisfying its acquire dependency. Do
                // not lose the acquire fence; it is the only safe dependency
                // to pass to cancelBuffer. The render fence is not useful to
                // Android in this invalid state and is closed by dropping it.
                log::error!(
                    "anland.dequeued invalid cancel: render fence with pending acquire; preserving acquire fence"
                );
                drop(render);
                Some(acquire)
            }
        };
        self.submit_cancel(fence)
    }

    fn submit_queue(&mut self, render_fence: Option<OwnedFd>) -> i32 {
        debug_assert!(self.acquire_fence.is_none());
        let raw = render_fence.map_or(-1, IntoRawFd::into_raw_fd);
        self.state = DequeuedState::Queued;
        // ANativeWindow's contract transfers ownership at the call boundary;
        // its implementation closes the fence on both success and error.
        // Never close `raw` here, or a failed queueBuffer would double-close
        // the fd that Android already consumed.
        self.queue.queue(self.anb, raw)
    }

    fn submit_cancel(&mut self, fence: Option<OwnedFd>) -> i32 {
        let raw = fence.map_or(-1, IntoRawFd::into_raw_fd);
        self.state = DequeuedState::Cancelled;
        // Same ownership rule as queueBuffer: cancelBuffer consumes `raw`.
        self.queue.cancel(self.anb, raw)
    }
}

impl<Q: BufferQueueOps + ?Sized> Drop for DequeuedBuffer<'_, Q> {
    fn drop(&mut self) {
        if matches!(self.state, DequeuedState::Queued | DequeuedState::Cancelled) {
            return;
        }
        // An unfinished lease must never just drop its acquire fd.  Transfer
        // it to cancelBuffer, including on timeout, epoch change, shutdown,
        // or any other early return.  After an acquire wait this is -1; the
        // acquire dependency has already been satisfied.
        let fence = self.acquire_fence.take();
        let raw = fence.map_or(-1, IntoRawFd::into_raw_fd);
        self.state = DequeuedState::Cancelled;
        let result = self.queue.cancel(self.anb, raw);
        if result != 0 {
            log::error!("anland.dequeued drop cancelBuffer failed: {result}");
        } else {
            log::debug!("anland.dequeued implicit cancel on lease drop");
        }
    }
}

struct ActiveGen {
    id: u64,
    buf_ready: OwnedFd,
    fence_read: OwnedFd,
    data: OwnedFd,
    _shm_fd: OwnedFd,
    /// Shared selected-index mapping; written only under `io_lock`.
    shm_ptr: usize,
    /// Audio slot peer (unused while the guest disables its audio engine).
    /// Held open so the producer never sees HUP on hello slot 4.
    _audio: OwnedFd,
}

/// Work requested by the KWin producer on the current data connection.  The
/// consumer deliberately coalesces newer requests, but never accepts a
/// sequence from an older producer generation.  A request is consumed exactly
/// once by the render thread after an accepted producer wake; VSYNC by itself
/// cannot create one.
#[derive(Default)]
struct FrameWorkState {
    connection_gen: u64,
    producer_generation: u64,
    last_sequence: u64,
    pending: Option<FrameWanted>,
}

impl FrameWorkState {
    fn reset_for_connection(&mut self, connection_gen: u64) {
        self.connection_gen = connection_gen;
        self.producer_generation = 0;
        self.last_sequence = 0;
        self.pending = None;
    }

    fn accept(&mut self, connection_gen: u64, wanted: FrameWanted) -> bool {
        if wanted.sequence == 0 || wanted.generation == 0 {
            return false;
        }
        if self.connection_gen != connection_gen {
            self.reset_for_connection(connection_gen);
        }
        let sequence = wanted.sequence;
        let generation = wanted.generation;
        if generation < self.producer_generation
            || (generation == self.producer_generation && sequence <= self.last_sequence)
        {
            return false;
        }
        if generation > self.producer_generation {
            self.pending = None;
            self.producer_generation = generation;
            self.last_sequence = 0;
        }
        self.last_sequence = sequence;
        self.pending = Some(wanted);
        true
    }

    fn take(&mut self, connection_gen: u64) -> Option<FrameWanted> {
        if self.connection_gen != connection_gen {
            return None;
        }
        self.pending.take()
    }
}

struct Inner {
    /// The current Android presentation surface. This is `None` while the
    /// Activity is suspended. It is never read by a surface-bound worker
    /// unless `window_live` is true and the worker has been started for that
    /// surface generation.
    window: Mutex<Option<*mut c_void>>,
    anw: AnwApi,
    broker: Arc<Broker>,
    running: AtomicBool,
    window_live: AtomicBool,
    broker_stop: Arc<AtomicBool>,
    /// Current connection generation (None = fallback, no producer fds).
    gen: Mutex<Option<ActiveGen>>,
    /// Serializes all quick fd ops on the masters (select writes, input
    /// sends, BUFS_READY, teardown close/munmap). Blocking ops use per-thread
    /// dups and never hold this lock.
    io_lock: Mutex<()>,
    next_gen: Mutex<u64>,
    buffers: Mutex<Vec<SlotInfo>>,
    spare: Mutex<Option<*mut ANativeWindowBuffer>>,
    screen: Mutex<(u32, u32)>,
    refresh_mhz: u32,
    /// Native-window epoch minted for each surface attachment (see
    /// [`surface_geometry::mint_surface_epoch`]). Delayed resize events from
    /// a destroyed surface carry the old epoch and are rejected as stale.
    surface_epoch: AtomicU64,
    /// Converged presentation surface generation. 1 = session start; each
    /// rotation rebind mints exactly one more. Input anchors and readiness
    /// are tagged against this: nothing from an older generation may mutate
    /// newer state or present into the new BufferQueue.
    surface_gen: Mutex<u64>,
    /// True while a requested render-thread surface rebind is outstanding. The
    /// render loop selects nothing (SurfaceFlinger holds its last frame),
    /// input is dropped, and fallback re-deposits are skipped: the rebind
    /// owns the next generation.
    rebind_active: AtomicBool,
    /// Latest requested geometry; consumed only between frames by the render
    /// thread. Android's lifecycle thread never operates on live queue slots.
    rebind_request: Mutex<Option<(u32, u32, u64)>>,
    /// Serializes reconnect/fallback generation replacement.
    transition_lock: Mutex<()>,
    /// Generation that completed its BUFS_READY push. Input events are only
    /// sent for this generation: anything earlier would land in the data
    /// channel ahead of BUFS_READY and desync the producer's handshake.
    connected_gen: Mutex<Option<u64>>,
    /// Producer work requests received on the current data fd. The render
    /// thread consumes this state after the producer wake; a display tick is
    /// optional telemetry only.
    work: Mutex<FrameWorkState>,
    /// A render-fence loss makes the current native window unsafe to reuse.
    /// Surface workers stop and the next Android lifecycle attachment retires
    /// this surface before creating a fresh one.
    surface_recovery_requested: AtomicBool,
    // Proof counters.
    frames_queued: AtomicU64,
    frames_fenced: AtomicU64,
    frames_bare: AtomicU64,
    fallback_count: AtomicU64,
    /// Demand/damage pacing counters (all inexpensive relaxed atomics).
    /// Stable surfaces read them via `diagnostics()` and the 10s alive line;
    /// per-frame detail stays at debug level.
    work_requested: AtomicU64,
    work_rejected: AtomicU64,
    work_consumed: AtomicU64,
    frames_no_damage: AtomicU64,
    unknown_slot_cancels: AtomicU64,
    gen_mismatch_cancels: AtomicU64,
    stale_gen_cancels: AtomicU64,
    dequeue_failures: AtomicU64,
    acquire_timeouts: AtomicU64,
    render_timeouts: AtomicU64,
    queue_failures: AtomicU64,
    cancel_failures: AtomicU64,
    timeline_hit: AtomicU64,
    timeline_miss: AtomicU64,
    deadline_misses: AtomicU64,
    rebinds_completed: AtomicU64,
    dequeue_us_total: AtomicU64,
    dequeue_us_max: AtomicU64,
    acquire_us_total: AtomicU64,
    acquire_us_max: AtomicU64,
    render_us_total: AtomicU64,
    render_us_max: AtomicU64,
    /// Surface generation the plasma-ready marker + Compose latch were
    /// recorded for. Reset on every rebind: readiness re-latches only after
    /// a valid frame from the new generation is actually presented (an old
    /// generation's completion can never mark the new surface ready).
    ready_surface_gen: Mutex<Option<u64>>,
    /// Emergency software-GL fallback active (guest kwin-glmode flag).
    /// Hardware (default): tight 5s fence watchdog, fenced READY evidence.
    /// Software: bare frames prove liveness honestly; the first frame gets
    /// a long cold budget (llvmpipe + cold shader cache need minutes).
    software_gl: bool,
    /// Control wake used only to interrupt the VSYNC wait for lifecycle and
    /// rebind transitions. It never permits a presentation by itself.
    wake: OwnedFd,
    /// Last forwarded pointer position (buffer pixels) for relative-delta
    /// synthesis, tagged with the observing surface generation. The KWin
    /// backend emits both absolute and relative motion from each
    /// POINTER_MOTION (`pointerMotion(pos, delta, delta)`), and relative
    /// clients (games, kinetic velocity) need real dx/dy — winit only
    /// carries absolute positions, so the session tracks them. The anchor
    /// never crosses a geometry boundary: a rebind clears it and any
    /// generation change re-anchors with zero delta (no first-motion jump).
    last_pointer: Mutex<Option<surface_geometry::PointerAnchor>>,
    /// Active touchpad finger-scroll axes as a bitmask (bit 0 = vertical,
    /// bit 1 = horizontal). Scroll-stop events go only to live streams.
    finger_axes: Mutex<u8>,
    /// Key/button presses actually delivered to KWin, released before resize.
    held_inputs: Mutex<std::collections::BTreeSet<(u32, i32)>>,
    /// Display-VSYNC tick source (Choreographer, timer fallback).
    vsync: Mutex<Option<sys::VsyncPump>>,
    /// Surface generation the once-per-generation first-motion diagnostic
    /// ran for (first pointer event after convergence + expected bounds).
    motion_logged_gen: Mutex<u64>,
}

unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

impl Inner {
    fn diagnostics_snapshot(&self) -> AnlandDiagnostics {
        AnlandDiagnostics {
            queued: self.frames_queued.load(Ordering::Relaxed),
            fenced: self.frames_fenced.load(Ordering::Relaxed),
            bare: self.frames_bare.load(Ordering::Relaxed),
            fallbacks: self.fallback_count.load(Ordering::Relaxed),
            requested: self.work_requested.load(Ordering::Relaxed),
            rejected: self.work_rejected.load(Ordering::Relaxed),
            consumed: self.work_consumed.load(Ordering::Relaxed),
            no_damage: self.frames_no_damage.load(Ordering::Relaxed),
            unknown_slot: self.unknown_slot_cancels.load(Ordering::Relaxed),
            gen_mismatch: self.gen_mismatch_cancels.load(Ordering::Relaxed),
            stale_gen: self.stale_gen_cancels.load(Ordering::Relaxed),
            dequeue_failures: self.dequeue_failures.load(Ordering::Relaxed),
            acquire_timeouts: self.acquire_timeouts.load(Ordering::Relaxed),
            render_timeouts: self.render_timeouts.load(Ordering::Relaxed),
            queue_failures: self.queue_failures.load(Ordering::Relaxed),
            cancel_failures: self.cancel_failures.load(Ordering::Relaxed),
            timeline_hit: self.timeline_hit.load(Ordering::Relaxed),
            timeline_miss: self.timeline_miss.load(Ordering::Relaxed),
            deadline_misses: self.deadline_misses.load(Ordering::Relaxed),
            rebinds: self.rebinds_completed.load(Ordering::Relaxed),
            dequeue_us_total: self.dequeue_us_total.load(Ordering::Relaxed),
            dequeue_us_max: self.dequeue_us_max.load(Ordering::Relaxed),
            acquire_us_total: self.acquire_us_total.load(Ordering::Relaxed),
            acquire_us_max: self.acquire_us_max.load(Ordering::Relaxed),
            render_us_total: self.render_us_total.load(Ordering::Relaxed),
            render_us_max: self.render_us_max.load(Ordering::Relaxed),
        }
    }
}

pub struct AnlandSession {
    inner: Arc<Inner>,
    render_thread: Option<JoinHandle<()>>,
    event_thread: Option<JoinHandle<()>>,
    broker_thread: Option<JoinHandle<()>>,
    /// The winit holder is surface-scoped. The broker/session remains alive
    /// while this is `None` between Android surface lifetimes.
    window_holder: Option<Arc<winit::window::Window>>,
}

pub struct AnlandConfig {
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
    pub socket_path: std::path::PathBuf,
}

/// Inexpensive demand/damage/pacing counter snapshot.
///
/// Counts are cumulative since session start; latency totals are in
/// microseconds (divide by the matching count for an average, max is the
/// worst observed). Damage-region/area detail lives on the KWin side
/// (`anland.damage` logs); this side proves the transport stayed idle on a
/// static desktop and that overload degrades to drops, never to sync
/// shortcuts.
#[derive(Clone, Copy, Debug, Default)]
pub struct AnlandDiagnostics {
    pub queued: u64,
    pub fenced: u64,
    pub bare: u64,
    pub fallbacks: u64,
    pub requested: u64,
    pub rejected: u64,
    pub consumed: u64,
    pub no_damage: u64,
    pub unknown_slot: u64,
    pub gen_mismatch: u64,
    pub stale_gen: u64,
    pub dequeue_failures: u64,
    pub acquire_timeouts: u64,
    pub render_timeouts: u64,
    pub queue_failures: u64,
    pub cancel_failures: u64,
    pub timeline_hit: u64,
    pub timeline_miss: u64,
    pub deadline_misses: u64,
    pub rebinds: u64,
    pub dequeue_us_total: u64,
    pub dequeue_us_max: u64,
    pub acquire_us_total: u64,
    pub acquire_us_max: u64,
    pub render_us_total: u64,
    pub render_us_max: u64,
}

fn record_latency(total: &AtomicU64, max: &AtomicU64, us: u64) {
    total.fetch_add(us, Ordering::Relaxed);
    let mut prev = max.load(Ordering::Relaxed);
    while us > prev {
        match max.compare_exchange_weak(prev, us, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(next) => prev = next,
        }
    }
}

fn dup_owned(fd: &OwnedFd) -> std::io::Result<OwnedFd> {
    let dup = unsafe { libc::dup(fd.as_raw_fd()) };
    if dup < 0 {
        return Err(std::io::Error::last_os_error());
    }
    unsafe { Ok(OwnedFd::from_raw_fd(dup)) }
}

fn close_silently(fd: i32) {
    if fd >= 0 {
        unsafe { libc::close(fd) };
    }
}

fn current_window(inner: &Arc<Inner>) -> Option<*mut c_void> {
    inner.window.lock().ok().and_then(|window| *window)
}

fn install_window(inner: &Arc<Inner>, window: *mut c_void) -> Result<(), String> {
    let mut current = inner
        .window
        .lock()
        .map_err(|_| "Anland surface state is poisoned".to_string())?;
    if current.is_some() {
        return Err("Anland surface is already attached".into());
    }
    *current = Some(window);
    Ok(())
}

fn remove_window(inner: &Arc<Inner>) -> Option<*mut c_void> {
    inner
        .window
        .lock()
        .ok()
        .and_then(|mut window| window.take())
}

fn release_window(inner: &Arc<Inner>) {
    if let Some(window) = remove_window(inner) {
        unsafe { anw::release(window, &inner.anw) };
    }
}

/// Acquire and configure one Android surface. Every failure after acquire
/// releases the reference again, so a failed resume cannot leak or leave a
/// half-configured surface owned by the persistent session.
fn configure_window(
    window: *mut c_void,
    anw: &AnwApi,
    cfg: &AnlandConfig,
) -> Result<(u32, u32, usize), String> {
    unsafe { anw::acquire(window, anw) };
    let result = (|| {
        let (win_w, win_h) = unsafe { (anw::get_width(window, anw), anw::get_height(window, anw)) };
        let (w, h) = if win_w > 0 && win_h > 0 {
            (win_w as u32, win_h as u32)
        } else {
            (cfg.width, cfg.height)
        };
        if w == 0 || h == 0 {
            return Err("Android surface has zero geometry".into());
        }
        log::info!(
            "anland.renderer=anland-gpu window={w}x{h} requested={}x{} format=RGBA_8888",
            cfg.width,
            cfg.height
        );
        // Set the format before the first CPU lock. A newly-created
        // SurfaceView can otherwise expose Android's default RGB_565 format
        // to ANativeWindow_lock, even though the Anland zero-copy contract is
        // strictly RGBA_8888. This keeps the CPU black-buffer proof and the
        // later dequeued-buffer proof on the same format.
        let r = unsafe {
            anw::set_buffers_geometry(window, anw, w as i32, h as i32, anw::FORMAT_RGBA_8888)
        };
        if r != 0 {
            return Err(format!("ANativeWindow_setBuffersGeometry failed: {r}"));
        }
        let r = unsafe { anw.set_buffers_dataspace(window, anw::DATASPACE_SRGB) };
        if r != 0 {
            return Err(format!(
                "ANativeWindow_setBuffersDataSpace(sRGB) failed: {r}"
            ));
        }
        log::info!(
            "anland.color contract format=RGBA_8888 dataspace=ADATASPACE_SRGB ({})",
            anw::DATASPACE_SRGB
        );
        // Connect to the CPU API via the lock/unlock ritual (see anw.rs)
        // only after geometry/format selection. The private in-object
        // perform() table is deliberately not touched.
        unsafe { anw.connect_cpu_ritual(window)? };
        let min_undequeued = unsafe { anw.query_min_undequeued(window) }?;
        let total = (min_undequeued + 2).clamp(3, MAX_BUFS as i32) as usize;
        let r = unsafe { anw.set_buffer_count(window, total) };
        if r != 0 {
            return Err(format!("ANativeWindow_setBufferCount({total}) failed: {r}"));
        }
        // The CPU connection ritual above proves the safe connection path.
        // Clear one buffer again after the final geometry/count configuration
        // so the first frame held while the producer handshakes is known
        // black for the actual presentation dimensions.
        unsafe { anw.clear_cpu_buffer(window)? };
        Ok((w, h, total))
    })();
    if result.is_err() {
        unsafe { anw::release(window, anw) };
    }
    result
}

impl AnlandSession {
    fn spawn_broker_thread(inner: &Arc<Inner>) -> Result<JoinHandle<()>, String> {
        let broker = inner.broker.clone();
        let shutdown = inner.broker_stop.clone();
        thread::Builder::new()
            .name("anland-broker".into())
            .spawn(move || {
                if let Err(error) = broker.serve(shutdown) {
                    log::warn!("anland.broker serve ended: {error}");
                }
            })
            .map_err(|error| format!("spawn broker thread: {error}"))
    }

    fn ensure_broker_thread(&mut self) -> Result<(), String> {
        if self
            .broker_thread
            .as_ref()
            .is_some_and(|thread| thread.is_finished())
        {
            if let Some(thread) = self.broker_thread.take() {
                join_surface_thread(thread, "broker-recovery");
            }
            self.inner.broker_stop.store(false, Ordering::Release);
            self.broker_thread = Some(Self::spawn_broker_thread(&self.inner)?);
            log::info!("anland.broker listener restarted before surface resume");
        }
        if self.broker_thread.is_none() {
            return Err("Anland broker listener is unavailable".into());
        }
        Ok(())
    }

    /// Take over `window` for zero-copy GPU presentation.
    ///
    /// `window_holder` keeps the winit Window alive for this surface lifetime.
    /// The caller must guarantee `window` is a valid, current `ANativeWindow*`
    /// and must call [`Self::stop`] while it is still valid (Portal's
    /// `suspended()` runs while the lifecycle window is alive).
    pub fn start(
        window: *mut c_void,
        window_holder: Arc<winit::window::Window>,
        cfg: &AnlandConfig,
    ) -> Result<Self, String> {
        let anw = unsafe { AnwApi::load() }?;
        if let Some(parent) = cfg.socket_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create anland socket dir: {e}"))?;
        }
        let (w, h, total) = configure_window(window, &anw, cfg)?;
        let screen = ScreenInfo {
            width: w,
            height: h,
            format: PIXEL_FORMAT_RGBA_8888,
            refresh: cfg.refresh_mhz,
        };
        let broker = Arc::new(Broker::new(screen, cfg.socket_path.clone()));
        // Presentation pacing: control wake + display-VSYNC tick source.
        // Pump failure fails the session loudly (never a silent free-spin).
        let wake = match sys::make_eventfd() {
            Ok(wake) => wake,
            Err(error) => {
                unsafe { anw::release(window, &anw) };
                return Err(format!("wake eventfd: {error}"));
            }
        };
        let vsync = match sys::VsyncPump::start(cfg.refresh_mhz) {
            Ok(vsync) => vsync,
            Err(error) => {
                unsafe { anw::release(window, &anw) };
                return Err(format!("vsync pump: {error}"));
            }
        };
        // Surface epoch for rotation convergence: delayed resize events from
        // a previous native window carry the old epoch and are rejected.
        // This session converges as surface generation 1; rotations mint 2+.
        let surface_epoch = surface_geometry::mint_surface_epoch();
        let inner = Arc::new(Inner {
            window: Mutex::new(Some(window)),
            anw,
            broker: broker.clone(),
            running: AtomicBool::new(true),
            window_live: AtomicBool::new(true),
            broker_stop: Arc::new(AtomicBool::new(false)),
            gen: Mutex::new(None),
            io_lock: Mutex::new(()),
            next_gen: Mutex::new(1),
            buffers: Mutex::new(Vec::new()),
            spare: Mutex::new(None),
            screen: Mutex::new((w, h)),
            refresh_mhz: cfg.refresh_mhz,
            surface_epoch: AtomicU64::new(surface_epoch),
            surface_gen: Mutex::new(1),
            rebind_active: AtomicBool::new(false),
            rebind_request: Mutex::new(None),
            transition_lock: Mutex::new(()),
            connected_gen: Mutex::new(None),
            work: Mutex::new(FrameWorkState::default()),
            surface_recovery_requested: AtomicBool::new(false),
            frames_queued: AtomicU64::new(0),
            frames_fenced: AtomicU64::new(0),
            frames_bare: AtomicU64::new(0),
            fallback_count: AtomicU64::new(0),
            work_requested: AtomicU64::new(0),
            work_rejected: AtomicU64::new(0),
            work_consumed: AtomicU64::new(0),
            frames_no_damage: AtomicU64::new(0),
            unknown_slot_cancels: AtomicU64::new(0),
            gen_mismatch_cancels: AtomicU64::new(0),
            stale_gen_cancels: AtomicU64::new(0),
            dequeue_failures: AtomicU64::new(0),
            acquire_timeouts: AtomicU64::new(0),
            render_timeouts: AtomicU64::new(0),
            queue_failures: AtomicU64::new(0),
            cancel_failures: AtomicU64::new(0),
            timeline_hit: AtomicU64::new(0),
            timeline_miss: AtomicU64::new(0),
            deadline_misses: AtomicU64::new(0),
            rebinds_completed: AtomicU64::new(0),
            dequeue_us_total: AtomicU64::new(0),
            dequeue_us_max: AtomicU64::new(0),
            acquire_us_total: AtomicU64::new(0),
            acquire_us_max: AtomicU64::new(0),
            render_us_total: AtomicU64::new(0),
            render_us_max: AtomicU64::new(0),
            ready_surface_gen: Mutex::new(None),
            software_gl: super::software_gl_fallback_requested(),
            wake,
            last_pointer: Mutex::new(None),
            finger_axes: Mutex::new(0),
            held_inputs: Mutex::new(std::collections::BTreeSet::new()),
            vsync: Mutex::new(Some(vsync)),
            motion_logged_gen: Mutex::new(0),
        });
        // Collect window slots (dup dma-buf fds, hold one spare back).
        if let Err(error) = collect_buffers(&inner, total, w, h) {
            cleanup_failed_session(&inner);
            return Err(error);
        }
        // First generation: fresh fds + deposit at the broker.
        if let Err(error) = deposit_generation(&inner) {
            cleanup_failed_session(&inner);
            return Err(error);
        }
        // The broker is session-scoped. Its listener intentionally outlives
        // the presentation surface so KWin's producer can remain alive and
        // use its normal fallback/reconnect path while Android owns no
        // window.
        let broker_thread = match Self::spawn_broker_thread(&inner) {
            Ok(thread) => thread,
            Err(error) => {
                cleanup_failed_session(&inner);
                return Err(error);
            }
        };
        let mut session = Self {
            inner,
            render_thread: None,
            event_thread: None,
            broker_thread: Some(broker_thread),
            window_holder: Some(window_holder),
        };
        if let Err(error) = session.start_surface_threads() {
            session.stop();
            return Err(error);
        }
        let inner = session.inner.clone();
        log::info!(
            "anland.session=start screen={}x{} fmt=RGBA_8888 refresh_mhz={} bufs={} socket={} epoch={surface_epoch} sgen=1",
            w,
            h,
            cfg.refresh_mhz,
            inner.buffers.lock().map(|b| b.len()).unwrap_or(0),
            cfg.socket_path.display()
        );
        log::info!("anland.qpainter_path=disabled (Smithay SHM upload not used in Anland mode)");
        session.log_pacing();
        Ok(session)
    }

    fn start_surface_threads(&mut self) -> Result<(), String> {
        let inner = self.inner.clone();
        let render_thread = thread::Builder::new()
            .name("anland-render".into())
            .spawn({
                let inner = inner.clone();
                move || render_loop(inner)
            })
            .map_err(|e| format!("spawn render thread: {e}"))?;
        self.render_thread = Some(render_thread);

        let event_thread = thread::Builder::new()
            .name("anland-event".into())
            .spawn(move || event_loop(inner))
            .map_err(|e| format!("spawn event thread: {e}"))?;
        self.event_thread = Some(event_thread);
        Ok(())
    }

    fn log_pacing(&self) {
        if let Ok(vsync) = self.inner.vsync.lock() {
            if let Some(pump) = vsync.as_ref() {
                log::info!(
                    "anland.pacing work-driven vsync={} period_ns={}",
                    pump.mode(),
                    pump.period_ns()
                );
            }
        }
    }

    /// Reattach a newly-created Android surface to this still-running
    /// Anland session. The broker, KWin producer and guest Plasma process are
    /// deliberately not recreated here; only the surface generation is.
    pub fn resume_surface(
        &mut self,
        window: *mut c_void,
        window_holder: Arc<winit::window::Window>,
        cfg: &AnlandConfig,
    ) -> Result<(u32, u32), String> {
        let inner = self.inner.clone();
        if !inner.running.load(Ordering::Acquire) {
            return Err("Anland session is permanently stopped".into());
        }
        if inner.window_live.load(Ordering::Acquire) || current_window(&inner).is_some() {
            return Err("Anland surface is already attached".into());
        }
        self.ensure_broker_thread()?;
        let (w, h, total) = configure_window(window, &inner.anw, cfg)?;
        if let Err(error) = install_window(&inner, window) {
            unsafe { anw::release(window, &inner.anw) };
            return Err(error);
        }
        inner.window_live.store(true, Ordering::Release);
        inner
            .surface_recovery_requested
            .store(false, Ordering::Release);

        let epoch = surface_geometry::mint_surface_epoch();
        inner.surface_epoch.store(epoch, Ordering::Release);
        *inner.surface_gen.lock().unwrap() = 1;
        *inner.last_pointer.lock().unwrap() = None;
        *inner.finger_axes.lock().unwrap() = 0;
        *inner.ready_surface_gen.lock().unwrap() = None;
        *inner.motion_logged_gen.lock().unwrap() = 0;
        *inner.rebind_request.lock().unwrap() = None;
        inner.rebind_active.store(false, Ordering::Release);
        *inner.screen.lock().unwrap() = (w, h);
        inner.broker.set_screen(ScreenInfo {
            width: w,
            height: h,
            format: PIXEL_FORMAT_RGBA_8888,
            refresh: inner.refresh_mhz,
        });

        let vsync = match sys::VsyncPump::start(inner.refresh_mhz) {
            Ok(vsync) => vsync,
            Err(error) => {
                self.suspend_surface();
                return Err(format!("vsync pump after Android surface resume: {error}"));
            }
        };
        *inner.vsync.lock().unwrap() = Some(vsync);

        // The producer is already alive in the guest. Withdrawn generation
        // fds make it enter its normal fallback; this fresh set lets its
        // existing try_exit_fallback/reconnect path recover onto the new
        // BufferQueue without another launch() call.
        if let Err(error) = collect_buffers(&inner, total, w, h) {
            self.suspend_surface();
            return Err(format!(
                "collect buffers after Android surface resume: {error}"
            ));
        }
        if let Err(error) = deposit_generation(&inner) {
            self.suspend_surface();
            return Err(format!(
                "deposit Anland generation after surface resume: {error}"
            ));
        }
        if let Err(error) = self.start_surface_threads() {
            self.suspend_surface();
            return Err(error);
        }

        self.window_holder = Some(window_holder);
        log::info!(
            "anland.surface=resumed epoch={epoch} sgen=1 screen={w}x{h} bufs={total}; guest session preserved"
        );
        self.log_pacing();
        Ok((w, h))
    }

    fn release_surface_inputs(&self) {
        if !self.inner.window_live.load(Ordering::Acquire) {
            return;
        }
        self.send_finger_stops();
        let held = std::mem::take(&mut *self.inner.held_inputs.lock().unwrap());
        for (kind, code) in held {
            let release = if kind == INPUT_TYPE_KEY {
                InputEvent::key(INPUT_ACTION_UP, code)
            } else {
                InputEvent::pointer_button(code as u32, false)
            };
            self.send_input(&release);
        }
    }

    /// Retire only the current Android presentation surface. This is
    /// intentionally distinct from [`Self::stop`]: broker/listener state and
    /// the guest KWin/Plasma process survive the Android suspend callback.
    pub fn suspend_surface(&mut self) {
        let has_surface = current_window(&self.inner).is_some()
            || self.render_thread.is_some()
            || self.event_thread.is_some()
            || self
                .inner
                .vsync
                .lock()
                .map(|vsync| vsync.is_some())
                .unwrap_or(false);
        if !has_surface {
            self.inner.window_live.store(false, Ordering::Release);
            return;
        }

        // The old channel is still live here, so all delivered keys/buttons
        // and active finger axes get their matching release/stop events.
        self.release_surface_inputs();
        self.inner.window_live.store(false, Ordering::Release);
        self.inner.rebind_active.store(false, Ordering::Release);
        self.inner.rebind_request.lock().unwrap().take();
        let _ = sys::eventfd_write(&self.inner.wake, 1);

        // Return the one dequeued spare before joining. This is the explicit
        // unblock for a render thread that is inside dequeueBuffer. The
        // render thread is joined before the native pointer is released;
        // detaching a surface-bound worker would make use-after-destroy
        // possible.
        if let Some(window) = current_window(&self.inner) {
            if let Ok(mut spare) = self.inner.spare.lock() {
                if let Some(anb) = spare.take() {
                    unsafe { self.inner.anw.cancel(window, anb, -1) };
                }
            }
        }
        if let Some(handle) = self.render_thread.take() {
            join_surface_thread(handle, "render");
        }
        if let Ok(mut vsync) = self.inner.vsync.lock() {
            if let Some(pump) = vsync.take() {
                pump.stop();
            }
        }
        // Closing the generation after render quiescence withdraws the
        // producer deposit and wakes its existing fallback/reconnect logic.
        teardown_generation(&self.inner);
        if let Some(handle) = self.event_thread.take() {
            join_surface_thread(handle, "event");
        }
        // These slots were queued back to Android; clear only our duplicate
        // dma-buf metadata. Never cancel the collected queued slots.
        self.inner.buffers.lock().unwrap().clear();
        *self.inner.ready_surface_gen.lock().unwrap() = None;
        *self.inner.last_pointer.lock().unwrap() = None;
        *self.inner.finger_axes.lock().unwrap() = 0;
        *self.inner.surface_gen.lock().unwrap() = 0;

        // No worker can touch the pointer now. Release our reference and
        // remove it from the persistent session before dropping the winit
        // holder, so later callbacks cannot observe the old surface.
        release_window(&self.inner);
        self.window_holder.take();
        self.inner.surface_epoch.store(0, Ordering::Release);
        log::info!("anland.surface=suspended broker=preserved guest=preserved");
    }

    /// Forward one fixed-size input event to the producer. No-op unless the
    /// current generation completed its BUFS_READY push.
    pub fn send_input(&self, ev: &InputEvent) {
        let inner = &self.inner;
        if !inner.running.load(Ordering::Acquire)
            || !inner.window_live.load(Ordering::Acquire)
            || inner.surface_recovery_requested.load(Ordering::Acquire)
        {
            return;
        }
        if inner.rebind_active.load(Ordering::Acquire) {
            // A rotation rebind owns the surface right now: events observed
            // against the obsolete geometry must not mutate the new state.
            log::debug!("anland.input dropped during surface rebind");
            return;
        }
        // Input is forwarded immediately; presentation is independently
        // authorized by the producer's FRAME_WANTED request.
        let _guard = inner.io_lock.lock().unwrap();
        let gen_guard = inner.gen.lock().unwrap();
        let Some(gen) = gen_guard.as_ref() else {
            return;
        };
        if inner.connected_gen.lock().unwrap().as_ref() != Some(&gen.id) {
            return;
        }
        let mut wire = [0u8; 8 + 20];
        wire[0..4].copy_from_slice(&DATA_MSG_INPUT_EVENT.to_ne_bytes());
        wire[4..8].copy_from_slice(&20u32.to_ne_bytes());
        wire[8..12].copy_from_slice(&ev.ev_type.to_ne_bytes());
        wire[12..28].copy_from_slice(&ev.payload);
        if sys::send_all(&gen.data, &wire).is_err() {
            let failed_gen = gen.id;
            drop(gen_guard);
            drop(_guard);
            enter_fallback(inner, failed_gen, "input send failed");
        } else if let Some((code, pressed)) = ev.press_edge() {
            let mut held = inner.held_inputs.lock().unwrap();
            if pressed {
                held.insert((ev.ev_type, code));
            } else {
                held.remove(&(ev.ev_type, code));
            }
        }
    }

    /// Forward absolute pointer motion with session-synthesized relative
    /// deltas. winit carries only absolute positions; the KWin backend emits
    /// both absolute and relative motion per event, and relative clients
    /// (plus KWin's velocity/kinetic path) need real dx/dy — zeroed deltas
    /// left the touchpad cursor frozen. First motion after session start (or
    /// a jump) reports zero delta, never a spike.
    pub fn send_pointer_motion(&self, x: f32, y: f32) {
        let inner = &self.inner;
        if !inner.running.load(Ordering::Acquire)
            || !inner.window_live.load(Ordering::Acquire)
            || inner.surface_recovery_requested.load(Ordering::Acquire)
        {
            return;
        }
        if inner.rebind_active.load(Ordering::Acquire) {
            log::debug!("anland.input motion dropped during surface rebind");
            return;
        }
        // Single event-loop thread: every event processed between rebinds
        // belongs to the converged generation, and each rebind clears the
        // anchor — so the pure anchor fold below only ever re-anchors on a
        // genuine generation change (session restart or missed clear),
        // reporting zero delta instead of a cross-boundary jump.
        let cur = self.surface_gen();
        let (next, decision) = {
            let prev = *inner.last_pointer.lock().unwrap();
            surface_geometry::anchor_motion(prev, x, y, cur, cur)
        };
        *inner.last_pointer.lock().unwrap() = next;
        let surface_geometry::MotionDecision::Send { dx, dy } = decision else {
            return;
        };
        {
            let mut logged = inner.motion_logged_gen.lock().unwrap();
            if *logged != cur {
                *logged = cur;
                let (sw, sh) = *inner.screen.lock().unwrap();
                log::info!(
                    "anland.rotate sgen={cur} first-motion x={x:.1} y={y:.1} bounds=0,0-{sw}x{sh}"
                );
            }
        }
        self.send_input(&InputEvent::pointer_motion(x, y, dx, dy));
    }

    /// Forward committed IME text (UTF-8) to KWin's input method
    /// (`inputMethod()->commitText()`). Framing: the type-9 header plus the
    /// raw bytes as a second write, mirroring the clipboard convention the
    /// producer's `poll_input_event_extend_data` expects. Returns false when
    /// no generation is connected (caller falls back: bridge FIFO, keys).
    pub fn send_text(&self, text: &str) -> bool {
        if text.is_empty() || text.len() > 4096 {
            return false;
        }
        let inner = &self.inner;
        if !inner.running.load(Ordering::Acquire)
            || !inner.window_live.load(Ordering::Acquire)
            || inner.surface_recovery_requested.load(Ordering::Acquire)
        {
            return false;
        }
        if inner.rebind_active.load(Ordering::Acquire) {
            return false;
        }
        let _guard = inner.io_lock.lock().unwrap();
        let gen_guard = inner.gen.lock().unwrap();
        let Some(gen) = gen_guard.as_ref() else {
            return false;
        };
        if inner.connected_gen.lock().unwrap().as_ref() != Some(&gen.id) {
            return false;
        }
        let body = InputEvent::text_input(text.len() as u32);
        let mut wire = [0u8; 8 + 20];
        wire[0..4].copy_from_slice(&DATA_MSG_INPUT_EVENT.to_ne_bytes());
        wire[4..8].copy_from_slice(&20u32.to_ne_bytes());
        wire[8..12].copy_from_slice(&body.ev_type.to_ne_bytes());
        wire[12..28].copy_from_slice(&body.payload);
        if sys::send_all(&gen.data, &wire).is_err()
            || sys::send_all(&gen.data, text.as_bytes()).is_err()
        {
            let failed_gen = gen.id;
            drop(gen_guard);
            drop(_guard);
            enter_fallback(inner, failed_gen, "text send failed");
            return false;
        }
        log::info!("anland.input text committed ({} bytes)", text.len());
        true
    }

    /// Forward one touchpad finger-scroll value (axis 0 = vertical,
    /// 1 = horizontal) and mark the stream live for stop events.
    pub fn send_finger_axis(&self, axis: u32, value: f32) {
        if !self.surface_active() {
            return;
        }
        if axis < 2 {
            if let Ok(mut live) = self.inner.finger_axes.lock() {
                *live |= 1u8 << axis;
            }
        }
        self.send_input(&InputEvent::finger_axis(axis, value));
    }

    /// Terminate live touchpad finger-scroll streams (both axes if active).
    /// Called on scroll end/cancel so KWin emits axis-stop and kinetic
    /// scrolling settles instead of hanging mid-gesture.
    pub fn send_finger_stops(&self) {
        let live = self
            .inner
            .finger_axes
            .lock()
            .map(|mut live| std::mem::replace(&mut *live, 0))
            .unwrap_or(0);
        for axis in 0..2u32 {
            if live & (1u8 << axis) != 0 {
                self.send_input(&InputEvent::finger_stop(axis));
            }
        }
    }

    /// Current proof counters: (queued, fenced, bare, fallbacks).
    pub fn stats(&self) -> (u64, u64, u64, u64) {
        (
            self.inner.frames_queued.load(Ordering::Relaxed),
            self.inner.frames_fenced.load(Ordering::Relaxed),
            self.inner.frames_bare.load(Ordering::Relaxed),
            self.inner.fallback_count.load(Ordering::Relaxed),
        )
    }

    /// Full demand/damage/pacing counter snapshot (inexpensive relaxed loads).
    pub fn diagnostics(&self) -> AnlandDiagnostics {
        self.inner.diagnostics_snapshot()
    }

    pub fn screen_size(&self) -> (u32, u32) {
        *self.inner.screen.lock().unwrap()
    }

    /// Native-window epoch minted at each surface attachment (stale-event
    /// rejection). Zero means the persistent session is surface-suspended.
    pub fn surface_epoch(&self) -> u64 {
        self.inner.surface_epoch.load(Ordering::Acquire)
    }

    /// Currently converged presentation surface generation.
    pub fn surface_gen(&self) -> u64 {
        *self.inner.surface_gen.lock().unwrap()
    }

    /// Whether a valid Android presentation surface is currently attached.
    /// The broker and guest session remain alive while this is false.
    pub fn surface_active(&self) -> bool {
        self.inner.window_live.load(Ordering::Acquire)
    }

    /// Whether the attached surface still has all of its session-owned
    /// workers. A worker can terminate because of an unexpected pacing or
    /// broker failure; treating that dead session as an active surface would
    /// make a later resume a no-op and leave a black BufferQueue.
    pub fn surface_healthy(&self) -> bool {
        self.surface_active()
            && !self
                .inner
                .surface_recovery_requested
                .load(Ordering::Acquire)
            && self
                .render_thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
            && self
                .event_thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
            && self
                .broker_thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
    }

    /// Raw `ANativeWindow*` this session owns for the current surface. It is
    /// null between Android surface lifetimes and must never be retained by a
    /// caller after the next suspend callback.
    pub fn native_window_ptr(&self) -> *mut c_void {
        current_window(&self.inner).unwrap_or(std::ptr::null_mut())
    }

    /// Live `ANativeWindow` size. Read on every rotation transaction: the
    /// winit event size and this must agree before the new geometry is
    /// treated as authoritative.
    pub fn native_window_size(&self) -> (i32, i32) {
        if !self.surface_active() {
            return (0, 0);
        }
        let Some(window) = current_window(&self.inner) else {
            return (0, 0);
        };
        unsafe {
            (
                anw::get_width(window, &self.inner.anw),
                anw::get_height(window, &self.inner.anw),
            )
        }
    }

    /// winit's view of the window size, if the holder is still alive.
    pub fn window_inner_size(&self) -> Option<(u32, u32)> {
        self.window_holder.as_ref().map(|w| {
            let s = w.inner_size();
            (s.width, s.height)
        })
    }

    /// Screen size currently published to producers via the broker.
    pub fn broker_screen(&self) -> ScreenInfo {
        self.inner.broker.screen()
    }

    /// Queue a resize for the render thread, which owns native buffer operations.
    pub fn rebind_surface(&self, w: u32, h: u32, gen: u64) -> Result<u64, String> {
        let inner = &self.inner;
        if w == 0
            || h == 0
            || !inner.running.load(Ordering::Acquire)
            || !inner.window_live.load(Ordering::Acquire)
            || inner.surface_recovery_requested.load(Ordering::Acquire)
            || current_window(inner).is_none()
        {
            return Err("invalid geometry or session stopping".into());
        }
        // End scroll streams while the old channel is still valid.
        self.send_finger_stops();
        let held = std::mem::take(&mut *inner.held_inputs.lock().unwrap());
        for (kind, code) in held {
            let release = if kind == INPUT_TYPE_KEY {
                InputEvent::key(INPUT_ACTION_UP, code)
            } else {
                InputEvent::pointer_button(code as u32, false)
            };
            self.send_input(&release);
        }
        let mut request = inner.rebind_request.lock().unwrap();
        inner.rebind_active.store(true, Ordering::Release);
        *request = Some((w, h, gen));
        // Wake the render thread so it observes the rebind promptly. This is
        // a control interrupt only; the next presentation still waits for
        // Android VSYNC.
        let _ = sys::eventfd_write(&inner.wake, 1);
        Ok(gen)
    }

    pub fn presented_surface_gen(&self) -> Option<u64> {
        *self.inner.ready_surface_gen.lock().unwrap()
    }

    /// Render-thread-only transaction. Shutdown the old connection, release
    /// our spare, resize and recollect, then publish one complete buffer set.
    /// KWin resizes its existing output from BUFS_READY buffer dimensions on
    /// import; SCREEN_INFO is only used by a new producer's initial hello.
    fn rebind_surface_inner(inner: &Arc<Inner>, w: u32, h: u32, gen: u64) -> Result<u64, String> {
        let Some(window) = current_window(inner) else {
            return Err("Android surface is no longer attached".into());
        };
        if !inner.window_live.load(Ordering::Acquire)
            || inner.surface_recovery_requested.load(Ordering::Acquire)
        {
            return Err("Android surface is suspended".into());
        }
        let (old_w, old_h) = *inner.screen.lock().unwrap();
        let cur_sgen = *inner.surface_gen.lock().unwrap();
        if gen < cur_sgen {
            // Obsolete attempt (a newer transition already converged or is
            // being attempted): never rewind the generation.
            return Err(format!("stale rebind gen={gen} current={cur_sgen}"));
        }
        // Packed struct: copy fields to locals before use (no field borrows).
        let (bw, bh) = {
            let p = inner.broker.screen();
            (p.width, p.height)
        };
        log::info!(
            "anland.rotate sgen={gen} begin old={old_w}x{old_h} new={w}x{h} ptr={:p} epoch={} broker={bw}x{bh}",
            window,
            inner.surface_epoch.load(Ordering::Acquire),
        );
        // No frame is in flight: this runs between render-loop iterations.
        teardown_generation(inner);
        // Only the held spare is dequeued and may be cancelled.
        if let Ok(mut spare) = inner.spare.lock() {
            if let Some(anb) = spare.take() {
                unsafe { inner.anw.cancel(window, anb, -1) };
            }
        }
        // Collected slots are QUEUED, not owned/dequeued: cancelling them
        // violates BufferQueue's ownership contract. Only the spare is ours.
        inner.buffers.lock().unwrap().clear();
        // Publish the new size before touching the queue: any producer hello
        // from here on (reconnect, fresh KWin, wrapper relaunch) observes
        // the current geometry. Plasma scale is untouched (still governed
        // by kwinoutputconfig, synced on the resize path as before).
        inner.broker.set_screen(ScreenInfo {
            width: w,
            height: h,
            format: PIXEL_FORMAT_RGBA_8888,
            refresh: inner.refresh_mhz,
        });
        let r = unsafe {
            anw::set_buffers_geometry(
                window,
                &inner.anw,
                w as i32,
                h as i32,
                anw::FORMAT_RGBA_8888,
            )
        };
        if r != 0 {
            return Err(format!(
                "ANativeWindow_setBuffersGeometry({w}x{h}) failed: {r}"
            ));
        }
        let r = unsafe { inner.anw.set_buffers_dataspace(window, anw::DATASPACE_SRGB) };
        if r != 0 {
            return Err(format!(
                "ANativeWindow_setBuffersDataSpace(sRGB) after resize failed: {r}"
            ));
        }
        let min_undequeued = unsafe { inner.anw.query_min_undequeued(window) }
            .map_err(|e| format!("ANativeWindow query min-undequeued after resize: {e}"))?;
        let total = (min_undequeued + 2).clamp(3, MAX_BUFS as i32) as usize;
        let r = unsafe { inner.anw.set_buffer_count(window, total) };
        if r != 0 {
            return Err(format!("ANativeWindow_setBufferCount({total}) failed: {r}"));
        }
        // Fresh slots for the new geometry (old handles are all returned
        // above; a failed collect leaves the failure pending so the next
        // event retries with the producer waiting in fallback).
        collect_buffers(inner, total, w, h)
            .map_err(|e| format!("collect after resize to {w}x{h}: {e}"))?;
        *inner.screen.lock().unwrap() = (w, h);
        *inner.surface_gen.lock().unwrap() = gen;
        *inner.last_pointer.lock().unwrap() = None;
        *inner.finger_axes.lock().unwrap() = 0;
        *inner.ready_surface_gen.lock().unwrap() = None;
        *inner.motion_logged_gen.lock().unwrap() = 0;
        deposit_generation(inner).map_err(|e| format!("re-deposit generation: {e}"))?;
        inner.rebinds_completed.fetch_add(1, Ordering::Relaxed);
        let bufs = inner.buffers.lock().map(|b| b.len()).unwrap_or(0);
        log::info!(
            "anland.rotate sgen={gen} bound bufs={bufs} screen={w}x{h}; awaiting producer attach + first frame"
        );
        Ok(gen)
    }

    /// Permanently stop the session. Unlike [`Self::suspend_surface`], this
    /// also tears down the persistent broker/listener. Normal Android
    /// suspend/resume must never call this method.
    pub fn stop(mut self) {
        self.suspend_surface();
        let inner = self.inner.clone();
        inner.running.store(false, Ordering::Release);
        if let Some(h) = self.broker_thread.take() {
            inner.broker_stop.store(true, Ordering::Release);
            join_surface_thread(h, "broker");
        }
        let (q, f, b, fb) = (
            inner.frames_queued.load(Ordering::Relaxed),
            inner.frames_fenced.load(Ordering::Relaxed),
            inner.frames_bare.load(Ordering::Relaxed),
            inner.fallback_count.load(Ordering::Relaxed),
        );
        let d = (
            inner.work_requested.load(Ordering::Relaxed),
            inner.work_consumed.load(Ordering::Relaxed),
            inner.frames_no_damage.load(Ordering::Relaxed),
            inner.acquire_timeouts.load(Ordering::Relaxed),
            inner.render_timeouts.load(Ordering::Relaxed),
            inner.deadline_misses.load(Ordering::Relaxed),
            inner.rebinds_completed.load(Ordering::Relaxed),
        );
        log::info!(
            "anland.session=stop queued={q} fenced={f} bare={b} fallbacks={fb} requested={} consumed={} no_damage={} acquire_timeouts={} render_timeouts={} deadline_misses={} rebinds={}",
            d.0, d.1, d.2, d.3, d.4, d.5, d.6
        );
    }
}

/// Clean up a session whose surface was configured but whose persistent
/// session could not finish starting. This path is only used before a fully
/// constructed [`AnlandSession`] is returned, so it may stop the session
/// state outright. It mirrors the normal surface teardown ordering: workers
/// are absent, the generation is withdrawn before the native window is
/// released, and any detached handshake waiter observes `running=false`.
fn cleanup_failed_session(inner: &Arc<Inner>) {
    inner.running.store(false, Ordering::Release);
    inner.window_live.store(false, Ordering::Release);
    inner.broker_stop.store(true, Ordering::Release);
    inner.rebind_active.store(false, Ordering::Release);
    inner.rebind_request.lock().unwrap().take();
    if let Ok(mut vsync) = inner.vsync.lock() {
        if let Some(pump) = vsync.take() {
            pump.stop();
        }
    }
    if let Some(window) = current_window(inner) {
        if let Ok(mut spare) = inner.spare.lock() {
            if let Some(anb) = spare.take() {
                unsafe { inner.anw.cancel(window, anb, -1) };
            }
        }
    }
    teardown_generation(inner);
    inner.buffers.lock().unwrap().clear();
    *inner.surface_gen.lock().unwrap() = 0;
    *inner.ready_surface_gen.lock().unwrap() = None;
    *inner.last_pointer.lock().unwrap() = None;
    *inner.finger_axes.lock().unwrap() = 0;
    release_window(inner);
    inner.surface_epoch.store(0, Ordering::Release);
}

/// Surface-bound threads are joined rather than detached. A detached render
/// worker could retain an old `ANativeWindow*` past Android's destruction of
/// the surface, which is never an acceptable recovery strategy.
fn join_surface_thread(handle: JoinHandle<()>, what: &str) {
    if let Err(error) = handle.join() {
        log::error!("anland.{what} thread panicked while stopping: {error:?}");
    }
}

/// Dequeue/rotate all window slots, dup their dma-buf fds, and hold one
/// spare dequeued (in our hands, never queued) for stop-time unblocking.
fn collect_buffers(
    inner: &Arc<Inner>,
    total: usize,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let Some(window) = current_window(inner) else {
        return Err("cannot collect buffers without an Android surface".into());
    };
    let queue = NativeBufferQueue {
        window,
        api: &inner.anw,
    };
    let need_producer = total.saturating_sub(1).max(2);
    let mut found: Vec<SlotInfo> = Vec::new();
    for attempt in 0..total * 4 + 2 {
        if found.len() >= need_producer {
            break;
        }
        let (anb, fence) = unsafe { inner.anw.dequeue(window) }
            .map_err(|r| format!("collect dequeueBuffer failed on attempt {attempt}: {r}"))?;
        let mut slot = DequeuedBuffer::new(anb, fence, &queue);
        if let Err(error) = slot.wait_for_acquire(ACQUIRE_WAIT_MS) {
            // Enumeration is not allowed to discard an unsignalled acquire
            // dependency.  The lease transfers the original fence to
            // cancelBuffer, even when the wait timed out.
            let _ = slot.cancel(None);
            return Err(format!(
                "collect acquire fence failed on attempt {attempt}: {error}"
            ));
        }
        let layout = match unsafe { AnwApi::buffer_dma_info(anb) } {
            Ok(layout) => layout,
            Err(reject) => {
                let _ = slot.cancel(None);
                log::warn!(
                    "anland.collect buffer rejected: require one RGBA_8888 linear-plane handle; metadata={reject:?}"
                );
                continue;
            }
        };
        let dma_fd = layout.fd;
        let stride_px = layout.stride_px;
        let bw = layout.width;
        let bh = layout.height;
        if bw != width as i32 || bh != height as i32 {
            let _ = slot.cancel(None);
            return Err(format!(
                "buffer geometry {bw}x{bh} differs from requested {width}x{height}"
            ));
        }
        let stride_bytes = (stride_px as u32)
            .checked_mul(4)
            .ok_or_else(|| format!("buffer stride overflows RGBA byte stride: {stride_px}"))?;
        // Dedup by slot pointer (stable per queue slot).
        if found.iter().any(|s: &SlotInfo| s.anb == anb) {
            let queue_result = slot.queue_initialized();
            if queue_result != 0 {
                return Err(format!(
                    "collect queueBuffer failed while rotating duplicate slot: {queue_result}"
                ));
            }
            continue;
        }
        let dup = unsafe { libc::dup(dma_fd) };
        if dup < 0 {
            let _ = slot.cancel(None);
            continue;
        }
        // Post back so the next dequeue rotates to another slot. The slot was
        // initialized through the synchronous CPU black-buffer ritual before
        // collection; the acquire fence was waited above, so -1 is correct.
        let queue_result = slot.queue_initialized();
        if queue_result != 0 {
            close_silently(dup);
            return Err(format!("collect queueBuffer failed: {queue_result}"));
        }
        // SAFETY: dup is a fresh fd.
        let fd = unsafe { OwnedFd::from_raw_fd(dup) };
        let info = BufInfo {
            stride: stride_bytes,
            width: bw as u32,
            height: bh as u32,
            format: layout.format as u32,
            modifier: layout.modifier,
            offset: 0,
        };
        log::info!(
            "anland.collect buf[{}]: {}x{} stride_px={} fd={} handle_fds={} handle_ints={} modifier={:#x}",
            found.len(),
            bw,
            bh,
            stride_px,
            fd.as_raw_fd(),
            layout.num_fds,
            layout.num_ints,
            layout.modifier,
        );
        found.push(SlotInfo { anb, fd, info });
    }
    if found.len() < need_producer {
        return Err(format!(
            "collect_buffers: only {}/{} producer slots (need {})",
            found.len(),
            total,
            need_producer
        ));
    }
    // Hold one extra slot dequeued as the stop-time spare: with it in our
    // hands, stop() can always cancelBuffer to unblock a stuck dequeue.
    // The spare never reaches the producer and is never queued mid-run; if
    // the render loop dequeues it at runtime it is handed straight back.
    let spare = match unsafe { inner.anw.dequeue(window) } {
        Ok((anb, fence)) => {
            let mut slot = DequeuedBuffer::new(anb, fence, &queue);
            match slot.wait_for_acquire(ACQUIRE_WAIT_MS) {
                Ok(()) => {
                    // The held spare is stored as a raw pointer for the
                    // existing lifecycle handoff.  Its acquire dependency
                    // has been fully satisfied before the lease is forgotten,
                    // so later cancelBuffer(..., -1) is correct.
                    log::info!("anland.collect spare held dequeued (stop-time unblock)");
                    Some(slot.into_held())
                }
                Err(error) => {
                    let _ = slot.cancel(None);
                    log::warn!("anland.collect spare acquire fence failed: {error}");
                    None
                }
            }
        }
        Err(r) => {
            log::warn!("anland.collect no spare slot available ({r}); stop may block briefly");
            None
        }
    };
    *inner.buffers.lock().unwrap() = found;
    *inner.spare.lock().unwrap() = spare;
    Ok(())
}

/// Build fresh connection fds, install the generation, deposit producer ends
/// at the broker, and spawn the one-shot BUFS_READY waiter.
fn deposit_generation(inner: &Arc<Inner>) -> Result<(), String> {
    let buf_ready = sys::make_eventfd().map_err(|e| format!("eventfd: {e}"))?;
    // Fence packets carry a fixed header plus optional SCM_RIGHTS. Keep the
    // packet boundary so a completion can never consume bytes from the next
    // frame and sequence/generation validation remains exact.
    let (fence_read, fence_write) =
        sys::socketpair(false).map_err(|e| format!("fence socketpair: {e}"))?;
    let (data_ours, data_theirs) =
        sys::socketpair(true).map_err(|e| format!("data socketpair: {e}"))?;
    let (audio_ours, audio_theirs) =
        sys::socketpair(false).map_err(|e| format!("audio socketpair: {e}"))?;
    let (shm_fd, shm_ptr) = sys::make_shm_index().map_err(|e| format!("shm index: {e}"))?;
    let id = {
        let mut next = inner.next_gen.lock().unwrap();
        let id = *next;
        *next += 1;
        id
    };
    // Producer ends for the broker deposit. Shared masters (buf_ready, shm)
    // stay with the consumer; the broker dup()s per served pickup.
    {
        let _guard = inner.io_lock.lock().unwrap();
        let mut gen = inner.gen.lock().unwrap();
        *gen = Some(ActiveGen {
            id,
            buf_ready,
            fence_read,
            data: data_ours,
            _shm_fd: shm_fd,
            shm_ptr: shm_ptr as usize,
            _audio: audio_ours,
        });
    }
    inner.work.lock().unwrap().reset_for_connection(id);
    // Move producer ends into the deposit (buf_ready + shm are dup'd here so
    // the broker owns independent masters for its per-pickup dup()s).
    let (buf_ready_dup, shm_dup) = {
        let gen = inner.gen.lock().unwrap();
        let gen = gen.as_ref().unwrap();
        (dup_owned(&gen.buf_ready), dup_owned(&gen._shm_fd))
    };
    let deposit = Deposit {
        generation: id,
        fds: [
            buf_ready_dup.map_err(|e| format!("dup buf_ready: {e}"))?,
            fence_write,
            data_theirs,
            shm_dup.map_err(|e| format!("dup shm: {e}"))?,
            audio_theirs,
        ],
    };
    // One-shot waiter: on producer attach, push the dma-buf set.
    let waiter_inner = inner.clone();
    let attach_rx = inner.broker.subscribe_attach();
    // Subscribe BEFORE publishing: pickup can happen immediately on reconnect.
    inner.broker.deposit(deposit);
    thread::Builder::new()
        .name(format!("anland-handshake-{id}"))
        .spawn(move || handshake_waiter(waiter_inner, id, attach_rx))
        .map_err(|e| format!("spawn handshake waiter: {e}"))?;
    log::info!("anland.session deposit generation={id}");
    Ok(())
}

/// Close generation masters + unmap. Caller must hold no other locks.
fn teardown_generation(inner: &Arc<Inner>) {
    inner.broker.withdraw();
    *inner.connected_gen.lock().unwrap() = None;
    inner.work.lock().unwrap().reset_for_connection(0);
    let _guard = inner.io_lock.lock().unwrap();
    let mut gen = inner.gen.lock().unwrap();
    if let Some(g) = gen.take() {
        // close() is insufficient while polling threads retain dup()s. Shutdown
        // reaches every duplicate and wakes the producer even on an idle desktop.
        unsafe {
            libc::shutdown(g.data.as_raw_fd(), libc::SHUT_RDWR);
            libc::shutdown(g.fence_read.as_raw_fd(), libc::SHUT_RDWR);
            libc::shutdown(g._audio.as_raw_fd(), libc::SHUT_RDWR);
        }
        sys::munmap_index(g.shm_ptr as *mut u32);
        // OwnedFds drop here (under io_lock, so no select/send races).
    }
}

/// Drop to fallback and immediately re-deposit (reference `enter_fallback`).
fn enter_fallback(inner: &Arc<Inner>, expected: u64, reason: &str) {
    let _transition = inner.transition_lock.lock().unwrap();
    if inner.rebind_active.load(Ordering::Acquire) {
        // A rotation rebind owns the next generation (teardown + deposit):
        // a concurrent fallback must not withdraw it or deposit a spare.
        log::info!("anland.fallback suppressed during surface rebind reason={reason}");
        return;
    }
    if inner.gen.lock().unwrap().as_ref().map(|g| g.id) != Some(expected) {
        return; // An old failed operation cannot tear down its replacement.
    }
    inner.fallback_count.fetch_add(1, Ordering::Relaxed);
    log::warn!("anland.fallback reason={reason}");
    teardown_generation(inner);
    if inner.running.load(Ordering::Acquire) && inner.window_live.load(Ordering::Acquire) {
        if let Err(e) = deposit_generation(inner) {
            log::error!("anland.fallback re-deposit failed: {e}");
        }
    }
}

/// Retire a surface after the producer render fence was lost.
///
/// Once KWin has been selected for a slot, there is no safe `cancelBuffer`
/// fence unless KWin sends its render-done fence.  Withdraw the generation,
/// stop the surface workers, and leave the dequeued slot to the native-window
/// teardown performed by `suspend_surface`.  The next Android lifecycle
/// attachment creates a new BufferQueue surface; no old slot is reused.
fn retire_surface_for_recovery(inner: &Arc<Inner>, expected: u64, reason: &str) {
    let _transition = inner.transition_lock.lock().unwrap();
    if inner
        .gen
        .lock()
        .unwrap()
        .as_ref()
        .map(|generation| generation.id)
        == Some(expected)
    {
        inner.fallback_count.fetch_add(1, Ordering::Relaxed);
        log::error!(
            "anland.surface recovery required reason={reason}; retiring generation={expected}"
        );
        inner
            .surface_recovery_requested
            .store(true, Ordering::Release);
        teardown_generation(inner);
    } else {
        log::warn!(
            "anland.surface recovery requested for stale generation={expected} reason={reason}"
        );
        inner
            .surface_recovery_requested
            .store(true, Ordering::Release);
    }
    let _ = sys::eventfd_write(&inner.wake, 1);
}

fn handshake_waiter(inner: Arc<Inner>, generation: u64, attach_rx: mpsc::Receiver<u64>) {
    // Wait for the broker to serve our generation (producer picked up fds).
    loop {
        if !inner.running.load(Ordering::Acquire) || !inner.window_live.load(Ordering::Acquire) {
            return;
        }
        match attach_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(gen) if gen == generation => break,
            Ok(_) => continue, // stale/other generation
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Still current? Otherwise exit.
                let cur = inner.gen.lock().unwrap().as_ref().map(|g| g.id);
                if cur != Some(generation) {
                    return;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
    // Push BUFS_READY: header + infos, fds as SCM_RIGHTS, then infos bytes.
    let _guard = inner.io_lock.lock().unwrap();
    let gen_guard = inner.gen.lock().unwrap();
    let Some(gen) = gen_guard.as_ref() else {
        return;
    };
    if gen.id != generation {
        return;
    }
    let buffers = inner.buffers.lock().unwrap();
    if buffers.is_empty() {
        return;
    }
    let count = buffers.len().min(MAX_BUFS) as u32;
    let mut hdr = [0u8; 8];
    hdr[0..4].copy_from_slice(&DATA_MSG_BUFS_READY.to_ne_bytes());
    hdr[4..8].copy_from_slice(&(count * 28).to_ne_bytes());
    let fds: Vec<i32> = buffers
        .iter()
        .take(count as usize)
        .map(|s| s.fd.as_raw_fd())
        .collect();
    // SAFETY: wire bytes for packed structs via raw copy.
    let infos: Vec<u8> = {
        let mut v = Vec::with_capacity(count as usize * 28);
        for s in buffers.iter().take(count as usize) {
            let bytes: &[u8; 28] = unsafe { &*(&s.info as *const BufInfo as *const [u8; 28]) };
            v.extend_from_slice(bytes);
        }
        v
    };
    drop(buffers);
    if sys::send_fds(&gen.data, &hdr, &fds).is_err() {
        drop(gen_guard);
        drop(_guard);
        enter_fallback(&inner, generation, "BUFS_READY send failed");
        return;
    }
    if sys::send_all(&gen.data, &infos).is_err() {
        drop(gen_guard);
        drop(_guard);
        enter_fallback(&inner, generation, "BUFS_READY infos send failed");
        return;
    }
    *inner.connected_gen.lock().unwrap() = Some(generation);
    let sgen = *inner.surface_gen.lock().unwrap();
    log::info!("anland.session=connected generation={generation} sgen={sgen} bufs={count}");
    // Seed the producer's render-loop pacing with the live display rate.
    let refresh = inner.refresh_mhz;
    let mut wire = [0u8; 8 + 20];
    wire[0..4].copy_from_slice(&DATA_MSG_INPUT_EVENT.to_ne_bytes());
    wire[4..8].copy_from_slice(&20u32.to_ne_bytes());
    wire[8..12].copy_from_slice(&INPUT_TYPE_DISPLAY_REFRESH.to_ne_bytes());
    wire[12..16].copy_from_slice(&refresh.to_ne_bytes());
    let _ = sys::send_all(&gen.data, &wire);
}

fn render_loop(inner: Arc<Inner>) {
    log::info!("anland.render thread started");
    // Per-generation dups owned by this thread (immune to teardown close).
    let mut cur_gen: u64 = 0;
    let mut cur_fence: Option<OwnedFd> = None;
    let mut idle_logged = false;
    // Thread-local dups of the pacing fds (same immunity rationale).
    let (tick, wake) = {
        let vsync = inner.vsync.lock().unwrap();
        let Some(pump) = vsync.as_ref() else {
            log::error!("anland.render no vsync pump; stopping (no silent free-spin)");
            return;
        };
        match (dup_owned(pump.tick_fd()), dup_owned(&inner.wake)) {
            (Ok(t), Ok(w)) => (t, w),
            _ => {
                log::error!("anland.render pacing dup failed; stopping");
                return;
            }
        }
    };
    // Work-driven instrumentation state. VSYNC contributes timing telemetry
    // when a callback happens to arrive with a real producer request, but it
    // is never a source of render work.
    let mut win_start_ns = sys::now_ns();
    let mut win_frames: u64 = 0;
    let mut win_comp_us: u64 = 0;
    let mut win_comp_max_us: u64 = 0;
    let mut win_skips: u64 = 0;
    let mut win_stalls: u64 = 0;
    // First-tick tracing plus periodic liveness counters.
    let mut iters: u64 = 0;
    let mut selects: u64 = 0;
    let mut alive_ns = sys::now_ns();
    // Per-sink 1 Hz rate limiters (monotonic ns of last log per sink).
    let mut log_stall_ns: u64 = 0;
    let mut log_unknown_ns: u64 = 0;
    let mut log_mismatch_ns: u64 = 0;
    let mut log_gen0_ns: u64 = 0;
    while inner.running.load(Ordering::Acquire)
        && !inner.surface_recovery_requested.load(Ordering::Acquire)
    {
        let Some(window) = current_window(&inner) else {
            break;
        };
        if !inner.window_live.load(Ordering::Acquire) {
            break;
        }
        // Resize only between frames: this thread owns all live queue slots.
        let request = inner.rebind_request.lock().unwrap().take();
        if let Some((w, h, sgen)) = request {
            let result = {
                let _transition = inner.transition_lock.lock().unwrap();
                AnlandSession::rebind_surface_inner(&inner, w, h, sgen)
            };
            let failed = result.is_err();
            {
                let mut next = inner.rebind_request.lock().unwrap();
                if let Err(e) = result {
                    log::warn!("anland.rotate sgen={sgen} retry after rebind failure: {e}");
                    if next.is_none() {
                        *next = Some((w, h, sgen));
                    }
                }
                inner.rebind_active.store(next.is_some(), Ordering::Release);
            }
            if failed {
                // Retry without blocking Android's lifecycle/input thread.
                let _ = sys::poll_readable(&wake, 500);
                drain_fd(&wake);
            }
            continue;
        }
        // Snapshot generation.
        let (gen_id, fence_dup) = {
            let gen = inner.gen.lock().unwrap();
            match gen.as_ref() {
                Some(g) => (g.id, dup_owned(&g.fence_read).ok()),
                None => (0, None),
            }
        };
        if gen_id != cur_gen {
            cur_gen = gen_id;
            cur_fence = fence_dup;
            idle_logged = false;
        }
        // Presentation is producer-work-driven while the Android surface is
        // live. A FRAME_WANTED control wake authorizes one coalesced render;
        // an Android VSYNC tick only contributes optional timing telemetry.
        // Lifecycle/rebind wakes still re-check state without authorizing
        // work when no request is pending.
        if !inner.window_live.load(Ordering::Acquire) {
            break;
        }
        if inner.rebind_active.load(Ordering::Acquire) {
            // A rotation rebind owns the BufferQueue right now: select
            // nothing so no stale-dimension buffer can be presented.
            // SurfaceFlinger holds its last frame until the new
            // generation's first queue lands.
            thread::sleep(Duration::from_millis(5));
            continue;
        }
        let (tick_ready, wake_ready) = match sys::poll_two(&tick, &wake, -1) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("anland.render pacing poll failed: {e}");
                thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        if tick_ready {
            drain_fd(&tick);
        }
        if wake_ready {
            drain_fd(&wake);
        }
        if !tick_ready && !wake_ready {
            // poll_two currently cannot return this combination, but retain
            // the guard so a future pacing backend cannot free-spin.
            continue;
        }
        let now = sys::now_ns();
        iters += 1;
        if iters <= 30 {
            log::info!("anland.work wake={wake_ready} vsync_telemetry={tick_ready}");
        }
        if now.wrapping_sub(alive_ns) >= 10_000_000_000 {
            let d = inner.diagnostics_snapshot();
            log::info!(
                "anland.render alive iters={iters} selects={selects} queued={} fenced={} bare={} fallbacks={} requested={} consumed={} no_damage={} dequeue_fail={} acquire_timeouts={} render_timeouts={} queue_fail={} cancel_fail={} deadline_miss={} rebinds={}",
                d.queued,
                d.fenced,
                d.bare,
                d.fallbacks,
                d.requested,
                d.consumed,
                d.no_damage,
                d.dequeue_failures,
                d.acquire_timeouts,
                d.render_timeouts,
                d.queue_failures,
                d.cancel_failures,
                d.deadline_misses,
                d.rebinds,
            );
            alive_ns = now;
        }
        // Producer gate: only select once THIS generation completed its
        // handshake (kwin picked up fds + received BUFS_READY, recorded in
        // connected_gen). Selecting earlier signals eventfd into the void
        // and then burns the fence stall budget waiting on a producer that
        // is still booting — every cold boot paid exactly one fallback this
        // way (generation 1 always died ~5s after deposit, long before kwin
        // could render; the working generation was always 2). Generation 0
        // (no producer by design) holds the initialized surface frame below.
        if cur_gen != 0 && inner.connected_gen.lock().unwrap().as_ref() != Some(&cur_gen) {
            win_skips += 1;
            log_fps_window(
                &mut win_start_ns,
                &mut win_frames,
                &mut win_comp_us,
                &mut win_comp_max_us,
                &mut win_skips,
                &mut win_stalls,
            );
            continue;
        }
        if cur_gen == 0 {
            // A disconnected generation is a control state, not a render
            // workload. SurfaceFlinger keeps its already-posted black frame;
            // do not dequeue/queue unrendered buffers on every VSYNC while
            // the producer is booting or recovering.
            win_skips += 1;
            if !idle_logged {
                log::info!(
                    "anland.render producer not connected; holding initialized surface frame"
                );
                idle_logged = true;
            } else if now.wrapping_sub(log_gen0_ns) >= 10_000_000_000 {
                log::info!("anland.render disconnected generation still holding last frame");
                log_gen0_ns = now;
            }
            log_fps_window(
                &mut win_start_ns,
                &mut win_frames,
                &mut win_comp_us,
                &mut win_comp_max_us,
                &mut win_skips,
                &mut win_stalls,
            );
            continue;
        }
        if inner.rebind_active.load(Ordering::Acquire) {
            continue;
        }
        // Work is producer-owned. A wake/tick without a pending request is
        // idle: do not dequeue, select, wait for a fence, or queue a duplicate
        // frame. This is the static-desktop power/BufferQueue gate.
        let Some(work) = inner.work.lock().unwrap().take(cur_gen) else {
            win_skips += 1;
            if !idle_logged {
                log::info!("anland.render idle: no FRAME_WANTED; holding last queued frame");
                idle_logged = true;
            } else if now.wrapping_sub(log_gen0_ns) >= 10_000_000_000 {
                log::info!("anland.render idle: no FRAME_WANTED for 10s");
                log_gen0_ns = now;
            }
            log_fps_window(
                &mut win_start_ns,
                &mut win_frames,
                &mut win_comp_us,
                &mut win_comp_max_us,
                &mut win_skips,
                &mut win_stalls,
            );
            continue;
        };
        idle_logged = false;
        inner.work_consumed.fetch_add(1, Ordering::Relaxed);
        // Consume timing data only for a real work item. A telemetry tick
        // arriving while the desktop is static must not be mistaken for a
        // presentation or cause its timing to be reused by later work.
        let tick_timeline = inner
            .vsync
            .lock()
            .ok()
            .and_then(|pump| pump.as_ref().and_then(sys::VsyncPump::take_timeline));
        if let Some(timeline) = tick_timeline {
            inner.timeline_hit.fetch_add(1, Ordering::Relaxed);
            let work_sequence = work.sequence;
            let work_generation = work.generation;
            let late_ns = if timeline.deadline_ns > 0 && now > timeline.deadline_ns {
                now - timeline.deadline_ns
            } else {
                0
            };
            if late_ns > 0 {
                inner.deadline_misses.fetch_add(1, Ordering::Relaxed);
            }
            log::debug!(
                "anland.timeline work sequence={} producer_generation={} frame_time_ns={} deadline_ns={} expected_present_ns={} vsync_id={} late_ns={}",
                work_sequence,
                work_generation,
                timeline.frame_time_ns,
                timeline.deadline_ns,
                timeline.expected_present_ns,
                timeline.vsync_id,
                late_ns,
            );
        } else {
            inner.timeline_miss.fetch_add(1, Ordering::Relaxed);
        }
        if iters <= 30 {
            let sequence = work.sequence;
            let generation = work.generation;
            log::info!(
                "anland.work render=true sequence={sequence} producer_generation={generation}"
            );
        }
        // Selected: this dequeue hands us the slot KWin will render into.
        // (BufferQueue backpressure still applies when all slots are live.)
        let t_dequeue_ns = sys::now_ns();
        let (anb, acquire) = match unsafe { inner.anw.dequeue(window) } {
            Ok(v) => v,
            Err(r) => {
                inner.dequeue_failures.fetch_add(1, Ordering::Relaxed);
                if inner.running.load(Ordering::Acquire) {
                    log::warn!("anland.render dequeue failed: {r}");
                    thread::sleep(Duration::from_millis(100));
                }
                continue;
            }
        };
        record_latency(
            &inner.dequeue_us_total,
            &inner.dequeue_us_max,
            sys::now_ns().wrapping_sub(t_dequeue_ns) / 1000,
        );
        let queue = NativeBufferQueue {
            window,
            api: &inner.anw,
        };
        let mut slot = DequeuedBuffer::new(anb, acquire, &queue);
        if !inner.window_live.load(Ordering::Acquire) {
            let _ = slot.cancel(None);
            break;
        }
        // The surface generation this frame belongs to. A rotation that
        // lands between here and queueBuffer must cancel, never present,
        // a buffer produced for the old dimensions.
        let frame_sgen = *inner.surface_gen.lock().unwrap();
        // Blocking acquire wait (see ACQUIRE_WAIT_MS). Never queue-back:
        // presenting a buffer SF hasn't released invites KWin to render
        // into scanout-active memory (GPU hang, proven). On timeout the
        // lease retains the unsignalled fd and transfers it to cancelBuffer.
        let t_acquire_ns = sys::now_ns();
        if slot.wait_for_acquire(ACQUIRE_WAIT_MS).is_err() {
            inner.acquire_timeouts.fetch_add(1, Ordering::Relaxed);
            record_latency(
                &inner.acquire_us_total,
                &inner.acquire_us_max,
                sys::now_ns().wrapping_sub(t_acquire_ns) / 1000,
            );
            win_stalls += 1;
            let now = sys::now_ns();
            if now.wrapping_sub(log_stall_ns) >= 1_000_000_000 {
                log::warn!("anland.sink acquire-stall (SF release pending >1s); cancelling");
                log_stall_ns = now;
            }
            if slot.cancel(None) != 0 {
                inner.cancel_failures.fetch_add(1, Ordering::Relaxed);
            }
            continue;
        }
        record_latency(
            &inner.acquire_us_total,
            &inner.acquire_us_max,
            sys::now_ns().wrapping_sub(t_acquire_ns) / 1000,
        );
        if !inner.window_live.load(Ordering::Acquire) {
            let _ = slot.cancel(None);
            break;
        }
        // Match slot -> producer index.
        let idx = inner
            .buffers
            .lock()
            .unwrap()
            .iter()
            .position(|s| s.anb == anb);
        let Some(idx) = idx else {
            // Unknown slot (e.g. the held spare surfaced, or SF reallocated
            // buffers): CANCEL back to the free pool, never present — it was
            // not rendered by KWin (see ACQUIRE_WAIT_MS invariant).
            inner.unknown_slot_cancels.fetch_add(1, Ordering::Relaxed);
            let now = sys::now_ns();
            if iters <= 30 || now.wrapping_sub(log_unknown_ns) >= 1_000_000_000 {
                log::info!("anland.sink unknown-slot cancel-back (SF may have reallocated)");
                log_unknown_ns = now;
            }
            if slot.cancel(None) != 0 {
                inner.cancel_failures.fetch_add(1, Ordering::Relaxed);
            }
            continue;
        };
        // select_dmabuf: shm write + eventfd signal under io_lock.
        // t_select spans signal -> render fence: KWin's composite latency.
        let t_select_ns = sys::now_ns();
        {
            let _guard = inner.io_lock.lock().unwrap();
            let gen = inner.gen.lock().unwrap();
            match gen.as_ref() {
                Some(g) if g.id == cur_gen => {
                    unsafe { *(g.shm_ptr as *mut u32) = idx as u32 };
                    if sys::eventfd_write(&g.buf_ready, 1).is_err() {
                        drop(gen);
                        drop(_guard);
                        enter_fallback(&inner, cur_gen, "eventfd signal failed");
                        let _ = slot.cancel(None);
                        continue;
                    }
                }
                _ => {
                    inner.gen_mismatch_cancels.fetch_add(1, Ordering::Relaxed);
                    let now = sys::now_ns();
                    if iters <= 60 || now.wrapping_sub(log_mismatch_ns) >= 1_000_000_000 {
                        log::info!("anland.sink gen-mismatch cancel-back cur={cur_gen}");
                        log_mismatch_ns = now;
                    }
                    if slot.cancel(None) != 0 {
                        inner.cancel_failures.fetch_add(1, Ordering::Relaxed);
                    }
                    continue;
                }
            }
        }
        // The producer has now been notified which slot to render.  From
        // this point a cancellation without the producer's render fence is a
        // separate recovery decision; it is never confused with an acquire
        // fence that was already satisfied above.
        slot.begin_render();
        // refresh_done: 5s poll on our fence dup, then non-blocking recvmsg.
        let t_render_ns = sys::now_ns();
        let rfence = refresh_done(&inner, cur_fence.as_ref(), work);
        if rfence == FENCE_LOST {
            inner.render_timeouts.fetch_add(1, Ordering::Relaxed);
            log::warn!(
                "anland.render fence lost (generation died); retiring surface without cancelBuffer"
            );
            if inner.window_live.load(Ordering::Acquire) {
                retire_surface_for_recovery(&inner, cur_gen, "render fence lost");
            }
            slot.abandon_for_surface_reset();
            break;
        }
        // NO_DAMAGE is an explicit producer completion, not a missing render
        // fence.  It has no render dependency and must reach the cancel path
        // below instead of retiring an otherwise healthy hardware surface.
        if rfence < 0 && rfence != FENCE_NO_DAMAGE && !inner.software_gl {
            log::error!(
                "anland.render producer returned no render fence in hardware mode; retiring surface"
            );
            if inner.window_live.load(Ordering::Acquire) {
                retire_surface_for_recovery(&inner, cur_gen, "hardware frame had no render fence");
            }
            slot.abandon_for_surface_reset();
            break;
        }
        if rfence == FENCE_NO_DAMAGE {
            inner.frames_no_damage.fetch_add(1, Ordering::Relaxed);
            selects += 1;
            let cur_sgen = *inner.surface_gen.lock().unwrap();
            if inner.rebind_active.load(Ordering::Acquire)
                || !surface_geometry::generation_completion_allowed(frame_sgen, cur_sgen)
            {
                inner.stale_gen_cancels.fetch_add(1, Ordering::Relaxed);
                log::info!(
                    "anland.render stale-generation cancel-back no-damage frame_sgen={frame_sgen} sgen={cur_sgen}"
                );
            } else {
                let work_sequence = work.sequence;
                let work_generation = work.generation;
                log::debug!(
                    "anland.render no-damage sequence={} producer_generation={}; cancelBuffer",
                    work_sequence,
                    work_generation
                );
            }
            // A NO_DAMAGE message has no render dependency. The selected slot
            // was acquire-satisfied above, so -1 is the only valid cancel fd.
            let c = slot.cancel(None);
            if c != 0 {
                inner.cancel_failures.fetch_add(1, Ordering::Relaxed);
                log::warn!("anland.render no-damage cancelBuffer failed: {c}");
            }
            log_fps_window(
                &mut win_start_ns,
                &mut win_frames,
                &mut win_comp_us,
                &mut win_comp_max_us,
                &mut win_skips,
                &mut win_stalls,
            );
            continue;
        }
        let rfence_owned = if rfence >= 0 {
            // SAFETY: refresh_done returns ownership of the SCM_RIGHTS fd.
            Some(unsafe { OwnedFd::from_raw_fd(rfence) })
        } else {
            None
        };
        if !inner.window_live.load(Ordering::Acquire) {
            // A surface suspend may land while a bare/software frame was
            // selected. Return the slot, never queue or mark readiness after
            // the lifecycle has retired this surface generation.
            let _ = slot.cancel(rfence_owned);
            break;
        }
        selects += 1;
        // Generation fence: a rotation that landed after this frame's
        // dequeue must cancel, never present, a buffer produced for the old
        // dimensions into the newly sized BufferQueue.
        let cur_sgen = *inner.surface_gen.lock().unwrap();
        if inner.rebind_active.load(Ordering::Acquire)
            || !surface_geometry::generation_completion_allowed(frame_sgen, cur_sgen)
        {
            inner.stale_gen_cancels.fetch_add(1, Ordering::Relaxed);
            log::info!(
                "anland.render stale-generation cancel-back frame_sgen={frame_sgen} sgen={cur_sgen}"
            );
            if slot.cancel(rfence_owned) != 0 {
                inner.cancel_failures.fetch_add(1, Ordering::Relaxed);
            }
            continue;
        }
        slot.complete_render();
        let t_queue_ns = sys::now_ns();
        let q = slot.queue_rendered(rfence_owned);
        record_latency(
            &inner.render_us_total,
            &inner.render_us_max,
            t_queue_ns.wrapping_sub(t_render_ns) / 1000,
        );
        if q != 0 {
            inner.queue_failures.fetch_add(1, Ordering::Relaxed);
            log::warn!("anland.render queueBuffer failed: {q}");
        } else {
            inner.frames_queued.fetch_add(1, Ordering::Relaxed);
            // Readiness contract (mirrors the Smithay path's first-frame
            // marker): the producer connected, rendered into our dma-buf,
            // and we queued it to SurfaceFlinger. Evidence is labeled
            // honestly. Production GPU mode requires a genuine fence (SF
            // waits GPU-side); only the explicit software fallback accepts
            // a bare frame (llvmpipe is CPU-synchronous, so pixels are final
            // when signaled and no fence exists — without this, software
            // sessions could never mark ready).
            let (evidence, ready_log) = if rfence >= 0 {
                inner.frames_fenced.fetch_add(1, Ordering::Relaxed);
                (
                    "anland-queue-fenced",
                    "anland.session=ready first fenced frame queued (plasma-ready marked)",
                )
            } else {
                inner.frames_bare.fetch_add(1, Ordering::Relaxed);
                (
                    "anland-queue-bare-software",
                    "anland.session=ready first software frame queued bare (plasma-ready marked, no GPU fence)",
                )
            };
            // Readiness re-latches per surface generation: only a valid
            // frame from the CURRENT generation marks ready (an old
            // generation's completion can never mark the new surface).
            // Production GPU mode requires a genuine fence; only the
            // explicit software fallback accepts a bare frame.
            let should_mark = {
                let mut ready_gen = inner.ready_surface_gen.lock().unwrap();
                let eligible = rfence >= 0 || (inner.software_gl && ready_gen.is_none());
                if eligible && ready_gen.is_none() {
                    *ready_gen = Some(cur_sgen);
                    true
                } else {
                    false
                }
            };
            if should_mark {
                let bufs = inner.buffers.lock().map(|b| b.len()).unwrap_or(0);
                crate::android::diagnostics::mark_plasma_frame_presented_for_generation_with_evidence(
                    bufs,
                    1,
                    cur_gen,
                    evidence,
                    None,
                );
                log::info!("{ready_log}");
                // Converged: the first valid frame of the new surface
                // generation is presented (buffer+fence zero-copy as ever).
                // Absolute pointer range from here on is the new physical
                // orientation; the anchor was reset at rebind, so there is
                // no first-motion jump.
                let (csw, csh) = *inner.screen.lock().unwrap();
                log::info!(
                    "anland.rotate sgen={cur_sgen} converged first_frame={} screen={csw}x{csh}",
                    if rfence >= 0 { "fenced" } else { "bare" },
                );
                // Latch native readiness for Compose. The live Anland surface
                // keeps running beneath the setup veil until the final swipe.
                crate::android::utils::compose_overlay::notify_desktop_ready_cached();
            }
            let n = inner.frames_queued.load(Ordering::Relaxed);
            if n == 1 || n % 120 == 0 {
                let f = inner.frames_fenced.load(Ordering::Relaxed);
                log::info!(
                    "anland.frame queued={n} fenced={f} bare={} (zero-copy, fence->SurfaceFlinger)",
                    n - f
                );
            }
            // Composite latency is recorded for the fps line only. KWin's
            // Anland backend requested this work explicitly; presentation is
            // not invented by a VSYNC tick.
            let comp_ns = sys::now_ns().wrapping_sub(t_select_ns);
            // Packed struct: copy fields to locals before use (no field borrows).
            let (lat_sequence, lat_generation) = (work.sequence, work.generation);
            log::debug!(
                "anland.lat sequence={} producer_generation={} comp_us={} fenced={}",
                lat_sequence,
                lat_generation,
                comp_ns / 1000,
                rfence >= 0,
            );
            win_frames += 1;
            win_comp_us += comp_ns / 1000;
            win_comp_max_us = win_comp_max_us.max(comp_ns / 1000);
            log_fps_window(
                &mut win_start_ns,
                &mut win_frames,
                &mut win_comp_us,
                &mut win_comp_max_us,
                &mut win_skips,
                &mut win_stalls,
            );
        }
    }
    log::info!("anland.render thread stopped");
}

/// Drain an eventfd counter after poll signalled it. A single read clears
/// accumulated ticks; errors are harmless (next poll re-arms).
fn drain_fd(fd: &OwnedFd) {
    let _ = sys::eventfd_read(fd);
}

/// Emit `anland.fps` once per window: present rate, KWin composite latency
/// (signal->fence), lifecycle/producer skip share, and acquire stalls.
#[allow(clippy::too_many_arguments)]
fn log_fps_window(
    win_start_ns: &mut u64,
    win_frames: &mut u64,
    win_comp_us: &mut u64,
    win_comp_max_us: &mut u64,
    win_skips: &mut u64,
    win_stalls: &mut u64,
) {
    let now = sys::now_ns();
    if now.wrapping_sub(*win_start_ns) < FPS_WINDOW_NS {
        return;
    }
    let elapsed_ns = now.wrapping_sub(*win_start_ns).max(1);
    let frames = *win_frames;
    let skips = *win_skips;
    let hz = frames as f64 * 1_000_000_000.0 / elapsed_ns as f64;
    let avg_us = if frames > 0 { *win_comp_us / frames } else { 0 };
    let skip_pct = skips * 100 / (frames + skips).max(1);
    log::info!(
        "anland.fps hz={hz:.1} comp_avg_us={avg_us} comp_max_us={} skip_pct={skip_pct} acquire_stalls={}",
        *win_comp_max_us,
        *win_stalls,
    );
    *win_start_ns = now;
    *win_frames = 0;
    *win_comp_us = 0;
    *win_comp_max_us = 0;
    *win_skips = 0;
    *win_stalls = 0;
}

const FENCE_LOST: i32 = -2;
const FENCE_NO_DAMAGE: i32 = -3;

/// Wait for the producer's frame completion message. A valid FRAME_DONE
/// returns its render fence fd (>=0) or -1 for a synchronous/software frame;
/// NO_DAMAGE returns FENCE_NO_DAMAGE and is handled by cancelBuffer. Any
/// sequence/generation mismatch is a fatal protocol loss, never an implicit
/// "ready" result.
fn refresh_done(inner: &Arc<Inner>, fence: Option<&OwnedFd>, work: FrameWanted) -> i32 {
    let Some(fence) = fence else { return -1 };
    // Emergency software fallback only: cold sessions (nothing ever queued)
    // get a long interruptible budget, because tearing down the generation
    // mid-render rips the producer connection while llvmpipe is still
    // compiling/rasterizing the first frame, wedging cold boot in a
    // reconnect loop forever. Production GPU path: always the tight 5s
    // detector (a GPU session that cannot present in seconds is broken).
    let cold = inner.software_gl && inner.frames_queued.load(Ordering::Relaxed) == 0;
    let budget_ms: i64 = if cold {
        COLD_FIRST_FRAME_WAIT_MS
    } else {
        FENCE_WAIT_MS as i64
    };
    let mut waited_ms: i64 = 0;
    loop {
        let quantum_ms = 200.min((budget_ms - waited_ms).max(1) as i32);
        match sys::poll_readable(fence, quantum_ms) {
            Ok(true) => break,
            _ => {
                waited_ms += quantum_ms as i64;
                if !inner.running.load(Ordering::Acquire)
                    || !inner.window_live.load(Ordering::Acquire)
                {
                    // Session or surface is stopping: bail without fallback
                    // churn so the render thread can be joined before the
                    // native window is released.
                    return FENCE_LOST;
                }
                if waited_ms >= budget_ms {
                    return FENCE_LOST;
                }
            }
        }
    }
    // The fence channel is a SOCK_SEQPACKET transport. Every packet carries
    // its explicit completion kind plus the exact work sequence/generation;
    // only FRAME_DONE may carry one SCM_RIGHTS render fence.
    let mut wire = [0u8; 24];
    let mut iov = libc::iovec {
        iov_base: wire.as_mut_ptr() as *mut libc::c_void,
        iov_len: wire.len(),
    };
    let mut cmsg_buf = [0u8; 64];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = cmsg_buf.len() as _;
    let n = unsafe { libc::recvmsg(fence.as_raw_fd(), &mut msg, libc::MSG_DONTWAIT) };
    if n < 0 {
        // EAGAIN after POLLIN (fd swapped under us) or error: treat as lost.
        return FENCE_LOST;
    }
    if n != wire.len() as isize
        || (msg.msg_flags & libc::MSG_TRUNC) != 0
        || (msg.msg_flags & libc::MSG_CTRUNC) != 0
    {
        return FENCE_LOST;
    }
    let mut received_fds = [libc::c_int::MIN; 4];
    let mut received_count = 0usize;
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let payload_bytes = (*cmsg).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                let count = (payload_bytes / std::mem::size_of::<libc::c_int>()).min(4);
                let ptr = libc::CMSG_DATA(cmsg) as *const libc::c_int;
                for index in 0..count {
                    if received_count < received_fds.len() {
                        received_fds[received_count] = *ptr.add(index);
                        received_count += 1;
                    } else {
                        libc::close(*ptr.add(index));
                    }
                }
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    let close_received = |fds: &[libc::c_int], count: usize| {
        for fd in fds.iter().take(count) {
            if *fd >= 0 {
                close_silently(*fd);
            }
        }
    };
    let msg_type = u32::from_ne_bytes(wire[0..4].try_into().unwrap());
    let sequence = u64::from_ne_bytes(wire[8..16].try_into().unwrap());
    let generation = u64::from_ne_bytes(wire[16..24].try_into().unwrap());
    let work_sequence = work.sequence;
    let work_generation = work.generation;
    if sequence != work_sequence || generation != work_generation {
        close_received(&received_fds, received_count);
        log::error!(
            "anland.fence sequence mismatch kind={msg_type} got={sequence}/{generation} expected={work_sequence}/{work_generation}"
        );
        return FENCE_LOST;
    }
    match msg_type {
        FENCE_MSG_FRAME_DONE => {
            if received_count > 1 {
                close_received(&received_fds[1..], received_count - 1);
                close_silently(received_fds[0]);
                log::error!("anland.fence FRAME_DONE carried more than one fd");
                return FENCE_LOST;
            }
            if received_count == 1 {
                received_fds[0]
            } else {
                -1
            }
        }
        FENCE_MSG_NO_DAMAGE => {
            close_received(&received_fds, received_count);
            if received_count != 0 {
                log::error!("anland.fence NO_DAMAGE carried an unexpected fd");
                FENCE_LOST
            } else {
                FENCE_NO_DAMAGE
            }
        }
        _ => {
            close_received(&received_fds, received_count);
            log::error!("anland.fence unknown completion kind={msg_type}");
            FENCE_LOST
        }
    }
}

fn event_loop(inner: Arc<Inner>) {
    log::info!("anland.event thread started");
    let mut cur_gen: u64 = 0;
    let mut cur_data: Option<OwnedFd> = None;
    while inner.running.load(Ordering::Acquire)
        && inner.window_live.load(Ordering::Acquire)
        && !inner.surface_recovery_requested.load(Ordering::Acquire)
    {
        let gen_id = inner
            .gen
            .lock()
            .unwrap()
            .as_ref()
            .map(|g| g.id)
            .unwrap_or(0);
        if gen_id != cur_gen {
            cur_gen = gen_id;
            cur_data = None;
            inner.work.lock().unwrap().reset_for_connection(gen_id);
            if gen_id != 0 {
                let dup = inner
                    .gen
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(|g| dup_owned(&g.data).ok());
                cur_data = dup;
            }
        }
        let Some(data) = cur_data.as_ref() else {
            thread::sleep(Duration::from_millis(100));
            continue;
        };
        match sys::poll_readable(data, 500) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => {
                cur_gen = 0;
                cur_data = None;
                continue;
            }
        }
        let mut header = [0u8; 8];
        if sys::recv_all(data, &mut header).is_err() {
            cur_gen = 0;
            cur_data = None;
            continue;
        }
        let msg_type = u32::from_ne_bytes(header[0..4].try_into().unwrap());
        let size = u32::from_ne_bytes(header[4..8].try_into().unwrap()) as usize;
        match msg_type {
            DATA_MSG_FRAME_WANTED => {
                if size != 16 {
                    log::error!("anland.event invalid FRAME_WANTED size={size}");
                    cur_gen = 0;
                    cur_data = None;
                    continue;
                }
                let mut payload = [0u8; 16];
                if sys::recv_all(data, &mut payload).is_err() {
                    cur_gen = 0;
                    cur_data = None;
                    continue;
                }
                let wanted = FrameWanted {
                    sequence: u64::from_ne_bytes(payload[0..8].try_into().unwrap()),
                    generation: u64::from_ne_bytes(payload[8..16].try_into().unwrap()),
                };
                let sequence = wanted.sequence;
                let generation = wanted.generation;
                let accepted = inner.work.lock().unwrap().accept(cur_gen, wanted);
                if accepted {
                    inner.work_requested.fetch_add(1, Ordering::Relaxed);
                    log::debug!(
                        "anland.event FRAME_WANTED sequence={} generation={}",
                        sequence,
                        generation
                    );
                    // Wake the render thread for this producer-owned piece of
                    // work. Android VSYNC is timing telemetry/deadline data;
                    // it must not gate or invent a presentation request.
                    if let Ok(vsync) = inner.vsync.lock() {
                        if let Some(pump) = vsync.as_ref() {
                            let _ = pump.request();
                        }
                    }
                    let _ = sys::eventfd_write(&inner.wake, 1);
                } else {
                    inner.work_rejected.fetch_add(1, Ordering::Relaxed);
                    log::warn!(
                        "anland.event stale/invalid FRAME_WANTED sequence={} generation={}",
                        sequence,
                        generation
                    );
                }
            }
            DATA_MSG_OUTPUT_EVENT => {
                if size != 20 {
                    log::error!("anland.event invalid OUTPUT_EVENT size={size}");
                    if !drain_data_bytes(data, size) {
                        cur_gen = 0;
                        cur_data = None;
                    }
                    continue;
                }
                let mut wire = [0u8; 20];
                if sys::recv_all(data, &mut wire).is_err() {
                    cur_gen = 0;
                    cur_data = None;
                    continue;
                }
                let ev = OutputEvent {
                    ev_type: u32::from_ne_bytes(wire[0..4].try_into().unwrap()),
                    payload: wire[4..20].try_into().unwrap(),
                };
                match ev.ev_type {
                    OUTPUT_TYPE_CLIPBOARD => {
                        // Milestone: drain (mandatory for stream health), bridge later.
                        let size = ev.clipboard_size() as usize;
                        log::info!(
                            "anland.event clipboard from producer: {size} bytes (drained, bridge pending)"
                        );
                        if !drain_data_bytes(data, size) {
                            cur_gen = 0;
                            cur_data = None;
                        }
                    }
                    OUTPUT_TYPE_RESOURCES_REQUEST => {
                        log::info!("anland.event resources request (camera): unanswered, producer treats as disabled");
                    }
                    OUTPUT_TYPE_SET_CONSUMER_VAR => {
                        log::info!(
                            "anland.event set-consumer-var (pointer capture tracking pending)"
                        );
                    }
                    OUTPUT_TYPE_SCHEDULING => {
                        log::info!(
                            "anland.event scheduling hint (cgroup boost needs root; ignored)"
                        );
                    }
                    other => {
                        log::info!("anland.event unknown output type={other}");
                    }
                }
            }
            other => {
                log::warn!("anland.event unexpected data msg type={other} size={size}");
                if !drain_data_bytes(data, size) {
                    cur_gen = 0;
                    cur_data = None;
                }
            }
        }
    }
    log::info!("anland.event thread stopped");
}

/// Drain a variable-length tail without allocating based on an untrusted wire
/// length. Returning false means the producer disconnected mid-message.
fn drain_data_bytes(fd: &OwnedFd, mut remaining: usize) -> bool {
    let mut sink = [0u8; 4096];
    while remaining > 0 {
        let count = remaining.min(sink.len());
        if sys::recv_all(fd, &mut sink[..count]).is_err() {
            return false;
        }
        remaining -= count;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Call {
        Queue(i32),
        Cancel(i32),
    }

    struct FakeQueue {
        calls: Mutex<Vec<Call>>,
        queue_result: i32,
        cancel_result: i32,
    }

    impl FakeQueue {
        fn new(queue_result: i32, cancel_result: i32) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                queue_result,
                cancel_result,
            }
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }

        fn consume_on_call(result: i32, fence: i32) -> i32 {
            // Mirrors ANativeWindow queue/cancel ownership: the callee owns
            // the fence once the call is made, including an error return.
            if fence >= 0 {
                unsafe { libc::close(fence) };
            }
            result
        }
    }

    impl BufferQueueOps for FakeQueue {
        fn queue(&self, _anb: *mut ANativeWindowBuffer, fence: i32) -> i32 {
            self.calls.lock().unwrap().push(Call::Queue(fence));
            Self::consume_on_call(self.queue_result, fence)
        }

        fn cancel(&self, _anb: *mut ANativeWindowBuffer, fence: i32) -> i32 {
            self.calls.lock().unwrap().push(Call::Cancel(fence));
            Self::consume_on_call(self.cancel_result, fence)
        }
    }

    fn new_fence() -> OwnedFd {
        sys::make_eventfd().expect("eventfd fence")
    }

    fn assert_closed(fd: i32) {
        let result = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert_eq!(result, -1, "fd {fd} was not closed");
    }

    fn null_buffer() -> *mut ANativeWindowBuffer {
        std::ptr::null_mut()
    }

    #[test]
    fn pending_acquire_timeout_transfers_original_fence_to_cancel() {
        let fake = FakeQueue::new(0, 0);
        let fence = new_fence();
        let raw = fence.as_raw_fd();
        let mut slot = DequeuedBuffer::new(null_buffer(), raw, &fake);
        // The fd is now owned by the lease, not by this local value.
        std::mem::forget(fence);

        assert!(slot.wait_for_acquire(0).is_err());
        assert_eq!(slot.cancel(None), 0);
        assert_eq!(fake.calls(), vec![Call::Cancel(raw)]);
        assert_closed(raw);
    }

    #[test]
    fn already_signalled_acquire_is_closed_before_cancel_uses_minus_one() {
        let fake = FakeQueue::new(0, 0);
        let fence = new_fence();
        let raw = fence.as_raw_fd();
        sys::eventfd_write(&fence, 1).unwrap();
        let mut slot = DequeuedBuffer::new(null_buffer(), raw, &fake);
        std::mem::forget(fence);

        slot.wait_for_acquire(100).unwrap();
        assert_closed(raw);
        assert_eq!(slot.cancel(None), 0);
        assert_eq!(fake.calls(), vec![Call::Cancel(-1)]);
    }

    #[test]
    fn delayed_acquire_wait_preserves_state_until_signal() {
        let fake = FakeQueue::new(0, 0);
        let fence = new_fence();
        let raw = fence.as_raw_fd();
        let writer = unsafe { libc::dup(raw) };
        assert!(writer >= 0);
        let signaler = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            let fd = unsafe { OwnedFd::from_raw_fd(writer) };
            sys::eventfd_write(&fd, 1).unwrap();
        });
        let mut slot = DequeuedBuffer::new(null_buffer(), raw, &fake);
        std::mem::forget(fence);

        slot.wait_for_acquire(250).unwrap();
        signaler.join().unwrap();
        assert_eq!(slot.cancel(None), 0);
        assert_eq!(fake.calls(), vec![Call::Cancel(-1)]);
        assert_closed(raw);
    }

    #[test]
    fn no_acquire_fence_cancels_with_minus_one() {
        let fake = FakeQueue::new(0, 0);
        let slot = DequeuedBuffer::new(null_buffer(), -1, &fake);
        assert_eq!(slot.cancel(None), 0);
        assert_eq!(fake.calls(), vec![Call::Cancel(-1)]);
    }

    #[test]
    fn epoch_or_stop_cancel_keeps_pending_acquire_dependency() {
        for _reason in ["epoch-change", "stop"] {
            let fake = FakeQueue::new(0, 0);
            let fence = new_fence();
            let raw = fence.as_raw_fd();
            let slot = DequeuedBuffer::new(null_buffer(), raw, &fake);
            std::mem::forget(fence);
            assert_eq!(slot.cancel(None), 0);
            assert_eq!(fake.calls(), vec![Call::Cancel(raw)]);
            assert_closed(raw);
        }
    }

    #[test]
    fn render_failure_and_fence_timeout_have_one_terminal_cancel() {
        let fake = FakeQueue::new(0, 0);
        let mut slot = DequeuedBuffer::new(null_buffer(), -1, &fake);
        slot.begin_render();
        assert_eq!(slot.cancel(None), 0);
        assert_eq!(fake.calls(), vec![Call::Cancel(-1)]);
    }

    #[test]
    fn lost_render_fence_abandons_without_cancel_buffer() {
        let fake = FakeQueue::new(0, 0);
        let mut slot = DequeuedBuffer::new(null_buffer(), -1, &fake);
        slot.begin_render();
        slot.abandon_for_surface_reset();
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn rendered_queue_transfers_render_fence_once() {
        let fake = FakeQueue::new(0, 0);
        let render_fence = new_fence();
        let raw = render_fence.as_raw_fd();
        let mut slot = DequeuedBuffer::new(null_buffer(), -1, &fake);
        slot.begin_render();
        slot.complete_render();
        assert_eq!(slot.queue_rendered(Some(render_fence)), 0);
        assert_eq!(fake.calls(), vec![Call::Queue(raw)]);
        assert_closed(raw);
    }

    #[test]
    fn queue_failure_is_terminal_and_closes_unaccepted_fence() {
        let fake = FakeQueue::new(-libc::EIO, 0);
        let render_fence = new_fence();
        let raw = render_fence.as_raw_fd();
        let mut slot = DequeuedBuffer::new(null_buffer(), -1, &fake);
        slot.begin_render();
        slot.complete_render();
        assert_ne!(slot.queue_rendered(Some(render_fence)), 0);
        assert_eq!(fake.calls(), vec![Call::Queue(raw)]);
        assert_closed(raw);
    }

    #[test]
    fn cancel_failure_is_terminal_and_does_not_leak_fence() {
        let fake = FakeQueue::new(0, -libc::EIO);
        let fence = new_fence();
        let raw = fence.as_raw_fd();
        let slot = DequeuedBuffer::new(null_buffer(), raw, &fake);
        std::mem::forget(fence);
        assert_ne!(slot.cancel(None), 0);
        assert_eq!(fake.calls(), vec![Call::Cancel(raw)]);
        assert_closed(raw);
    }

    #[test]
    fn dropping_an_unresolved_lease_performs_exactly_one_cancel() {
        let fake = FakeQueue::new(0, 0);
        let fence = new_fence();
        let raw = fence.as_raw_fd();
        let slot = DequeuedBuffer::new(null_buffer(), raw, &fake);
        std::mem::forget(fence);
        drop(slot);
        assert_eq!(fake.calls(), vec![Call::Cancel(raw)]);
        assert_closed(raw);
    }

    #[test]
    fn frame_work_is_monotonic_and_coalesced() {
        let mut state = FrameWorkState::default();
        let first = FrameWanted {
            sequence: 1,
            generation: 4,
        };
        let newer = FrameWanted {
            sequence: 2,
            generation: 4,
        };
        assert!(state.accept(9, first));
        assert!(state.accept(9, newer));
        assert_eq!(state.take(9), Some(newer));
        assert_eq!(state.take(9), None);
        assert!(!state.accept(9, first));
    }

    #[test]
    fn frame_work_generation_change_discards_old_pending_request() {
        let mut state = FrameWorkState::default();
        assert!(state.accept(
            3,
            FrameWanted {
                sequence: 8,
                generation: 1,
            }
        ));
        assert!(state.accept(
            3,
            FrameWanted {
                sequence: 1,
                generation: 2,
            }
        ));
        assert_eq!(
            state.take(3),
            Some(FrameWanted {
                sequence: 1,
                generation: 2,
            })
        );
        assert!(!state.accept(
            3,
            FrameWanted {
                sequence: 1,
                generation: 1,
            }
        ));
    }

    #[test]
    fn latency_recorder_accumulates_total_and_max() {
        let total = AtomicU64::new(0);
        let max = AtomicU64::new(0);
        record_latency(&total, &max, 10);
        record_latency(&total, &max, 30);
        record_latency(&total, &max, 20);
        assert_eq!(total.load(Ordering::Relaxed), 60);
        assert_eq!(max.load(Ordering::Relaxed), 30);
    }

    #[test]
    fn diagnostics_snapshot_starts_at_zero() {
        let snapshot = AnlandDiagnostics::default();
        assert_eq!(snapshot.queued, 0);
        assert_eq!(snapshot.requested, 0);
        assert_eq!(snapshot.no_damage, 0);
        assert_eq!(snapshot.deadline_misses, 0);
        assert_eq!(snapshot.dequeue_us_max, 0);
    }
}
