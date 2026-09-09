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
const ACQUIRE_WAIT_MS: i32 = 1000;
const JOIN_TIMEOUT: Duration = Duration::from_secs(8);

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
        // Idempotent (re)connect to the CPU API, mirroring the reference.
        unsafe {
            AnwApi::api_disconnect(window, 2);
            if AnwApi::api_connect(window, 2) != 0 {
                anw::release(window, &anw);
                return Err(
                    "ANativeWindow api_connect(CPU) failed: window is owned by another API (EGL?)"
                        .into(),
                );
            }
        }
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
                AnwApi::api_disconnect(window, 2);
                anw::release(window, &anw);
            }
            return Err(format!("ANativeWindow_setBuffersGeometry failed: {r}"));
        }
        let min_undequeued = unsafe { anw.query_min_undequeued(window) }?;
        let total = (min_undequeued + 2).clamp(3, MAX_BUFS as i32) as usize;
        let r = unsafe { anw.set_buffer_count(window, total) };
        if r != 0 {
            unsafe {
                AnwApi::api_disconnect(window, 2);
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
            AnwApi::api_disconnect(inner.window, 2);
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
        // Blocking dequeue (BufferQueue backpressure = frame pacing).
        if !inner.window_live.load(Ordering::Acquire) {
            break;
        }
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
            // CPU-wait the acquire fence (SurfaceFlinger done with the slot).
            let fence = unsafe { OwnedFd::from_raw_fd(acquire) };
            let _ = sys::wait_fence(fence, ACQUIRE_WAIT_MS);
        }
        // Match slot -> producer index.
        let idx = inner
            .buffers
            .lock()
            .unwrap()
            .iter()
            .position(|s| s.anb == anb);
        let Some(idx) = idx else {
            // Unknown slot (e.g. the held spare surfaced): hand straight back.
            unsafe { inner.anw.queue(inner.window, anb, -1) };
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
            }
            thread::sleep(Duration::from_millis(16));
            continue;
        }
        // select_dmabuf: shm write + eventfd signal under io_lock.
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
                        unsafe { inner.anw.queue(inner.window, anb, -1) };
                        continue;
                    }
                    pending = Some(cur_gen);
                }
                _ => {
                    unsafe { inner.anw.queue(inner.window, anb, -1) };
                    continue;
                }
            }
        }
        // refresh_done: 5s poll on our fence dup, then non-blocking recvmsg.
        let rfence = refresh_done(&inner, cur_fence.as_ref(), pending == Some(cur_gen));
        if pending == Some(cur_gen) && rfence == FENCE_LOST {
            unsafe { inner.anw.queue(inner.window, anb, -1) };
            continue;
        }
        pending = None;
        let q = unsafe { inner.anw.queue(inner.window, anb, rfence) };
        if q != 0 {
            log::warn!("anland.render queueBuffer failed: {q}");
            close_silently(rfence);
        } else {
            inner.frames_queued.fetch_add(1, Ordering::Relaxed);
            if rfence >= 0 {
                inner.frames_fenced.fetch_add(1, Ordering::Relaxed);
            } else {
                inner.frames_bare.fetch_add(1, Ordering::Relaxed);
            }
            let n = inner.frames_queued.load(Ordering::Relaxed);
            if n == 1 || n % 120 == 0 {
                let f = inner.frames_fenced.load(Ordering::Relaxed);
                log::info!(
                    "anland.frame queued={n} fenced={f} bare={} (zero-copy, fence->SurfaceFlinger)",
                    n - f
                );
            }
        }
    }
    log::info!("anland.render thread stopped");
}

const FENCE_LOST: i32 = -2;

/// Wait for the producer's render-done message; return its fence fd (>=0),
/// -1 for ready-now, or FENCE_LOST when the generation died.
fn refresh_done(inner: &Arc<Inner>, fence: Option<&OwnedFd>, selected: bool) -> i32 {
    if !selected {
        return -1;
    }
    let Some(fence) = fence else { return -1 };
    match sys::poll_readable(fence, FENCE_WAIT_MS) {
        Ok(true) => {}
        _ => {
            enter_fallback(inner, "refresh_done timeout (producer stalled)");
            return FENCE_LOST;
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
            Ok(false) => continue,
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
