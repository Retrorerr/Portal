//! Raw fd plumbing for the Anland consumer/broker (Android-only).
//!
//! Mirrors `third_party/anland/common/socket_utils.c` plus the
//! eventfd/memfd/shm setup from `display_consumer.c`. All fds are owned via
//! [`OwnedFd`]-style wrappers so fallback teardown cannot leak or double-close.

use std::io;
use std::mem;
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};

// ---------------------------------------------------------------------------
// byte-stream framing
// ---------------------------------------------------------------------------

pub fn send_all(fd: &OwnedFd, mut buf: &[u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let n = unsafe {
            libc::send(
                fd.as_raw_fd(),
                buf.as_ptr() as *const libc::c_void,
                buf.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        buf = &buf[n as usize..];
    }
    Ok(())
}

pub fn recv_all(fd: &OwnedFd, buf: &mut [u8]) -> io::Result<()> {
    let mut off = 0usize;
    while off < buf.len() {
        let n = unsafe {
            libc::recv(
                fd.as_raw_fd(),
                buf[off..].as_mut_ptr() as *mut libc::c_void,
                buf.len() - off,
                0,
            )
        };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "peer closed"));
        }
        off += n as usize;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// fd passing (SCM_RIGHTS)
// ---------------------------------------------------------------------------

fn cmsg_space(fd_count: usize) -> usize {
    // CMSG_SPACE(sizeof(int) * n); compute portably via libc macros is
    // unavailable, so use the Linux/x86_64+aarch64 layout: header (16 bytes
    // aligned) + payload aligned to 8. Matches CMSG_SPACE exactly on Android.
    const ALIGN: usize = 8;
    let hdr: usize = 16;
    hdr + (mem::size_of::<libc::c_int>() * fd_count).div_ceil(ALIGN) * ALIGN
}

fn cmsg_len(fd_count: usize) -> usize {
    16 + mem::size_of::<libc::c_int>() * fd_count
}

/// Send `data` plus `fds` as ancillary `SCM_RIGHTS` in one `sendmsg`.
/// Mirrors `send_fds()`.
pub fn send_fds(fd: &OwnedFd, data: &[u8], fds: &[i32]) -> io::Result<()> {
    let mut iov = libc::iovec {
        iov_base: data.as_ptr() as *mut libc::c_void,
        iov_len: data.len(),
    };
    let cmsg_size = cmsg_space(fds.len()).max(16);
    let mut cmsg_buf = vec![0u8; cmsg_size];
    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = cmsg_buf.len() as _;
    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if cmsg.is_null() {
            return Err(io::Error::new(io::ErrorKind::Other, "CMSG_FIRSTHDR null"));
        }
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = cmsg_len(fds.len()) as _;
        std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(cmsg) as *mut i32, fds.len());
        let n = libc::sendmsg(fd.as_raw_fd(), &msg, libc::MSG_NOSIGNAL);
        if n != data.len() as isize {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Receive `data.len()` bytes plus up to `fds.len()` file descriptors.
/// Returns the number of fds received. Mirrors `recv_fds()`.
pub fn recv_fds(fd: &OwnedFd, data: &mut [u8], fds: &mut [i32]) -> io::Result<usize> {
    let mut iov = libc::iovec {
        iov_base: data.as_mut_ptr() as *mut libc::c_void,
        iov_len: data.len(),
    };
    let cmsg_size = cmsg_space(fds.len()).max(16);
    let mut cmsg_buf = vec![0u8; cmsg_size];
    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = cmsg_buf.len() as _;
    let n = unsafe { libc::recvmsg(fd.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC) };
    if n <= 0 {
        return Err(if n == 0 {
            io::Error::new(io::ErrorKind::UnexpectedEof, "peer closed")
        } else {
            io::Error::last_os_error()
        });
    }
    let mut received = 0usize;
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let bytes = (*cmsg).cmsg_len as usize - cmsg_len(0);
                let count = (bytes / mem::size_of::<libc::c_int>()).min(fds.len());
                std::ptr::copy_nonoverlapping(
                    libc::CMSG_DATA(cmsg) as *const i32,
                    fds.as_mut_ptr(),
                    count,
                );
                received = count;
                break;
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    Ok(received)
}

// ---------------------------------------------------------------------------
// primitives: socketpair / eventfd / memfd / poll
// ---------------------------------------------------------------------------

pub fn socketpair(stream: bool) -> io::Result<(OwnedFd, OwnedFd)> {
    let mut sv = [0 as libc::c_int; 2];
    let typ = if stream {
        libc::SOCK_STREAM
    } else {
        libc::SOCK_SEQPACKET
    };
    let r =
        unsafe { libc::socketpair(libc::AF_UNIX, typ | libc::SOCK_CLOEXEC, 0, sv.as_mut_ptr()) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe { Ok((OwnedFd::from_raw_fd(sv[0]), OwnedFd::from_raw_fd(sv[1]))) }
}

pub fn make_eventfd() -> io::Result<OwnedFd> {
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe { Ok(OwnedFd::from_raw_fd(fd)) }
}

/// Signal an eventfd counter (+1). Uses write(2): eventfds are not sockets,
/// so send(2) fails (this exact bug looped fallback during bring-up).
pub fn eventfd_write(fd: &OwnedFd, value: u64) -> io::Result<()> {
    let bytes = value.to_ne_bytes();
    let mut off = 0;
    while off < bytes.len() {
        let n = unsafe {
            libc::write(
                fd.as_raw_fd(),
                bytes[off..].as_ptr() as *const libc::c_void,
                bytes.len() - off,
            )
        };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        off += n as usize;
    }
    Ok(())
}

#[allow(dead_code)]
pub fn eventfd_read(fd: &OwnedFd) -> io::Result<u64> {
    let mut bytes = [0u8; 8];
    // Non-blocking drain: the producer may have signalled several frames.
    let n = unsafe {
        libc::read(
            fd.as_raw_fd(),
            bytes.as_mut_ptr() as *mut libc::c_void,
            bytes.len(),
        )
    };
    // Temporarily non-blocking via fcntl is overkill here; the caller only
    // drains after select wakeups. A short blocking read is acceptable, but
    // all current call sites drain a just-signalled fd, so require 8 bytes.
    if n != 8 {
        return Err(if n < 0 {
            io::Error::last_os_error()
        } else {
            io::Error::new(io::ErrorKind::UnexpectedEof, "short eventfd read")
        });
    }
    Ok(u64::from_ne_bytes(bytes))
}

/// 4-byte shared selected-buffer index. Returns (fd, *mut u32 mapping).
/// The mapping must be munmap'ed by the caller before dropping the fd.
pub fn make_shm_index() -> io::Result<(OwnedFd, *mut u32)> {
    let fd = unsafe {
        libc::memfd_create(
            c"buf_select".as_ptr(),
            (libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) as u32,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    if unsafe { libc::ftruncate(fd.as_raw_fd(), 4) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            4,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    unsafe { *(ptr as *mut u32) = 0 };
    Ok((fd, ptr as *mut u32))
}

pub fn munmap_index(ptr: *mut u32) {
    if !ptr.is_null() {
        unsafe {
            libc::munmap(ptr as *mut libc::c_void, 4);
        }
    }
}

/// Poll two fds for readability with a millisecond timeout.
/// Returns `(a_ready, b_ready)`; timeout yields `(false, false)`.
pub fn poll_two(a: &OwnedFd, b: &OwnedFd, timeout_ms: i32) -> io::Result<(bool, bool)> {
    use std::os::unix::io::AsRawFd;
    let mut pfds = [
        libc::pollfd {
            fd: a.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: b.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let r = unsafe { libc::poll(pfds.as_mut_ptr(), 2, timeout_ms as libc::c_int) };
    if r < 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            return Ok((false, false));
        }
        return Err(err);
    }
    if r == 0 {
        return Ok((false, false));
    }
    for p in pfds.iter() {
        if p.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            return Err(io::Error::new(io::ErrorKind::ConnectionReset, "hup/err"));
        }
    }
    Ok((
        pfds[0].revents & libc::POLLIN != 0,
        pfds[1].revents & libc::POLLIN != 0,
    ))
}

/// Monotonic clock in nanoseconds (pacing math must be wall-clock immune).
pub fn now_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
    }
    (ts.tv_sec.max(0) as u64)
        .wrapping_mul(1_000_000_000)
        .wrapping_add(ts.tv_nsec.max(0) as u64)
}

// ---------------------------------------------------------------------------
// VSYNC pump (Android Choreographer, dlopen'd — no NDK dependency)
// ---------------------------------------------------------------------------

type FrameCallback64 = unsafe extern "C" fn(frame_time_nanos: i64, data: *mut libc::c_void);

#[derive(Clone, Copy)]
struct ChoreoApi {
    _lib: *mut libc::c_void,
    looper_prepare: unsafe extern "C" fn(i32) -> *mut libc::c_void,
    looper_poll_once: unsafe extern "C" fn(i32, *mut i32, *mut i32, *mut *mut libc::c_void) -> i32,
    looper_release: unsafe extern "C" fn(*mut libc::c_void),
    get_instance: unsafe extern "C" fn() -> *mut libc::c_void,
    post_frame_callback64:
        unsafe extern "C" fn(*mut libc::c_void, FrameCallback64, *mut libc::c_void),
}

// SAFETY: function pointers loaded once, then only called.
unsafe impl Send for ChoreoApi {}
unsafe impl Sync for ChoreoApi {}

fn load_choreo() -> Option<ChoreoApi> {
    // NUL-terminated symbol names as byte strings (no c-string concat needed).
    unsafe fn sym(lib: *mut libc::c_void, name: &[u8]) -> Option<*mut libc::c_void> {
        let p = unsafe { libc::dlsym(lib, name.as_ptr() as *const libc::c_char) };
        if p.is_null() {
            None
        } else {
            Some(p)
        }
    }
    unsafe {
        let lib = libc::dlopen(
            b"libandroid.so\0".as_ptr() as *const libc::c_char,
            libc::RTLD_NOW,
        );
        if lib.is_null() {
            return None;
        }
        macro_rules! load {
            ($name:expr, $ty:ty) => {
                match sym(lib, $name) {
                    Some(p) => std::mem::transmute::<*mut libc::c_void, $ty>(p),
                    None => {
                        libc::dlclose(lib);
                        return None;
                    }
                }
            };
        }
        Some(ChoreoApi {
            _lib: lib,
            looper_prepare: load!(
                b"ALooper_prepare\0",
                unsafe extern "C" fn(i32) -> *mut libc::c_void
            ),
            looper_poll_once: load!(
                b"ALooper_pollOnce\0",
                unsafe extern "C" fn(i32, *mut i32, *mut i32, *mut *mut libc::c_void) -> i32
            ),
            looper_release: load!(
                b"ALooper_release\0",
                unsafe extern "C" fn(*mut libc::c_void)
            ),
            get_instance: load!(
                b"AChoreographer_getInstance\0",
                unsafe extern "C" fn() -> *mut libc::c_void
            ),
            post_frame_callback64: load!(
                b"AChoreographer_postFrameCallback64\0",
                unsafe extern "C" fn(*mut libc::c_void, FrameCallback64, *mut libc::c_void)
            ),
        })
    }
}

static CHOREO: OnceLock<Option<ChoreoApi>> = OnceLock::new();

fn choreo_api() -> Option<&'static ChoreoApi> {
    CHOREO.get_or_init(load_choreo).as_ref()
}

/// Choreographer frame trampoline: runs on the pump thread's looper, writes
/// one tick per display vsync. `data` is the heap holder for the tick fd,
/// alive exactly while the pump thread runs (reclaimed on thread exit).
/// Callback context: heap holder owned by the pump thread. The trampoline
/// runs on the pump thread's looper, so bumping `count` needs no sync.
struct TickHolder {
    tick_fd: libc::c_int,
    count: u64,
}

unsafe extern "C" fn frame_trampoline(_when_ns: i64, data: *mut libc::c_void) {
    if data.is_null() {
        return;
    }
    let holder = unsafe { &mut *(data as *mut TickHolder) };
    holder.count += 1;
    let one: u64 = 1;
    unsafe {
        libc::write(
            holder.tick_fd,
            &one as *const u64 as *const libc::c_void,
            8,
        );
    }
}

fn pump_thread_choreo(api: ChoreoApi, tick_fd: libc::c_int, running: Arc<AtomicBool>) {
    unsafe {
        let looper = (api.looper_prepare)(1); // ALOOPER_PREPARE_ALLOW_NON_CALLBACKS
        if looper.is_null() {
            log::warn!("anland.vsync looper_prepare failed; timer fallback");
            pump_thread_timer(tick_fd, running, 16_666_666);
            return;
        }
        let choreo = (api.get_instance)();
        if choreo.is_null() {
            log::warn!("anland.vsync choreographer instance null; timer fallback");
            (api.looper_release)(looper);
            pump_thread_timer(tick_fd, running, 16_666_666);
            return;
        }
        // Holder owned by this thread; the callback only ever runs here.
        let holder = Box::new(TickHolder { tick_fd, count: 0 });
        let holder_ptr = Box::into_raw(holder) as *mut libc::c_void;
        let mut last_count: u64 = 0;
        let mut last_log_ns = now_ns();
        while running.load(Ordering::Acquire) {
            (api.post_frame_callback64)(choreo, frame_trampoline, holder_ptr);
            // Returns on callback dispatch, or after 250ms so stop() stays
            // bounded without cross-thread looper wakeups.
            let r = (api.looper_poll_once)(
                250,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            // r: LOOPER_POLL_WAKE=-1, CALLBACK=1, TIMEOUT=0, ERROR=-2..-4.
            let now = now_ns();
            if now.wrapping_sub(last_log_ns) >= 5_000_000_000 {
                let total = unsafe { (*(holder_ptr as *mut TickHolder)).count };
                log::info!(
                    "anland.vsync alive callbacks_5s={} poll_last={r}",
                    total.wrapping_sub(last_count)
                );
                last_count = total;
                last_log_ns = now;
            }
        }
        // A callback posted but never dispatched (stop raced it) simply never
        // fires: the looper is no longer pumped after this thread exits.
        let _ = Box::from_raw(holder_ptr as *mut TickHolder);
        (api.looper_release)(looper);
    }
}

fn pump_thread_timer(tick_fd: libc::c_int, running: Arc<AtomicBool>, period_ns: u64) {
    let step = std::time::Duration::from_millis(50);
    let mut acc: u64 = 0;
    let mut ticks: u64 = 0;
    let mut last_log_ns = now_ns();
    while running.load(Ordering::Acquire) {
        std::thread::sleep(step);
        acc += 50_000_000;
        if acc >= period_ns {
            acc = 0;
            ticks += 1;
            let one: u64 = 1;
            unsafe {
                libc::write(
                    tick_fd,
                    &one as *const u64 as *const libc::c_void,
                    8,
                );
            }
        }
        let now = now_ns();
        if now.wrapping_sub(last_log_ns) >= 5_000_000_000 {
            log::info!("anland.vsync alive ticks_5s={ticks} mode=timer");
            ticks = 0;
            last_log_ns = now;
        }
    }
}

/// Display-VSYNC tick source for demand-driven presentation.
///
/// Preferred mode is Android Choreographer on a dedicated looper thread (one
/// eventfd write per display vsync, no polling, no timers). When the NDK
/// symbols are unavailable it degrades to a nanosleep timer at the panel
/// rate. The render loop polls the tick fd alongside its kick fd, so both
/// modes share the pacing logic.
pub struct VsyncPump {
    tick: OwnedFd,
    running: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    mode: &'static str,
    period_ns: u64,
}

impl VsyncPump {
    pub fn start(refresh_mhz: u32) -> io::Result<Self> {
        let tick = make_eventfd()?;
        // refresh is millihertz (144000 = 144Hz): period_ns = 1e12/mHz.
        let period_ns = if refresh_mhz > 0 {
            1_000_000_000_000u64 / refresh_mhz.max(1) as u64
        } else {
            16_666_666
        };
        let running = Arc::new(AtomicBool::new(true));
        let tick_fd = tick.as_raw_fd();
        let (mode, handle) = match choreo_api() {
            Some(api) => {
                let api = *api;
                let running = running.clone();
                let h = std::thread::Builder::new()
                    .name("anland-vsync".into())
                    .spawn(move || pump_thread_choreo(api, tick_fd, running))
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                ("choreographer", h)
            }
            None => {
                let running = running.clone();
                let h = std::thread::Builder::new()
                    .name("anland-vsync-timer".into())
                    .spawn(move || pump_thread_timer(tick_fd, running, period_ns))
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                ("timer", h)
            }
        };
        log::info!("anland.vsync mode={mode} period_ns={period_ns}");
        Ok(Self {
            tick,
            running,
            handle: Some(handle),
            mode,
            period_ns,
        })
    }

    pub fn tick_fd(&self) -> &OwnedFd {
        &self.tick
    }

    pub fn mode(&self) -> &'static str {
        self.mode
    }

    pub fn period_ns(&self) -> u64 {
        self.period_ns
    }

    pub fn stop(mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(h) = self.handle.take() {
            // Bounded by construction (250ms looper poll / 50ms timer step).
            let _ = h.join();
        }
    }
}

/// Poll `fd` for readability with a millisecond timeout.
/// Returns Ok(true) when readable, Ok(false) on timeout.
pub fn poll_readable(fd: &OwnedFd, timeout_ms: i32) -> io::Result<bool> {
    let mut pfd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let r = unsafe { libc::poll(&mut pfd, 1, timeout_ms as libc::c_int) };
    if r < 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(err);
    }
    if r == 0 {
        return Ok(false);
    }
    if pfd.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
        return Err(io::Error::new(io::ErrorKind::ConnectionReset, "hup/err"));
    }
    Ok(pfd.revents & libc::POLLIN != 0)
}

/// Wait (up to `timeout_ms`) for a sync-file fence fd to signal, then close it.
/// A sync file signals POLLIN when the GPU work completes.
pub fn wait_fence(fence_fd: OwnedFd, timeout_ms: i32) -> io::Result<()> {
    let readable = poll_readable(&fence_fd, timeout_ms)?;
    if !readable {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "fence wait timed out",
        ));
    }
    Ok(())
}
