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
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
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

const FENCE_WAIT_MS: i32 = 5000;
/// Cold-start patience for the very first frame: software rendering
/// (llvmpipe) plus cold shader-cache compilation can need minutes before
/// anything is queued, while a session that was already presenting and then
/// stalls is genuinely wedged within seconds. Until any frame has ever been
/// queued, the fence wait below runs in interruptible quanta up to this
/// budget so session stop still joins promptly; afterwards the tight 5s
/// stall detector applies unchanged.
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
const JOIN_TIMEOUT: Duration = Duration::from_secs(8);

// Demand-driven presentation budgets (see render_loop).
/// Full-rate window after any forwarded input event (touch/pointer/key).
const INPUT_BURST_MS: u64 = 1500;
/// Full-rate tail after client-CPU activity (video/animation without touch).
const SELF_SUSTAIN_MS: u64 = 2000;
/// Full-rate window after producer connect and session start (window mapping,
/// startup animation).
const CONNECT_BURST_MS: u64 = 3000;
const START_BURST_MS: u64 = 5000;
/// Idle ceiling: at most one select per second (clock minute-updates land
/// within a second — fine for a clock; cursor blink resumes on input).
const HEARTBEAT_NS: u64 = 1_000_000_000;
/// Instrumentation window for the `anland.fps` line.
const FPS_WINDOW_NS: u64 = 2_000_000_000;

/// One collected window slot handed to the producer.
struct SlotInfo {
    anb: *mut ANativeWindowBuffer,
    fd: OwnedFd,
    info: BufInfo,
}

// SAFETY: slots are only touched by the render thread while the session lives,
// and the spare is cancelled before the window is released.
unsafe impl Send for SlotInfo {}

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

struct Inner {
    window: *mut c_void,
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
    screen_w: u32,
    screen_h: u32,
    refresh_mhz: u32,
    /// Generation that completed its BUFS_READY push. Input events are only
    /// sent for this generation: anything earlier would land in the data
    /// channel ahead of BUFS_READY and desync the producer's handshake.
    connected_gen: Mutex<Option<u64>>,
    // Proof counters.
    frames_queued: AtomicU64,
    frames_fenced: AtomicU64,
    frames_bare: AtomicU64,
    fallback_count: AtomicU64,
    /// Whether the plasma-ready marker was written for this session.
    ready_marked: AtomicBool,
    /// GUI-client CPU sampler state: (total watched jiffies, sample ns,
    /// consecutive busy windows). Updated only by the event thread.
    client_prev: Mutex<(u64, u64, u32)>,
    /// Watched-PID cache: (pids, last full-scan ns). A full /proc scan runs
    /// at most every 5s; between scans only cached PIDs are re-statted (with
    /// cmdline re-verification against PID reuse). Keeps the 500ms sampler
    /// at ~0.3% instead of 3%.
    client_cache: Mutex<(Vec<u32>, u64)>,
    /// Transition latch for busy logging (quiet -> active).
    client_active: AtomicBool,
    /// Kick channel: input forwarding and busy composites write here so the
    /// render loop wakes for an immediate select (input latency), then falls
    /// back to VSYNC-gated pacing. Only the render thread reads.
    wake: OwnedFd,
    /// Monotonic-nanos deadline (CLOCK_MONOTONIC) until which the loop
    /// selects every vsync tick. Past it, only the 1Hz heartbeat selects.
    demand_until_ns: AtomicU64,
    /// Last forwarded pointer position (buffer pixels) for relative-delta
    /// synthesis. The KWin backend emits both absolute and relative motion
    /// from each POINTER_MOTION (`pointerMotion(pos, delta, delta)`), and
    /// relative clients (games, kinetic velocity) need real dx/dy — winit
    /// only carries absolute positions, so the session tracks them.
    last_pointer: Mutex<Option<(f32, f32)>>,
    /// Active touchpad finger-scroll axes as a bitmask (bit 0 = vertical,
    /// bit 1 = horizontal). Scroll-stop events go only to live streams.
    finger_axes: Mutex<u8>,
    /// Display-VSYNC tick source (Choreographer, timer fallback).
    vsync: Mutex<Option<sys::VsyncPump>>,
}

/// Extend the full-rate presentation deadline and wake the render loop for
/// an immediate select. Racy max-update is harmless: a lost race only
/// shortens a burst, and every kick source re-kicks continuously
/// (input stream, per-frame busy composites).
fn kick(inner: &Arc<Inner>, burst_ms: u64) {
    let until = sys::now_ns().wrapping_add(burst_ms.wrapping_mul(1_000_000));
    inner
        .demand_until_ns
        .fetch_max(until, Ordering::AcqRel);
    let _ = sys::eventfd_write(&inner.wake, 1);
}

unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

pub struct AnlandSession {
    inner: Arc<Inner>,
    render_thread: Option<JoinHandle<()>>,
    event_thread: Option<JoinHandle<()>>,
    broker_thread: Option<JoinHandle<()>>,
    _window_holder: Option<Arc<winit::window::Window>>,
}

pub struct AnlandConfig {
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
    pub socket_path: std::path::PathBuf,
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

impl AnlandSession {
    /// Take over `window` for zero-copy GPU presentation.
    ///
    /// `window_holder` keeps the winit Window alive for the session lifetime.
    /// The caller must guarantee `window` is a valid, current `ANativeWindow*`
    /// and must call [`Self::stop`] while it is still valid (Portal's
    /// `suspended()` runs while the lifecycle window is alive).
    pub fn start(
        window: *mut c_void,
        window_holder: Arc<winit::window::Window>,
        cfg: &AnlandConfig,
    ) -> Result<Self, String> {
        let anw = unsafe { AnwApi::load() }?;
        unsafe { anw::acquire(window, &anw) };
        // Connect to the CPU API via the lock/unlock ritual (see anw.rs):
        // the in-object perform() slot is not trusted for indirect calls
        // (its supposed address disagrees with every dlsym'd entry on this
        // device), while the dlsym'd dequeue/queue path is proven working.
        unsafe { anw.connect_cpu_ritual(window)? };
        let (win_w, win_h) = unsafe {
            (
                anw::get_width(window, &anw),
                anw::get_height(window, &anw),
            )
        };
        let (w, h) = if win_w > 0 && win_h > 0 {
            (win_w as u32, win_h as u32)
        } else {
            (cfg.width, cfg.height)
        };
        log::info!(
            "anland.renderer=anland-gpu window={w}x{h} requested={}x{}",
            cfg.width,
            cfg.height
        );
        let r = unsafe {
            anw::set_buffers_geometry(window, &anw, w as i32, h as i32, anw::FORMAT_RGBA_8888)
        };
        if r != 0 {
            unsafe {
                anw::release(window, &anw);
            }
            return Err(format!("ANativeWindow_setBuffersGeometry failed: {r}"));
        }
        let min_undequeued = unsafe { anw.query_min_undequeued(window) }?;
        let total = (min_undequeued + 2).clamp(3, MAX_BUFS as i32) as usize;
        let r = unsafe { anw.set_buffer_count(window, total) };
        if r != 0 {
            unsafe {
                anw::release(window, &anw);
            }
            return Err(format!("ANativeWindow_setBufferCount({total}) failed: {r}"));
        }
        if let Some(parent) = cfg.socket_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create anland socket dir: {e}"))?;
        }
        let screen = ScreenInfo {
            width: w,
            height: h,
            format: PIXEL_FORMAT_RGBA_8888,
            refresh: cfg.refresh_mhz,
        };
        let broker = Arc::new(Broker::new(screen, cfg.socket_path.clone()));
        // Demand pacing: kick channel + display-VSYNC tick source. Pump
        // failure fails the session loudly (never a silent free-spin).
        let wake = sys::make_eventfd().map_err(|e| format!("wake eventfd: {e}"))?;
        let vsync =
            sys::VsyncPump::start(cfg.refresh_mhz).map_err(|e| format!("vsync pump: {e}"))?;
        let inner = Arc::new(Inner {
            window,
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
            screen_w: w,
            screen_h: h,
            refresh_mhz: cfg.refresh_mhz,
            connected_gen: Mutex::new(None),
            frames_queued: AtomicU64::new(0),
            frames_fenced: AtomicU64::new(0),
            frames_bare: AtomicU64::new(0),
            fallback_count: AtomicU64::new(0),
            ready_marked: AtomicBool::new(false),
            client_prev: Mutex::new((0, 0, 0)),
            client_cache: Mutex::new((Vec::new(), 0)),
            client_active: AtomicBool::new(false),
            wake,
            demand_until_ns: AtomicU64::new(0),
            last_pointer: Mutex::new(None),
            finger_axes: Mutex::new(0),
            vsync: Mutex::new(Some(vsync)),
        });
        // Collect window slots (dup dma-buf fds, hold one spare back).
        collect_buffers(&inner, total)?;
        // First generation: fresh fds + deposit at the broker.
        deposit_generation(&inner)?;
        // Threads.
        let broker_thread = {
            let broker = broker.clone();
            let shutdown = inner.broker_stop.clone();
            thread::Builder::new()
                .name("anland-broker".into())
                .spawn(move || {
                    if let Err(e) = broker.serve(shutdown) {
                        log::warn!("anland.broker serve ended: {e}");
                    }
                })
                .map_err(|e| format!("spawn broker thread: {e}"))?
        };
        let render_thread = {
            let inner = inner.clone();
            thread::Builder::new()
                .name("anland-render".into())
                .spawn(move || render_loop(inner))
                .map_err(|e| format!("spawn render thread: {e}"))?
        };
        let event_thread = {
            let inner = inner.clone();
            thread::Builder::new()
                .name("anland-event".into())
                .spawn(move || event_loop(inner))
                .map_err(|e| format!("spawn event thread: {e}"))?
        };
        // Stash shutdown flag inside inner for stop(); broker thread owns its clone.
        log::info!(
            "anland.session=start screen={}x{} fmt=RGBA_8888 refresh_mhz={} bufs={} socket={}",
            w,
            h,
            cfg.refresh_mhz,
            inner.buffers.lock().map(|b| b.len()).unwrap_or(0),
            cfg.socket_path.display()
        );
        log::info!("anland.qpainter_path=disabled (Smithay SHM upload not used in Anland mode)");
        // Session-start burst covers window mapping + startup animation;
        // the loop then lapses to heartbeat unless input/work sustains it.
        kick(&inner, START_BURST_MS);
        if let Ok(vsync) = inner.vsync.lock() {
            if let Some(pump) = vsync.as_ref() {
                log::info!(
                    "anland.pacing demand-driven vsync={} period_ns={}",
                    pump.mode(),
                    pump.period_ns()
                );
            }
        }
        Ok(Self {
            inner,
            render_thread: Some(render_thread),
            event_thread: Some(event_thread),
            broker_thread: Some(broker_thread),
            _window_holder: Some(window_holder),
        })
    }

    /// Forward one fixed-size input event to the producer. No-op unless the
    /// current generation completed its BUFS_READY push.
    pub fn send_input(&self, ev: &InputEvent) {
        let inner = &self.inner;
        if !inner.running.load(Ordering::Acquire) {
            return;
        }
        // Input means the user is active: full-rate presentation window plus
        // an immediate wake (the select bypasses the vsync gate once, then
        // subsequent frames lock to ticks — low latency without free-spin).
        kick(inner, INPUT_BURST_MS);
        let _guard = inner.io_lock.lock().unwrap();
        let gen = inner.gen.lock().unwrap();
        let Some(gen) = gen.as_ref() else { return };
        if inner.connected_gen.lock().unwrap().as_ref() != Some(&gen.id) {
            return;
        }
        let mut wire = [0u8; 8 + 20];
        wire[0..4].copy_from_slice(&DATA_MSG_INPUT_EVENT.to_ne_bytes());
        wire[4..8].copy_from_slice(&20u32.to_ne_bytes());
        wire[8..12].copy_from_slice(&ev.ev_type.to_ne_bytes());
        wire[12..28].copy_from_slice(&ev.payload);
        if sys::send_all(&gen.data, &wire).is_err() {
            drop(gen);
            drop(_guard);
            enter_fallback(inner, "input send failed");
        }
    }

    /// Forward absolute pointer motion with session-synthesized relative
    /// deltas. winit carries only absolute positions; the KWin backend emits
    /// both absolute and relative motion per event, and relative clients
    /// (plus KWin's velocity/kinetic path) need real dx/dy — zeroed deltas
    /// left the touchpad cursor frozen. First motion after session start (or
    /// a jump) reports zero delta, never a spike.
    pub fn send_pointer_motion(&self, x: f32, y: f32) {
        let (dx, dy) = {
            let mut last = self.inner.last_pointer.lock().unwrap();
            let d = match *last {
                Some((lx, ly)) => (x - lx, y - ly),
                None => (0.0, 0.0),
            };
            *last = Some((x, y));
            d
        };
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
        if !inner.running.load(Ordering::Acquire) {
            return false;
        }
        kick(inner, INPUT_BURST_MS);
        let _guard = inner.io_lock.lock().unwrap();
        let gen = inner.gen.lock().unwrap();
        let Some(gen) = gen.as_ref() else { return false };
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
            drop(gen);
            drop(_guard);
            enter_fallback(inner, "text send failed");
            return false;
        }
        log::info!("anland.input text committed ({} bytes)", text.len());
        true
    }

    /// Forward one touchpad finger-scroll value (axis 0 = vertical,
    /// 1 = horizontal) and mark the stream live for stop events.
    pub fn send_finger_axis(&self, axis: u32, value: f32) {
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

    pub fn screen_size(&self) -> (u32, u32) {
        (self.inner.screen_w, self.inner.screen_h)
    }

    /// Stop the session while the window is still valid. Bounded joins;
    /// never blocks the lifecycle thread indefinitely.
    pub fn stop(mut self) {
        let inner = self.inner.clone();
        inner.running.store(false, Ordering::Release);
        inner.window_live.store(false, Ordering::Release);
        // Unblock a render loop parked in its vsync/kick poll promptly.
        let _ = sys::eventfd_write(&inner.wake, 1);
        // Return the held spare to the queue so a thread blocked in
        // dequeueBuffer wakes promptly (cancel presents nothing).
        if let Ok(mut spare) = inner.spare.lock() {
            if let Some(anb) = spare.take() {
                unsafe { inner.anw.cancel(inner.window, anb, -1) };
            }
        }
        if let Some(h) = self.render_thread.take() {
            join_bounded(h, JOIN_TIMEOUT, "render");
        }
        // The render thread is joined: no reader of the vsync tick remains,
        // so the pump can stop (its thread joins bounded by construction).
        if let Ok(mut vsync) = inner.vsync.lock() {
            if let Some(pump) = vsync.take() {
                pump.stop();
            }
        }
        if let Some(h) = self.event_thread.take() {
            join_bounded(h, JOIN_TIMEOUT, "event");
        }
        if let Some(h) = self.broker_thread.take() {
            inner.broker_stop.store(true, Ordering::Release);
            join_bounded(h, JOIN_TIMEOUT, "broker");
        }
        teardown_generation(&inner);
        // Release window buffers back to the queue state and disconnect.
        if let Ok(buffers) = inner.buffers.lock() {
            for slot in buffers.iter() {
                unsafe { inner.anw.cancel(inner.window, slot.anb, -1) };
            }
        }
        unsafe {
            // No API disconnect: the window was connected via the lock ritual
            // and struct perform() is untrusted. The NativeActivity window is
            // destroyed by the framework right after suspend, which drops all
            // API state; a renderer-flag switch always lands on a fresh window.
            anw::release(inner.window, &inner.anw);
        }
        let (q, f, b, fb) = (
            inner.frames_queued.load(Ordering::Relaxed),
            inner.frames_fenced.load(Ordering::Relaxed),
            inner.frames_bare.load(Ordering::Relaxed),
            inner.fallback_count.load(Ordering::Relaxed),
        );
        log::info!("anland.session=stop queued={q} fenced={f} bare={b} fallbacks={fb}");
    }
}

fn join_bounded(handle: JoinHandle<()>, timeout: Duration, what: &str) {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = handle.join();
        let _ = tx.send(());
    });
    if rx.recv_timeout(timeout).is_err() {
        log::warn!("anland.{what} join timed out after {timeout:?}; detaching");
    }
}

/// Dequeue/rotate all window slots, dup their dma-buf fds, and hold one
/// spare dequeued (in our hands, never queued) for stop-time unblocking.
fn collect_buffers(inner: &Arc<Inner>, total: usize) -> Result<(), String> {
    let need_producer = total.saturating_sub(1).max(2);
    let mut found: Vec<SlotInfo> = Vec::new();
    for attempt in 0..total * 4 + 2 {
        if found.len() >= need_producer {
            break;
        }
        let (anb, fence) = unsafe { inner.anw.dequeue(inner.window) }
            .map_err(|r| format!("collect dequeueBuffer failed on attempt {attempt}: {r}"))?;
        close_silently(fence); // enumeration only; no wait needed
        let Some((dma_fd, stride_px, bw, bh)) = (unsafe { AnwApi::buffer_dma_info(anb) }) else {
            unsafe { inner.anw.cancel(inner.window, anb, -1) };
            continue;
        };
        // Dedup by slot pointer (stable per queue slot).
        if found.iter().any(|s: &SlotInfo| s.anb == anb) {
            unsafe { inner.anw.queue(inner.window, anb, -1) };
            continue;
        }
        // Post back so the next dequeue rotates to another slot.
        unsafe { inner.anw.queue(inner.window, anb, -1) };
        let dup = unsafe { libc::dup(dma_fd) };
        if dup < 0 {
            continue;
        }
        // SAFETY: dup is a fresh fd.
        let fd = unsafe { OwnedFd::from_raw_fd(dup) };
        let info = BufInfo {
            stride: (stride_px as u32).wrapping_mul(4),
            width: bw as u32,
            height: bh as u32,
            format: PIXEL_FORMAT_RGBA_8888,
            modifier: 0,
            offset: 0,
        };
        log::info!(
            "anland.collect buf[{}]: {}x{} stride_px={} fd={}",
            found.len(),
            bw,
            bh,
            stride_px,
            fd.as_raw_fd()
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
    let spare = match unsafe { inner.anw.dequeue(inner.window) } {
        Ok((anb, fence)) => {
            close_silently(fence);
            log::info!("anland.collect spare held dequeued (stop-time unblock)");
            Some(anb)
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
    let (fence_read, fence_write) =
        sys::socketpair(true).map_err(|e| format!("fence socketpair: {e}"))?;
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
    inner.broker.deposit(deposit);
    // One-shot waiter: on producer attach, push the dma-buf set.
    let waiter_inner = inner.clone();
    let attach_rx = inner.broker.subscribe_attach();
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
    let _guard = inner.io_lock.lock().unwrap();
    let mut gen = inner.gen.lock().unwrap();
    if let Some(g) = gen.take() {
        sys::munmap_index(g.shm_ptr as *mut u32);
        // OwnedFds drop here (under io_lock, so no select/send races).
    }
}

/// Drop to fallback and immediately re-deposit (reference `enter_fallback`).
fn enter_fallback(inner: &Arc<Inner>, reason: &str) {
    inner.fallback_count.fetch_add(1, Ordering::Relaxed);
    log::warn!("anland.fallback reason={reason}");
    teardown_generation(inner);
    if inner.running.load(Ordering::Acquire) && inner.window_live.load(Ordering::Acquire) {
        if let Err(e) = deposit_generation(inner) {
            log::error!("anland.fallback re-deposit failed: {e}");
        }
    }
}

fn handshake_waiter(inner: Arc<Inner>, generation: u64, attach_rx: mpsc::Receiver<u64>) {
    // Wait for the broker to serve our generation (producer picked up fds).
    loop {
        if !inner.running.load(Ordering::Acquire) {
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
    let gen = inner.gen.lock().unwrap();
    let Some(gen) = gen.as_ref() else { return };
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
        drop(gen);
        drop(_guard);
        enter_fallback(&inner, "BUFS_READY send failed");
        return;
    }
    if sys::send_all(&gen.data, &infos).is_err() {
        drop(gen);
        drop(_guard);
        enter_fallback(&inner, "BUFS_READY infos send failed");
        return;
    }
    *inner.connected_gen.lock().unwrap() = Some(generation);
    // Connect burst: the producer's first frames (window mapping) present at
    // full rate even before any input arrives.
    kick(&inner, CONNECT_BURST_MS);
    // Seed the producer's render-loop pacing with the live display rate.
    let refresh = inner.refresh_mhz;
    let mut wire = [0u8; 8 + 20];
    wire[0..4].copy_from_slice(&DATA_MSG_INPUT_EVENT.to_ne_bytes());
    wire[4..8].copy_from_slice(&20u32.to_ne_bytes());
    wire[8..12].copy_from_slice(&INPUT_TYPE_DISPLAY_REFRESH.to_ne_bytes());
    wire[12..16].copy_from_slice(&refresh.to_ne_bytes());
    let _ = sys::send_all(&gen.data, &wire);
    log::info!("anland.session=connected generation={generation} bufs={count}");
}

fn render_loop(inner: Arc<Inner>) {
    log::info!("anland.render thread started");
    // Per-generation dups owned by this thread (immune to teardown close).
    let mut cur_gen: u64 = 0;
    let mut cur_fence: Option<OwnedFd> = None;
    let mut pending: Option<u64> = None; // generation a select was issued on
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
    // Demand + instrumentation state.
    let mut last_select_ns: u64 = 0;
    let mut win_start_ns = sys::now_ns();
    let mut win_frames: u64 = 0;
    let mut win_comp_us: u64 = 0;
    let mut win_comp_max_us: u64 = 0;
    let mut win_skips: u64 = 0;
    let mut win_stalls: u64 = 0;
    // Temporary gate tracing (demand-tuning build): first decisions + alive.
    let mut iters: u64 = 0;
    let mut selects: u64 = 0;
    let mut alive_ns = sys::now_ns();
    // Per-sink 1 Hz rate limiters (monotonic ns of last log per sink).
    let mut log_stall_ns: u64 = 0;
    let mut log_unknown_ns: u64 = 0;
    let mut log_mismatch_ns: u64 = 0;
    let mut log_gen0_ns: u64 = 0;
    while inner.running.load(Ordering::Acquire) {
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
            pending = None;
            idle_logged = false;
        }
        // Demand gate: select on every display-vsync tick while the user is
        // active (input bursts, connect/start bursts, self-sustaining busy
        // composites), immediately on kick (input latency), and otherwise at
        // most at the 2Hz heartbeat. Skipped ticks do no dequeue/select at
        // all: KWin stays idle, no GPU work is submitted, no frame is queued.
        if !inner.window_live.load(Ordering::Acquire) {
            break;
        }
        let now = sys::now_ns();
        let demanding = inner.demand_until_ns.load(Ordering::Acquire) > now;
        let timeout_ms = if demanding { 50 } else { 500 };
        let (tick_ready, wake_ready) = match sys::poll_two(&tick, &wake, timeout_ms) {
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
        let now = sys::now_ns();
        let demanding = inner.demand_until_ns.load(Ordering::Acquire) > now;
        let since_last = now.wrapping_sub(last_select_ns);
        let want = wake_ready
            || (tick_ready && (demanding || since_last >= HEARTBEAT_NS))
            || (!tick_ready && !wake_ready && !demanding && since_last >= HEARTBEAT_NS);
        iters += 1;
        if iters <= 30 {
            log::info!(
                "anland.gate iter={iters} tick={tick_ready} wake={wake_ready} demanding={demanding} since_last_us={} want={want}",
                since_last / 1000
            );
        }
        if now.wrapping_sub(alive_ns) >= 10_000_000_000 {
            let (q, f, b, fb) = (
                inner.frames_queued.load(Ordering::Relaxed),
                inner.frames_fenced.load(Ordering::Relaxed),
                inner.frames_bare.load(Ordering::Relaxed),
                inner.fallback_count.load(Ordering::Relaxed),
            );
            log::info!(
                "anland.render alive iters={iters} selects={selects} queued={q} fenced={f} bare={b} fallbacks={fb}"
            );
            alive_ns = now;
        }
        if !want {
            win_skips += 1;
            log_fps_window(
                &inner,
                &mut win_start_ns,
                &mut win_frames,
                &mut win_comp_us,
                &mut win_comp_max_us,
                &mut win_skips,
                &mut win_stalls,
            );
            continue;
        }
        // Selected: this dequeue hands us the slot KWin will render into.
        // (BufferQueue backpressure still applies when all slots are live.)
        let (anb, acquire) = match unsafe { inner.anw.dequeue(inner.window) } {
            Ok(v) => v,
            Err(r) => {
                if inner.running.load(Ordering::Acquire) {
                    log::warn!("anland.render dequeue failed: {r}");
                    thread::sleep(Duration::from_millis(100));
                }
                continue;
            }
        };
        if acquire >= 0 {
            // Blocking acquire wait (see ACQUIRE_WAIT_MS). Never queue-back:
            // presenting a buffer SF hasn't released invites KWin to render
            // into scanout-active memory (GPU hang, proven). Cancel instead.
            let fence = unsafe { OwnedFd::from_raw_fd(acquire) };
            if sys::wait_fence(fence, ACQUIRE_WAIT_MS).is_err() {
                win_stalls += 1;
                let now = sys::now_ns();
                if now.wrapping_sub(log_stall_ns) >= 1_000_000_000 {
                    log::warn!("anland.sink acquire-stall (SF release pending >1s); cancelling");
                    log_stall_ns = now;
                }
                unsafe { inner.anw.cancel(inner.window, anb, -1) };
                continue;
            }
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
            let now = sys::now_ns();
            if iters <= 30 || now.wrapping_sub(log_unknown_ns) >= 1_000_000_000 {
                log::info!("anland.sink unknown-slot cancel-back (SF may have reallocated)");
                log_unknown_ns = now;
            }
            unsafe { inner.anw.cancel(inner.window, anb, -1) };
            continue;
        };
        if cur_gen == 0 {
            // Fallback: keep the window alive with unrendered frames.
            unsafe { inner.anw.queue(inner.window, anb, -1) };
            if !idle_logged {
                log::info!(
                    "anland.render fallback: presenting unrendered frames until producer connects"
                );
                idle_logged = true;
            } else {
                let now = sys::now_ns();
                if now.wrapping_sub(log_gen0_ns) >= 10_000_000_000 {
                    log::info!("anland.sink gen0-fallback still unconnected");
                    log_gen0_ns = now;
                }
            }
            thread::sleep(Duration::from_millis(16));
            continue;
        }
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
                        enter_fallback(&inner, "eventfd signal failed");
                        unsafe { inner.anw.cancel(inner.window, anb, -1) };
                        continue;
                    }
                    pending = Some(cur_gen);
                }
                _ => {
                    let now = sys::now_ns();
                    if iters <= 60 || now.wrapping_sub(log_mismatch_ns) >= 1_000_000_000 {
                        log::info!(
                            "anland.sink gen-mismatch cancel-back cur={cur_gen}"
                        );
                        log_mismatch_ns = now;
                    }
                    unsafe { inner.anw.cancel(inner.window, anb, -1) };
                    continue;
                }
            }
        }
        // refresh_done: 5s poll on our fence dup, then non-blocking recvmsg.
        let rfence = refresh_done(&inner, cur_fence.as_ref(), pending == Some(cur_gen));
        if pending == Some(cur_gen) && rfence == FENCE_LOST {
            log::warn!("anland.render fence lost (generation died); buffer cancelled, not presented");
            unsafe { inner.anw.cancel(inner.window, anb, -1) };
            continue;
        }
        pending = None;
        selects += 1;
        let q = unsafe { inner.anw.queue(inner.window, anb, rfence) };
        if q != 0 {
            log::warn!("anland.render queueBuffer failed: {q}");
            close_silently(rfence);
        } else {
            inner.frames_queued.fetch_add(1, Ordering::Relaxed);
            // Readiness contract (mirrors the Smithay path's first-frame
            // marker): the producer connected, rendered into our dma-buf,
            // and we queued it to SurfaceFlinger. Evidence is labeled
            // honestly: queue with fence (SF waits GPU-side), or queue bare
            // (software rendering is CPU-synchronous, so pixels are final
            // when signaled and no fence exists). A bare first frame still
            // proves a live desktop; without it, software sessions can never
            // mark ready and always hit the session watchdog.
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
            if !inner.ready_marked.swap(true, Ordering::AcqRel) {
                let bufs = inner.buffers.lock().map(|b| b.len()).unwrap_or(0);
                crate::android::diagnostics::mark_plasma_frame_presented_for_generation_with_evidence(
                    bufs,
                    1,
                    cur_gen,
                    evidence,
                    None,
                );
                log::info!("{ready_log}");
            }
            let n = inner.frames_queued.load(Ordering::Relaxed);
            if n == 1 || n % 120 == 0 {
                let f = inner.frames_fenced.load(Ordering::Relaxed);
                log::info!(
                    "anland.frame queued={n} fenced={f} bare={} (zero-copy, fence->SurfaceFlinger)",
                    n - f
                );
            }
            // Composite latency is recorded for the fps line, but it must NOT
            // drive demand: KWin's Anland backend re-renders the full scene
            // on every select (proven: 7.5ms even with plasmashell frozen),
            // so a latency threshold can never distinguish idle from busy.
            // Video/animation protection comes from the client-CPU sampler.
            let comp_ns = sys::now_ns().wrapping_sub(t_select_ns);
            win_frames += 1;
            win_comp_us += comp_ns / 1000;
            win_comp_max_us = win_comp_max_us.max(comp_ns / 1000);
            last_select_ns = sys::now_ns();
            log_fps_window(
                &inner,
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
/// (signal->fence), skip share (demand gating working set), acquire stalls.
#[allow(clippy::too_many_arguments)]
fn log_fps_window(
    inner: &Arc<Inner>,
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
    let avg_us = if frames > 0 {
        *win_comp_us / frames
    } else {
        0
    };
    let skip_pct = skips * 100 / (frames + skips).max(1);
    let demanding = inner.demand_until_ns.load(Ordering::Acquire) > now;
    log::info!(
        "anland.fps hz={hz:.1} comp_avg_us={avg_us} comp_max_us={} skip_pct={skip_pct} acquire_stalls={} demanding={demanding}",
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

/// Wait for the producer's render-done message; return its fence fd (>=0),
/// -1 for ready-now, or FENCE_LOST when the generation died.
fn refresh_done(inner: &Arc<Inner>, fence: Option<&OwnedFd>, selected: bool) -> i32 {
    if !selected {
        return -1;
    }
    let Some(fence) = fence else { return -1 };
    // Cold sessions (nothing ever queued) get a long interruptible budget;
    // flowing sessions keep the tight stall detector. Tearing down the
    // generation mid-render (as the old unconditional 5s timeout did) rips
    // the producer connection while llvmpipe is still compiling/rasterizing
    // the first frame, wedging cold boot in a reconnect loop forever.
    let cold = inner.frames_queued.load(Ordering::Relaxed) == 0;
    let budget_ms: i64 = if cold {
        COLD_FIRST_FRAME_WAIT_MS
    } else {
        FENCE_WAIT_MS as i64
    };
    let mut waited_ms: i64 = 0;
    loop {
        let quantum_ms = FENCE_WAIT_MS.min((budget_ms - waited_ms).max(1) as i32);
        match sys::poll_readable(fence, quantum_ms) {
            Ok(true) => break,
            _ => {
                waited_ms += quantum_ms as i64;
                if !inner.running.load(Ordering::Acquire) {
                    // Session is stopping: bail without fallback churn so
                    // the render thread still joins promptly.
                    return FENCE_LOST;
                }
                if waited_ms >= budget_ms {
                    enter_fallback(inner, "refresh_done timeout (producer stalled)");
                    return FENCE_LOST;
                }
            }
        }
    }
    // Non-blocking recvmsg: 1 byte + optional SCM_RIGHTS fence.
    let mut byte = [0u8; 1];
    let mut iov = libc::iovec {
        iov_base: byte.as_mut_ptr() as *mut libc::c_void,
        iov_len: 1,
    };
    let mut cmsg_buf = [0u8; 32];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = cmsg_buf.len() as _;
    let n = unsafe { libc::recvmsg(fence.as_raw_fd(), &mut msg, libc::MSG_DONTWAIT) };
    if n == 0 {
        enter_fallback(inner, "fence channel EOF (producer gone)");
        return FENCE_LOST;
    }
    if n < 0 {
        // EAGAIN after POLLIN (fd swapped under us) or error: treat as lost.
        enter_fallback(inner, "fence channel recv failed");
        return FENCE_LOST;
    }
    let mut rfence = -1;
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                rfence = *(libc::CMSG_DATA(cmsg) as *const i32);
                break;
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    rfence
}

fn event_loop(inner: Arc<Inner>) {
    log::info!("anland.event thread started");
    let mut cur_gen: u64 = 0;
    let mut cur_data: Option<OwnedFd> = None;
    while inner.running.load(Ordering::Acquire) {
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
            Ok(false) => {
                // No producer output event: sample GUI-client CPU for demand.
                sample_client_activity(&inner);
                continue;
            }
            Err(_) => {
                cur_gen = 0;
                cur_data = None;
                continue;
            }
        }
        let mut wire = [0u8; 8 + 20];
        if sys::recv_all(data, &mut wire).is_err() {
            cur_gen = 0;
            cur_data = None;
            continue;
        }
        let msg_type = u32::from_ne_bytes(wire[0..4].try_into().unwrap());
        if msg_type != DATA_MSG_OUTPUT_EVENT {
            log::warn!("anland.event unexpected data msg type={msg_type}");
            continue;
        }
        let ev = OutputEvent {
            ev_type: u32::from_ne_bytes(wire[8..12].try_into().unwrap()),
            payload: wire[12..28].try_into().unwrap(),
        };
        match ev.ev_type {
            OUTPUT_TYPE_CLIPBOARD => {
                // Milestone: drain (mandatory for stream health), bridge later.
                let size = ev.clipboard_size() as usize;
                log::info!(
                    "anland.event clipboard from producer: {size} bytes (drained, bridge pending)"
                );
                let mut sink = vec![0u8; size.min(1 << 20)];
                let mut left = size;
                while left > 0 {
                    let n = left.min(sink.len());
                    if sys::recv_all(data, &mut sink[..n]).is_err() {
                        break;
                    }
                    left -= n;
                }
            }
            OUTPUT_TYPE_RESOURCES_REQUEST => {
                log::info!("anland.event resources request (camera): unanswered, producer treats as disabled");
            }
            OUTPUT_TYPE_SET_CONSUMER_VAR => {
                log::info!("anland.event set-consumer-var (pointer capture tracking pending)");
            }
            OUTPUT_TYPE_SCHEDULING => {
                log::info!("anland.event scheduling hint (cgroup boost needs root; ignored)");
            }
            other => {
                log::info!("anland.event unknown output type={other}");
            }
        }
    }
    log::info!("anland.event thread stopped");
}

/// GUI-client CPU activity sampler (demand input for video/animation).
///
/// Runs on the event thread's 500ms poll cadence. Sums user+sys jiffies over
/// watched guest client processes; any window above threshold kicks
/// full-rate presentation. Watched: Wayland/X11 clients (video raster and
/// software decode burn client CPU here — our Firefox is SWGL, so playback
/// always shows up). Deliberately NOT watched: KWin (our selects drive its
/// CPU — feedback loop) and our own process. Fail-quiet (no kick on error),
/// never fail-busy.

/// utime+stime jiffies for one PID (0 on any read/parse failure).
fn proc_jiffies(pid: u32) -> u64 {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return 0;
    };
    // comm is parenthesized and may contain spaces: split after last ')'.
    let Some(end) = stat.rfind(')') else {
        return 0;
    };
    let fields: Vec<&str> = stat[end + 1..].split_whitespace().collect();
    // fields[0] = state (overall field 3): utime=14 -> [11], stime=15 -> [12].
    if fields.len() < 13 {
        return 0;
    }
    let u: u64 = fields[11].parse().unwrap_or(0);
    let s: u64 = fields[12].parse().unwrap_or(0);
    u.wrapping_add(s)
}

fn sample_client_activity(inner: &Arc<Inner>) {
    // Substrings matched against /proc/PID/cmdline (NULs are fine: the
    // binary name itself is matched, separators don't matter).
    // NOTE: plasmashell is deliberately NOT watched. It burns ~2.6% CPU
    // continuously at idle (tray/clock polling) with no visible motion, and
    // all of its input-driven animations (hover, window management) coincide
    // with input kicks. Watching it flaps demand.
    // Real video lives in watched apps (SWGL raster always shows up).
    const WATCH: &[&str] = &[
        // Path prefix covers every Firefox process kind (main, content,
        // gpu, rdd, forkserver, utility): all exec from /usr/lib/firefox*
        // (also matches firefox-esr). Binary-name matching would miss kinds
        // whose cmdline differs; nothing else lives under that path.
        "/usr/lib/firefox",
        "chromium",
        "chrome",
        "electron",
        "Xwayland",
        "dolphin",
        "konsole",
        "systemsettings",
        "discover",
        "vlc",
        "mpv",
        "kded6",
        "krunner",
    ];
    // ~2% of one core within the 500ms window (USER_HZ=100). Idle plasmashell
    // sits ~1.4% (below); any video raster/decode is far above.
    const BUSY_JIFFIES_PER_WINDOW: u64 = 1;
    // Full /proc re-scan cadence for PID discovery (cached PIDs are cheap).
    const FULL_SCAN_NS: u64 = 5_000_000_000;
    let self_pid = std::process::id();
    let now = sys::now_ns();
    // Fast path: re-stat cached PIDs (with cmdline re-verification).
    let mut total: u64 = 0;
    let mut cached: Vec<u32> = Vec::new();
    let mut scanned: usize = 0;
    let full_scan = {
        let cache = inner.client_cache.lock().unwrap();
        now.wrapping_sub(cache.1) >= FULL_SCAN_NS
    };
    if !full_scan {
        let cache = inner.client_cache.lock().unwrap();
        for &pid in cache.0.iter() {
            if pid == self_pid {
                continue;
            }
            let Ok(cmd) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
                continue;
            };
            if cmd.is_empty() || cmd[0] != b'/' {
                continue;
            }
            let mut watched = false;
            for w in WATCH {
                if cmd.windows(w.len()).any(|s| s == w.as_bytes()) {
                    watched = true;
                    break;
                }
            }
            if !watched {
                continue; // PID reused by another binary: drop from cache
            }
            cached.push(pid);
            total = total.wrapping_add(proc_jiffies(pid));
        }
    } else {
        // Slow path: full scan, repopulate the cache.
        let Ok(dir) = std::fs::read_dir("/proc") else {
            log::warn!("anland.demand sampler: cannot read /proc (fail-quiet)");
            return;
        };
        for entry in dir.flatten() {
            scanned += 1;
            let name = entry.file_name();
            let Some(pid_str) = name.to_str() else {
                continue;
            };
            let Ok(pid) = pid_str.parse::<u32>() else {
                continue;
            };
            if pid == self_pid {
                continue;
            }
            let Ok(cmd) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
                continue;
            };
            if cmd.is_empty() || cmd[0] != b'/' {
                continue;
            }
            let mut watched = false;
            for w in WATCH {
                if cmd.windows(w.len()).any(|s| s == w.as_bytes()) {
                    watched = true;
                    break;
                }
            }
            if !watched {
                continue;
            }
            cached.push(pid);
            total = total.wrapping_add(proc_jiffies(pid));
        }
        *inner.client_cache.lock().unwrap() = (cached, now);
        // Temporary sampler visibility (demand-tuning build).
        let peek = inner.client_prev.lock().unwrap();
        let peek_delta = total.wrapping_sub(peek.0);
        log::info!("anland.demand scan procs={scanned} watched={} total={total} delta500ms={peek_delta}", inner.client_cache.lock().unwrap().0.len());
    }
    let mut prev = inner.client_prev.lock().unwrap();
    let (prev_total, prev_ns, streak) = *prev;
    if prev_ns == 0 {
        *prev = (total, now, 0);
        return; // first sample: baseline only
    }
    let delta = total.wrapping_sub(prev_total);
    if delta >= BUSY_JIFFIES_PER_WINDOW {
        let streak = streak.saturating_add(1);
        *prev = (total, now, streak);
        drop(prev);
        // Single-window blips (clock tick, tray poll) must not hold
        // full-rate: require sustained activity. Video/animation keeps
        // every window busy, so the 1s ramp is the only cost.
        if streak >= 2 {
            if !inner.client_active.swap(true, Ordering::AcqRel) {
                log::info!("anland.demand client-activity kick (jiffies_500ms={delta})");
            }
            kick(inner, SELF_SUSTAIN_MS);
        }
    } else {
        *prev = (total, now, 0);
        drop(prev);
        if inner.client_active.swap(false, Ordering::AcqRel) {
            log::info!("anland.demand clients quiet; lapsing to heartbeat");
        }
    }
}
