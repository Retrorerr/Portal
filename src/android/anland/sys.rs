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
    Arc, Mutex, OnceLock,
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
type FrameCallbackData = libc::c_void;
type LooperCallback = unsafe extern "C" fn(
    fd: libc::c_int,
    events: libc::c_int,
    data: *mut libc::c_void,
) -> libc::c_int;
type VsyncCallback =
    unsafe extern "C" fn(data: *const FrameCallbackData, callback_data: *mut libc::c_void);
type PostVsyncCallback = unsafe extern "C" fn(*mut libc::c_void, VsyncCallback, *mut libc::c_void);
type GetFrameTime = unsafe extern "C" fn(*const FrameCallbackData) -> i64;
type GetPreferredTimeline = unsafe extern "C" fn(*const FrameCallbackData) -> usize;
type GetTimelineCount = unsafe extern "C" fn(*const FrameCallbackData) -> usize;
type GetTimelineDeadline = unsafe extern "C" fn(*const FrameCallbackData, usize) -> i64;
type GetTimelineExpectedPresent = unsafe extern "C" fn(*const FrameCallbackData, usize) -> i64;
type GetTimelineVsyncId = unsafe extern "C" fn(*const FrameCallbackData, usize) -> i64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VsyncTimeline {
    pub frame_time_ns: u64,
    pub deadline_ns: u64,
    pub expected_present_ns: u64,
    pub vsync_id: i64,
}

#[derive(Clone, Copy)]
struct TimelineFunctions {
    post: PostVsyncCallback,
    frame_time: GetFrameTime,
    preferred: GetPreferredTimeline,
    count: GetTimelineCount,
    deadline: GetTimelineDeadline,
    expected_present: GetTimelineExpectedPresent,
    vsync_id: GetTimelineVsyncId,
}

#[derive(Clone, Copy)]
struct ChoreoApi {
    _lib: *mut libc::c_void,
    looper_prepare: unsafe extern "C" fn(i32) -> *mut libc::c_void,
    looper_add_fd: unsafe extern "C" fn(
        *mut libc::c_void,
        libc::c_int,
        libc::c_int,
        libc::c_int,
        Option<LooperCallback>,
        *mut libc::c_void,
    ) -> libc::c_int,
    looper_poll_once: unsafe extern "C" fn(i32, *mut i32, *mut i32, *mut *mut libc::c_void) -> i32,
    looper_release: unsafe extern "C" fn(*mut libc::c_void),
    get_instance: unsafe extern "C" fn() -> *mut libc::c_void,
    post_frame_callback64:
        unsafe extern "C" fn(*mut libc::c_void, FrameCallback64, *mut libc::c_void),
    timeline: Option<TimelineFunctions>,
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
        macro_rules! load_required {
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
        macro_rules! load_optional {
            ($name:expr, $ty:ty) => {
                sym(lib, $name).map(|p| std::mem::transmute::<*mut libc::c_void, $ty>(p))
            };
        }
        let timeline = match (
            load_optional!(b"AChoreographer_postVsyncCallback\0", PostVsyncCallback),
            load_optional!(
                b"AChoreographerFrameCallbackData_getFrameTimeNanos\0",
                GetFrameTime
            ),
            load_optional!(
                b"AChoreographerFrameCallbackData_getPreferredFrameTimelineIndex\0",
                GetPreferredTimeline
            ),
            load_optional!(
                b"AChoreographerFrameCallbackData_getFrameTimelinesLength\0",
                GetTimelineCount
            ),
            load_optional!(
                b"AChoreographerFrameCallbackData_getFrameTimelineDeadlineNanos\0",
                GetTimelineDeadline
            ),
            load_optional!(
                b"AChoreographerFrameCallbackData_getFrameTimelineExpectedPresentationTimeNanos\0",
                GetTimelineExpectedPresent
            ),
            load_optional!(
                b"AChoreographerFrameCallbackData_getFrameTimelineVsyncId\0",
                GetTimelineVsyncId
            ),
        ) {
            (
                Some(post),
                Some(frame_time),
                Some(preferred),
                Some(count),
                Some(deadline),
                Some(expected_present),
                Some(vsync_id),
            ) => Some(TimelineFunctions {
                post,
                frame_time,
                preferred,
                count,
                deadline,
                expected_present,
                vsync_id,
            }),
            _ => None,
        };
        Some(ChoreoApi {
            _lib: lib,
            looper_prepare: load_required!(
                b"ALooper_prepare\0",
                unsafe extern "C" fn(i32) -> *mut libc::c_void
            ),
            looper_add_fd: load_required!(
                b"ALooper_addFd\0",
                unsafe extern "C" fn(
                    *mut libc::c_void,
                    libc::c_int,
                    libc::c_int,
                    libc::c_int,
                    Option<LooperCallback>,
                    *mut libc::c_void,
                ) -> libc::c_int
            ),
            looper_poll_once: load_required!(
                b"ALooper_pollOnce\0",
                unsafe extern "C" fn(i32, *mut i32, *mut i32, *mut *mut libc::c_void) -> i32
            ),
            looper_release: load_required!(
                b"ALooper_release\0",
                unsafe extern "C" fn(*mut libc::c_void)
            ),
            get_instance: load_required!(
                b"AChoreographer_getInstance\0",
                unsafe extern "C" fn() -> *mut libc::c_void
            ),
            post_frame_callback64: load_required!(
                b"AChoreographer_postFrameCallback64\0",
                unsafe extern "C" fn(*mut libc::c_void, FrameCallback64, *mut libc::c_void)
            ),
            timeline,
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
    callback_pending: Arc<AtomicBool>,
    request_pending: Arc<AtomicBool>,
    timeline: Arc<Mutex<Option<VsyncTimeline>>>,
    timeline_functions: Option<TimelineFunctions>,
}

unsafe extern "C" fn frame_trampoline(_when_ns: i64, data: *mut libc::c_void) {
    if data.is_null() {
        return;
    }
    let holder = unsafe { &mut *(data as *mut TickHolder) };
    holder.callback_pending.store(false, Ordering::Release);
    holder.request_pending.store(false, Ordering::Release);
    if let Ok(mut timeline) = holder.timeline.lock() {
        *timeline = None;
    }
    holder.count += 1;
    let one: u64 = 1;
    unsafe {
        libc::write(holder.tick_fd, &one as *const u64 as *const libc::c_void, 8);
    }
}

fn read_eventfd_raw(fd: libc::c_int) -> io::Result<u64> {
    let mut bytes = [0u8; 8];
    loop {
        let n = unsafe { libc::read(fd, bytes.as_mut_ptr() as *mut libc::c_void, bytes.len()) };
        if n == bytes.len() as isize {
            return Ok(u64::from_ne_bytes(bytes));
        }
        if n < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "short eventfd read",
        ));
    }
}

fn pump_thread_choreo(
    api: ChoreoApi,
    tick_fd: libc::c_int,
    request_fd: libc::c_int,
    request_pending: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    period_ns: u64,
    timeline: Arc<Mutex<Option<VsyncTimeline>>>,
) {
    unsafe {
        let looper = (api.looper_prepare)(1); // ALOOPER_PREPARE_ALLOW_NON_CALLBACKS
        if looper.is_null() {
            log::warn!("anland.vsync looper_prepare failed; timer fallback");
            pump_thread_timer(
                tick_fd,
                request_fd,
                request_pending,
                running,
                period_ns,
                timeline,
            );
            return;
        }
        let choreo = (api.get_instance)();
        if choreo.is_null() {
            log::warn!("anland.vsync choreographer instance null; timer fallback");
            (api.looper_release)(looper);
            pump_thread_timer(
                tick_fd,
                request_fd,
                request_pending,
                running,
                period_ns,
                timeline,
            );
            return;
        }
        const REQUEST_IDENT: libc::c_int = 1;
        // Holder owned by this thread; the callback only ever runs here.
        let callback_pending = Arc::new(AtomicBool::new(false));
        let holder = Box::new(TickHolder {
            tick_fd,
            count: 0,
            callback_pending: callback_pending.clone(),
            request_pending: request_pending.clone(),
            timeline: timeline.clone(),
            timeline_functions: api.timeline,
        });
        let holder_ptr = Box::into_raw(holder) as *mut libc::c_void;
        let add_result = (api.looper_add_fd)(
            looper,
            request_fd,
            REQUEST_IDENT,
            1, // ALOOPER_EVENT_INPUT
            None,
            std::ptr::null_mut(),
        );
        if add_result < 0 {
            log::warn!("anland.vsync request fd registration failed; timer fallback");
            let _ = Box::from_raw(holder_ptr as *mut TickHolder);
            (api.looper_release)(looper);
            pump_thread_timer(
                tick_fd,
                request_fd,
                request_pending,
                running,
                period_ns,
                timeline,
            );
            return;
        }
        let mut posts: u64 = 0;
        let mut last_count: u64 = 0;
        let mut last_log_ns = now_ns();
        while running.load(Ordering::Acquire) {
            // The request fd is the only wake source while idle. A callback
            // remains one-shot and at most one callback is outstanding. The
            // atomic request token coalesces all producer requests received
            // while that callback is pending.
            let mut out_fd = 0;
            let mut out_events = 0;
            let mut out_data = std::ptr::null_mut();
            let r = (api.looper_poll_once)(-1, &mut out_fd, &mut out_events, &mut out_data);
            if r == REQUEST_IDENT {
                let _ = read_eventfd_raw(request_fd);
                if request_pending.load(Ordering::Acquire)
                    && !callback_pending.load(Ordering::Acquire)
                {
                    callback_pending.store(true, Ordering::Release);
                    if let Some(timeline) = api.timeline {
                        (timeline.post)(choreo, vsync_trampoline, holder_ptr);
                    } else {
                        (api.post_frame_callback64)(choreo, frame_trampoline, holder_ptr);
                    }
                    posts += 1;
                }
            }
            // The callback itself clears callback_pending. ALooper's callback
            // return value is not used as proof that a frame was requested.
            let now = now_ns();
            if now.wrapping_sub(last_log_ns) >= 5_000_000_000 {
                let total = (*(holder_ptr as *mut TickHolder)).count;
                log::info!(
                    "anland.vsync alive callbacks_5s={} posts_5s={} pending={} poll_last={r}",
                    total.wrapping_sub(last_count),
                    posts,
                    callback_pending.load(Ordering::Acquire)
                );
                last_count = total;
                posts = 0;
                last_log_ns = now;
            }
        }
        // A callback posted but never dispatched (stop raced it) simply never
        // fires: the looper is no longer pumped after this thread exits.
        let _ = Box::from_raw(holder_ptr as *mut TickHolder);
        (api.looper_release)(looper);
    }
}

fn pump_thread_timer(
    tick_fd: libc::c_int,
    request_fd: libc::c_int,
    request_pending: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    period_ns: u64,
    timeline: Arc<Mutex<Option<VsyncTimeline>>>,
) {
    let period_ns = period_ns.max(1);
    let mut next_deadline = now_ns().saturating_add(period_ns);
    let mut requested = false;
    let mut ticks: u64 = 0;
    let mut last_log_ns = now_ns();
    while running.load(Ordering::Acquire) {
        if !requested {
            match poll_raw_readable(request_fd, -1) {
                Ok(true) => {
                    let _ = read_eventfd_raw(request_fd);
                    requested = request_pending.load(Ordering::Acquire);
                }
                Ok(false) => continue,
                Err(error) => {
                    log::warn!("anland.vsync timer request poll failed: {error}");
                    break;
                }
            }
            if !running.load(Ordering::Acquire) {
                break;
            }
        }
        if !requested {
            continue;
        }
        let now = now_ns();
        if now < next_deadline {
            let remaining_ns = next_deadline - now;
            let timeout_ms = remaining_ns
                .saturating_add(999_999)
                .saturating_div(1_000_000)
                .min(i32::MAX as u64) as i32;
            match poll_raw_readable(request_fd, timeout_ms) {
                Ok(true) => {
                    let _ = read_eventfd_raw(request_fd);
                }
                Ok(false) => {}
                Err(error) => {
                    log::warn!("anland.vsync timer deadline poll failed: {error}");
                    break;
                }
            }
            continue;
        }
        requested = false;
        request_pending.store(false, Ordering::Release);
        ticks += 1;
        if let Ok(mut target) = timeline.lock() {
            *target = Some(VsyncTimeline {
                frame_time_ns: now,
                deadline_ns: 0,
                expected_present_ns: next_deadline,
                vsync_id: -1,
            });
        }
        let one: u64 = 1;
        unsafe {
            libc::write(tick_fd, &one as *const u64 as *const libc::c_void, 8);
        }
        // A late wake produces one tick, then skips directly to the next
        // future deadline. Never burst stale ticks and never accumulate drift.
        advance_timer_deadline(&mut next_deadline, now_ns(), period_ns);
        let now = now_ns();
        if now.wrapping_sub(last_log_ns) >= 5_000_000_000 {
            log::info!("anland.vsync alive ticks_5s={ticks} mode=timer");
            ticks = 0;
            last_log_ns = now;
        }
    }
}

unsafe extern "C" fn vsync_trampoline(
    data: *const FrameCallbackData,
    callback_data: *mut libc::c_void,
) {
    if callback_data.is_null() {
        return;
    }
    let holder = unsafe { &mut *(callback_data as *mut TickHolder) };
    holder.callback_pending.store(false, Ordering::Release);
    holder.request_pending.store(false, Ordering::Release);
    let Some(functions) = holder.timeline_functions else {
        return;
    };
    if data.is_null() {
        return;
    }
    let preferred = unsafe { (functions.preferred)(data) };
    let count = unsafe { (functions.count)(data) };
    if preferred >= count {
        return;
    }
    let timeline = VsyncTimeline {
        frame_time_ns: unsafe { (functions.frame_time)(data) }.max(0) as u64,
        deadline_ns: unsafe { (functions.deadline)(data, preferred) }.max(0) as u64,
        expected_present_ns: unsafe { (functions.expected_present)(data, preferred) }.max(0) as u64,
        vsync_id: unsafe { (functions.vsync_id)(data, preferred) },
    };
    if let Ok(mut target) = holder.timeline.lock() {
        *target = Some(timeline);
    }
    holder.count += 1;
    let one: u64 = 1;
    unsafe {
        libc::write(holder.tick_fd, &one as *const u64 as *const libc::c_void, 8);
    }
}

fn advance_timer_deadline(next_deadline: &mut u64, now: u64, period_ns: u64) {
    let period_ns = period_ns.max(1);
    if now < *next_deadline {
        return;
    }
    let elapsed_periods = now.saturating_sub(*next_deadline) / period_ns + 1;
    *next_deadline = next_deadline.saturating_add(period_ns.saturating_mul(elapsed_periods));
}

/// On-demand display-VSYNC telemetry source for producer pacing.
///
/// A producer request calls [`VsyncPump::request`], which posts at most one
/// Android Choreographer callback. When the NDK symbols are unavailable it
/// uses an absolute monotonic timer for the same one-shot telemetry event.
/// The render loop polls the tick fd alongside its producer/lifecycle wake;
/// the tick is never required to authorize presentation.
pub struct VsyncPump {
    tick: OwnedFd,
    request_fd: OwnedFd,
    request_pending: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    mode: &'static str,
    period_ns: u64,
    timeline: Arc<Mutex<Option<VsyncTimeline>>>,
}

impl VsyncPump {
    pub fn start(refresh_mhz: u32) -> io::Result<Self> {
        let tick = make_eventfd()?;
        let request_fd = make_eventfd()?;
        // refresh is millihertz (144000 = 144Hz): period_ns = 1e12/mHz.
        let period_ns = if refresh_mhz > 0 {
            1_000_000_000_000u64 / refresh_mhz.max(1) as u64
        } else {
            16_666_666
        };
        let running = Arc::new(AtomicBool::new(true));
        let request_pending = Arc::new(AtomicBool::new(false));
        let timeline = Arc::new(Mutex::new(None));
        let tick_fd = tick.as_raw_fd();
        let request_fd_raw = request_fd.as_raw_fd();
        let (mode, handle) = match choreo_api() {
            Some(api) => {
                let api = *api;
                let request_pending = request_pending.clone();
                let running = running.clone();
                let timeline = timeline.clone();
                let h = std::thread::Builder::new()
                    .name("anland-vsync".into())
                    .spawn(move || {
                        pump_thread_choreo(
                            api,
                            tick_fd,
                            request_fd_raw,
                            request_pending,
                            running,
                            period_ns,
                            timeline,
                        )
                    })
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                ("choreographer", h)
            }
            None => {
                let request_pending = request_pending.clone();
                let running = running.clone();
                let timeline = timeline.clone();
                let h = std::thread::Builder::new()
                    .name("anland-vsync-timer".into())
                    .spawn(move || {
                        pump_thread_timer(
                            tick_fd,
                            request_fd_raw,
                            request_pending,
                            running,
                            period_ns,
                            timeline,
                        )
                    })
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                ("timer", h)
            }
        };
        log::info!("anland.vsync mode={mode} period_ns={period_ns}");
        Ok(Self {
            tick,
            request_fd,
            request_pending,
            running,
            handle: Some(handle),
            mode,
            period_ns,
            timeline,
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

    /// Request one timing callback. Duplicate requests are counted by the
    /// atomic edge token and coalesced to one outstanding Choreographer
    /// callback.
    pub fn request(&self) -> io::Result<()> {
        if self
            .request_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(());
        }
        if let Err(error) = eventfd_write(&self.request_fd, 1) {
            self.request_pending.store(false, Ordering::Release);
            return Err(error);
        }
        Ok(())
    }

    /// Consume the timeline associated with the most recently delivered tick.
    /// A work item may use it once; stale timing data must never be reused by
    /// a later frame.
    pub fn take_timeline(&self) -> Option<VsyncTimeline> {
        self.timeline.lock().ok().and_then(|mut value| value.take())
    }

    pub fn stop(mut self) {
        self.running.store(false, Ordering::Release);
        // Wake either the ALooper or timer poll so stop is independent of the
        // next display callback/deadline.
        let _ = eventfd_write(&self.request_fd, 1);
        if let Some(h) = self.handle.take() {
            // Bounded by the request-fd wake for both pacing backends.
            let _ = h.join();
        }
    }
}

/// Poll `fd` for readability with a millisecond timeout.
/// Returns Ok(true) when readable, Ok(false) on timeout.
pub fn poll_readable(fd: &OwnedFd, timeout_ms: i32) -> io::Result<bool> {
    poll_raw_readable(fd.as_raw_fd(), timeout_ms)
}

fn poll_raw_readable(fd: libc::c_int, timeout_ms: i32) -> io::Result<bool> {
    let mut pfd = libc::pollfd {
        fd,
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

/// Wait (up to `timeout_ms`) for a sync-file fence fd to signal.
///
/// The fd is borrowed deliberately.  A timeout or poll error must leave the
/// acquire fence owned by the dequeued-buffer lease so it can be transferred
/// to `cancelBuffer`; closing an unsignaled acquire fence and substituting
/// `-1` would discard SurfaceFlinger's dependency.
pub fn wait_fence(fence_fd: &OwnedFd, timeout_ms: i32) -> io::Result<()> {
    let readable = poll_readable(fence_fd, timeout_ms)?;
    if !readable {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "fence wait timed out",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::advance_timer_deadline;

    #[test]
    fn absolute_deadline_timer_covers_common_refresh_rates() {
        for period in [16_666_666, 11_111_111, 8_333_333, 6_944_444] {
            let mut deadline = 1_000_000_000u64;
            let late = deadline + period * 3 + 1;
            advance_timer_deadline(&mut deadline, late, period);
            assert_eq!(deadline, 1_000_000_000 + period * 4);
        }
    }

    #[test]
    fn absolute_deadline_timer_does_not_advance_early_or_burst() {
        let period = 6_944_444;
        let mut deadline = 1_000_000_000u64;
        let early = deadline - 1;
        advance_timer_deadline(&mut deadline, early, period);
        assert_eq!(deadline, 1_000_000_000);
        let very_late = deadline + period * 100;
        advance_timer_deadline(&mut deadline, very_late, period);
        assert_eq!(deadline, 1_000_000_000 + period * 101);
    }
}
