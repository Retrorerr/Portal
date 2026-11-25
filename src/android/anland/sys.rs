//! Raw fd plumbing for the Anland consumer/broker (Android-only).
//!
//! Mirrors `third_party/anland/common/socket_utils.c` plus the
//! eventfd/memfd/shm setup from `display_consumer.c`. All fds are owned via
//! [`OwnedFd`]-style wrappers so fallback teardown cannot leak or double-close.

use std::io;
use std::mem;
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};

/// Return the backing size in bytes of a dma-buf fd via `lseek(SEEK_END)`.
pub fn dmabuf_size(fd: libc::c_int) -> Option<u64> {
    if fd < 0 {
        return None;
    }
    let size = unsafe { libc::lseek(fd, 0, libc::SEEK_END) };
    (size > 0).then_some(size as u64)
}

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

/// Most fds one SCM_RIGHTS message carries here (the broker's buffer set).
const MAX_PASSED_FDS: usize = 16;

/// Control-message buffer with `cmsghdr` alignment. A plain `[u8]` buffer
/// is only byte-aligned, which makes every `(*cmsg)` access undefined.
#[repr(C, align(8))]
struct CmsgBuf([u8; cmsg_space(MAX_PASSED_FDS)]);

const fn cmsg_space(fd_count: usize) -> usize {
    // CMSG_SPACE(sizeof(int) * n) for the Linux aarch64/x86_64 layout:
    // 16-byte header + payload aligned to 8. Matches CMSG_SPACE on Android.
    const ALIGN: usize = 8;
    16 + (mem::size_of::<libc::c_int>() * fd_count).div_ceil(ALIGN) * ALIGN
}

const fn cmsg_len(fd_count: usize) -> usize {
    16 + mem::size_of::<libc::c_int>() * fd_count
}

/// Send `data` plus `fds` as ancillary `SCM_RIGHTS` in one `sendmsg`.
/// Mirrors `send_fds()`.
pub fn send_fds(fd: &OwnedFd, data: &[u8], fds: &[i32]) -> io::Result<()> {
    if fds.len() > MAX_PASSED_FDS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many fds for one message",
        ));
    }
    let mut iov = libc::iovec {
        iov_base: data.as_ptr() as *mut libc::c_void,
        iov_len: data.len(),
    };
    let mut cmsg_buf = CmsgBuf([0; cmsg_space(MAX_PASSED_FDS)]);
    // SAFETY: msghdr is plain old data; all-zero is a valid empty header.
    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.0.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = cmsg_space(fds.len()) as _;
    // SAFETY: the control buffer is 8-byte aligned and holds CMSG_SPACE for
    // `fds.len()` ints, so the header and payload writes stay in bounds.
    let n = unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if cmsg.is_null() {
            return Err(io::Error::other("CMSG_FIRSTHDR null"));
        }
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = cmsg_len(fds.len()) as _;
        std::ptr::copy_nonoverlapping(
            fds.as_ptr().cast::<u8>(),
            libc::CMSG_DATA(cmsg),
            mem::size_of_val(fds),
        );
        libc::sendmsg(fd.as_raw_fd(), &msg, libc::MSG_NOSIGNAL)
    };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    if n as usize != data.len() {
        return Err(io::Error::new(io::ErrorKind::WriteZero, "short sendmsg"));
    }
    Ok(())
}

/// Bytes and file descriptors one [`recv_fds`] call received.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Received {
    /// Bytes written to `data`; at least one (a closed peer is an error).
    pub bytes: usize,
    /// File descriptors written to the front of `fds`.
    pub fds: usize,
}

/// Receive up to `data.len()` bytes plus up to `fds.len()` file descriptors,
/// close-on-exec. Fds beyond `fds.len()` are closed instead of leaked.
/// `flags` adds e.g. `MSG_DONTWAIT`. Mirrors `recv_fds()`.
pub fn recv_fds(
    fd: &OwnedFd,
    data: &mut [u8],
    fds: &mut [i32],
    flags: libc::c_int,
) -> io::Result<Received> {
    let mut iov = libc::iovec {
        iov_base: data.as_mut_ptr() as *mut libc::c_void,
        iov_len: data.len(),
    };
    let mut cmsg_buf = CmsgBuf([0; cmsg_space(MAX_PASSED_FDS)]);
    // SAFETY: msghdr is plain old data; all-zero is a valid empty header.
    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.0.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = mem::size_of::<CmsgBuf>() as _;
    // SAFETY: iov and control point at live buffers of the stated lengths.
    let n = unsafe { libc::recvmsg(fd.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC | flags) };
    if n <= 0 {
        return Err(if n == 0 {
            io::Error::new(io::ErrorKind::UnexpectedEof, "peer closed")
        } else {
            io::Error::last_os_error()
        });
    }
    let mut received = 0usize;
    // SAFETY: the kernel filled `msg_controllen` bytes of the aligned control
    // buffer with well-formed headers; CMSG_* walk only within it.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let bytes = ((*cmsg).cmsg_len as usize).saturating_sub(cmsg_len(0));
                let data_ptr = libc::CMSG_DATA(cmsg).cast_const();
                for i in 0..bytes / mem::size_of::<libc::c_int>() {
                    let mut raw = [0u8; mem::size_of::<libc::c_int>()];
                    std::ptr::copy_nonoverlapping(
                        data_ptr.add(i * raw.len()),
                        raw.as_mut_ptr(),
                        raw.len(),
                    );
                    let passed = libc::c_int::from_ne_bytes(raw);
                    if received < fds.len() {
                        fds[received] = passed;
                        received += 1;
                    } else {
                        libc::close(passed);
                    }
                }
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    Ok(Received {
        bytes: n as usize,
        fds: received,
    })
}

// ---------------------------------------------------------------------------
// systrace markers
// ---------------------------------------------------------------------------

/// Android systrace sections and counters for the presentation path. They cost
/// one load and branch while tracing is off, and nothing where `libandroid`
/// is missing (host tests).
pub mod trace {
    use std::sync::OnceLock;

    struct Api {
        is_enabled: unsafe extern "C" fn() -> bool,
        begin: unsafe extern "C" fn(*const libc::c_char),
        end: unsafe extern "C" fn(),
        counter: Option<unsafe extern "C" fn(*const libc::c_char, i64)>,
    }

    fn api() -> Option<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(|| unsafe {
            let lib = libc::dlopen(c"libandroid.so".as_ptr(), libc::RTLD_NOW);
            if lib.is_null() {
                return None;
            }
            let symbol = |name: &std::ffi::CStr| libc::dlsym(lib, name.as_ptr());
            let (is_enabled, begin, end) = (
                symbol(c"ATrace_isEnabled"),
                symbol(c"ATrace_beginSection"),
                symbol(c"ATrace_endSection"),
            );
            if is_enabled.is_null() || begin.is_null() || end.is_null() {
                return None;
            }
            let counter = symbol(c"ATrace_setCounter");
            Some(Api {
                is_enabled: std::mem::transmute::<*mut libc::c_void, unsafe extern "C" fn() -> bool>(is_enabled),
                begin: std::mem::transmute::<*mut libc::c_void, unsafe extern "C" fn(*const libc::c_char)>(begin),
                end: std::mem::transmute::<*mut libc::c_void, unsafe extern "C" fn()>(end),
                counter: (!counter.is_null()).then(|| {
                    std::mem::transmute::<*mut libc::c_void, unsafe extern "C" fn(*const libc::c_char, i64)>(counter)
                }),
            })
        })
        .as_ref()
    }

    /// An open section, closed when dropped.
    pub struct Section(bool);

    impl Drop for Section {
        fn drop(&mut self) {
            if self.0 {
                if let Some(api) = api() {
                    unsafe { (api.end)() };
                }
            }
        }
    }

    pub fn section(name: &std::ffi::CStr) -> Section {
        match api() {
            Some(api) if unsafe { (api.is_enabled)() } => {
                unsafe { (api.begin)(name.as_ptr()) };
                Section(true)
            }
            _ => Section(false),
        }
    }

    pub fn counter(name: &std::ffi::CStr, value: i64) {
        if let Some(api) = api() {
            if let Some(counter) = api.counter {
                if unsafe { (api.is_enabled)() } {
                    unsafe { counter(name.as_ptr(), value) };
                }
            }
        }
    }
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
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
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

/// Extend demand without submitting another off-vsync frame for every input
/// sample. Only an idle-to-active transition needs to wake the render loop;
/// an active loop already receives display ticks. Read the clock after the
/// atomic update so a concurrent disarm cannot lose the wake.
pub fn extend_demand_until(deadline: &AtomicU64, until: u64) -> bool {
    deadline.fetch_max(until, Ordering::AcqRel) <= now_ns()
}

// ---------------------------------------------------------------------------
// VSYNC pump (Android Choreographer, dlopen'd — no NDK dependency)
// ---------------------------------------------------------------------------

type FrameCallback64 = unsafe extern "C" fn(frame_time_nanos: i64, data: *mut libc::c_void);
type VsyncCallback =
    unsafe extern "C" fn(callback_data: *const libc::c_void, data: *mut libc::c_void);

/// `AChoreographer_postVsyncCallback` and the frame-timeline getters
/// (API 33). They add what the plain frame callback lacks: the time by which
/// SurfaceFlinger must have a frame to show it at the next vsync.
#[derive(Clone, Copy)]
struct VsyncDataApi {
    post_vsync_callback: unsafe extern "C" fn(*mut libc::c_void, VsyncCallback, *mut libc::c_void),
    frame_time: unsafe extern "C" fn(*const libc::c_void) -> i64,
    preferred_index: unsafe extern "C" fn(*const libc::c_void) -> usize,
    deadline: unsafe extern "C" fn(*const libc::c_void, usize) -> i64,
    expected_present: Option<unsafe extern "C" fn(*const libc::c_void, usize) -> i64>,
}

#[derive(Clone, Copy)]
struct ChoreoApi {
    _lib: *mut libc::c_void,
    looper_prepare: unsafe extern "C" fn(i32) -> *mut libc::c_void,
    looper_poll_once: unsafe extern "C" fn(i32, *mut i32, *mut i32, *mut *mut libc::c_void) -> i32,
    looper_release: unsafe extern "C" fn(*mut libc::c_void),
    looper_wake: unsafe extern "C" fn(*mut libc::c_void),
    get_instance: unsafe extern "C" fn() -> *mut libc::c_void,
    post_frame_callback64:
        unsafe extern "C" fn(*mut libc::c_void, FrameCallback64, *mut libc::c_void),
    vsync_data: Option<VsyncDataApi>,
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
        let lib = libc::dlopen(c"libandroid.so".as_ptr(), libc::RTLD_NOW);
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
        let vsync_data = (|| {
            Some(VsyncDataApi {
                post_vsync_callback: std::mem::transmute::<
                    *mut libc::c_void,
                    unsafe extern "C" fn(*mut libc::c_void, VsyncCallback, *mut libc::c_void),
                >(sym(
                    lib,
                    b"AChoreographer_postVsyncCallback\0",
                )?),
                frame_time: std::mem::transmute::<
                    *mut libc::c_void,
                    unsafe extern "C" fn(*const libc::c_void) -> i64,
                >(sym(
                    lib,
                    b"AChoreographerFrameCallbackData_getFrameTimeNanos\0",
                )?),
                preferred_index: std::mem::transmute::<
                    *mut libc::c_void,
                    unsafe extern "C" fn(*const libc::c_void) -> usize,
                >(sym(
                    lib,
                    b"AChoreographerFrameCallbackData_getPreferredFrameTimelineIndex\0",
                )?),
                deadline: std::mem::transmute::<
                    *mut libc::c_void,
                    unsafe extern "C" fn(*const libc::c_void, usize) -> i64,
                >(sym(
                    lib,
                    b"AChoreographerFrameCallbackData_getFrameTimelineDeadlineNanos\0",
                )?),
                expected_present: sym(
                    lib,
                    b"AChoreographerFrameCallbackData_getFrameTimelineExpectedPresentationTimeNanos\0",
                )
                .map(|p| {
                    std::mem::transmute::<
                        *mut libc::c_void,
                        unsafe extern "C" fn(*const libc::c_void, usize) -> i64,
                    >(p)
                }),
            })
        })();
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
            looper_wake: load!(b"ALooper_wake\0", unsafe extern "C" fn(*mut libc::c_void)),
            get_instance: load!(
                b"AChoreographer_getInstance\0",
                unsafe extern "C" fn() -> *mut libc::c_void
            ),
            post_frame_callback64: load!(
                b"AChoreographer_postFrameCallback64\0",
                unsafe extern "C" fn(*mut libc::c_void, FrameCallback64, *mut libc::c_void)
            ),
            vsync_data,
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
    pending: bool,
    armed: Arc<AtomicBool>,
    vsync_ns: Arc<AtomicU64>,
    deadline_ns: Arc<AtomicU64>,
    expected_present_ns: Arc<AtomicU64>,
    observed_period_ns: Arc<AtomicU64>,
    prev_vsync_ns: u64,
    prev_dt_ns: u64,
    vsync_data: Option<VsyncDataApi>,
}

/// Vsync callback with frame timelines: records the latch deadline and expected
/// presentation time, then ticks like the plain frame callback.
unsafe extern "C" fn vsync_trampoline(callback_data: *const libc::c_void, data: *mut libc::c_void) {
    if data.is_null() || callback_data.is_null() {
        return;
    }
    let holder = unsafe { &mut *(data as *mut TickHolder) };
    let Some(api) = holder.vsync_data else {
        return;
    };
    let (when_ns, deadline_ns, expected_present_ns) = unsafe {
        let index = (api.preferred_index)(callback_data);
        (
            (api.frame_time)(callback_data),
            (api.deadline)(callback_data, index),
            api.expected_present.map_or(0, |f| f(callback_data, index)),
        )
    };
    holder
        .deadline_ns
        .store(deadline_ns.max(0) as u64, Ordering::Release);
    holder
        .expected_present_ns
        .store(expected_present_ns.max(0) as u64, Ordering::Release);
    unsafe { frame_trampoline(when_ns, data) };
}

unsafe extern "C" fn frame_trampoline(when_ns: i64, data: *mut libc::c_void) {
    if data.is_null() {
        return;
    }
    let holder = unsafe { &mut *(data as *mut TickHolder) };
    holder.pending = false;
    if !holder.armed.load(Ordering::Acquire) {
        holder.prev_vsync_ns = 0;
        holder.prev_dt_ns = 0;
        return;
    }
    holder.count += 1;
    let vsync = when_ns.max(0) as u64;
    if vsync != 0 {
        if holder.prev_vsync_ns != 0 && vsync > holder.prev_vsync_ns {
            let dt = vsync - holder.prev_vsync_ns;
            // Plausible display refresh period: 20 Hz (50 ms) to 250 Hz (4 ms).
            if (4_000_000..=50_000_000).contains(&dt) {
                if holder.prev_dt_ns != 0
                    && dt.abs_diff(holder.prev_dt_ns).saturating_mul(8) <= dt.max(holder.prev_dt_ns)
                {
                    holder
                        .observed_period_ns
                        .store((dt + holder.prev_dt_ns) / 2, Ordering::Release);
                }
                holder.prev_dt_ns = dt;
            } else {
                holder.prev_dt_ns = 0;
            }
        }
        holder.prev_vsync_ns = vsync;
    }
    holder.vsync_ns.store(vsync, Ordering::Release);
    let one: u64 = 1;
    unsafe {
        libc::write(holder.tick_fd, &one as *const u64 as *const libc::c_void, 8);
    }
}

fn pump_thread_choreo(
    api: ChoreoApi,
    tick_fd: libc::c_int,
    running: Arc<AtomicBool>,
    gate: VsyncGate,
) {
    #[cfg(target_os = "android")]
    crate::android::utils::frame_pacing::prioritize_current_render_thread();
    unsafe {
        let looper = (api.looper_prepare)(1); // ALOOPER_PREPARE_ALLOW_NON_CALLBACKS
        if looper.is_null() {
            log::warn!("anland.vsync looper_prepare failed; timer fallback");
            pump_thread_timer(tick_fd, running, 16_666_666, gate.vsync_ns.clone());
            return;
        }
        *gate.looper.lock().unwrap() = looper as usize;
        let choreo = (api.get_instance)();
        if choreo.is_null() {
            *gate.looper.lock().unwrap() = 0;
            log::warn!("anland.vsync choreographer instance null; timer fallback");
            (api.looper_release)(looper);
            pump_thread_timer(tick_fd, running, 16_666_666, gate.vsync_ns.clone());
            return;
        }
        // Holder owned by this thread; the callback only ever runs here.
        let holder = Box::new(TickHolder {
            tick_fd,
            count: 0,
            pending: false,
            armed: gate.armed.clone(),
            vsync_ns: gate.vsync_ns.clone(),
            deadline_ns: gate.deadline_ns.clone(),
            expected_present_ns: gate.expected_present_ns.clone(),
            observed_period_ns: gate.observed_period_ns.clone(),
            prev_vsync_ns: 0,
            prev_dt_ns: 0,
            vsync_data: api.vsync_data,
        });
        let holder_ptr = Box::into_raw(holder) as *mut libc::c_void;
        let mut last_count: u64 = 0;
        let mut last_log_ns = now_ns();
        while running.load(Ordering::Acquire) {
            // Disarmed (nothing to present): no callback, so the display's
            // vsync stops waking this thread and the render loop. Arming
            // wakes the looper, and the next vsync ticks again.
            // ALooper may return for a wake, another fd, or a timeout before
            // the callback fires. Posting again in that case accumulates
            // callbacks, eventually writing multiple ticks on every vsync.
            let holder = &mut *(holder_ptr as *mut TickHolder);
            if gate.armed.load(Ordering::Acquire) && !holder.pending {
                holder.pending = true;
                match api.vsync_data {
                    Some(vsync) => {
                        (vsync.post_vsync_callback)(choreo, vsync_trampoline, holder_ptr)
                    }
                    None => (api.post_frame_callback64)(choreo, frame_trampoline, holder_ptr),
                }
            }
            // Returns on callback dispatch or wake, or after 250ms so stop()
            // stays bounded.
            let r = (api.looper_poll_once)(
                250,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            // ALOOPER_POLL_WAKE=-1, CALLBACK=-2, TIMEOUT=-3, ERROR=-4.
            let now = now_ns();
            if now.wrapping_sub(last_log_ns) >= 5_000_000_000 {
                let total = (*(holder_ptr as *mut TickHolder)).count;
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
        // Serialize with set_armed: no thread may wake this looper after
        // its last reference is released.
        *gate.looper.lock().unwrap() = 0;
        let _ = Box::from_raw(holder_ptr as *mut TickHolder);
        (api.looper_release)(looper);
    }
}

fn pump_thread_timer(
    tick_fd: libc::c_int,
    running: Arc<AtomicBool>,
    period_ns: u64,
    vsync_ns: Arc<AtomicU64>,
) {
    // Absolute CLOCK_MONOTONIC deadlines: one tick per period with no drift
    // (the old 50ms sleep step only ever ticked at 20 Hz).
    let mut ticks: u64 = 0;
    let mut last_log_ns = now_ns();
    let mut deadline_ns = now_ns();
    while running.load(Ordering::Acquire) {
        deadline_ns = deadline_ns.wrapping_add(period_ns);
        let now = now_ns();
        if deadline_ns < now {
            // Fell behind (freezer, suspend): resync instead of bursting.
            deadline_ns = now.wrapping_add(period_ns);
        }
        let ts = libc::timespec {
            tv_sec: (deadline_ns / 1_000_000_000) as libc::time_t,
            tv_nsec: (deadline_ns % 1_000_000_000) as libc::c_long,
        };
        unsafe {
            libc::clock_nanosleep(
                libc::CLOCK_MONOTONIC,
                libc::TIMER_ABSTIME,
                &ts,
                std::ptr::null_mut(),
            );
        }
        ticks += 1;
        vsync_ns.store(deadline_ns, Ordering::Release);
        let one: u64 = 1;
        unsafe {
            libc::write(tick_fd, &one as *const u64 as *const libc::c_void, 8);
        }
        let now = now_ns();
        if now.wrapping_sub(last_log_ns) >= 5_000_000_000 {
            log::info!("anland.vsync alive ticks_5s={ticks} mode=timer");
            ticks = 0;
            last_log_ns = now;
        }
    }
}

/// External surfaces have their own cadence. A kernel timer can wake the
/// render loop directly, without a second thread or handset-frame deadlines.
struct DisplayTimer {
    fd: Arc<OwnedFd>,
    period_ns: AtomicU64,
}

impl DisplayTimer {
    fn arm(&self, armed: bool) -> io::Result<()> {
        let ns = if armed {
            self.period_ns.load(Ordering::Relaxed)
        } else {
            0
        };
        let period = libc::timespec {
            tv_sec: (ns / 1_000_000_000) as libc::time_t,
            tv_nsec: (ns % 1_000_000_000) as libc::c_long,
        };
        let spec = libc::itimerspec {
            it_interval: period,
            it_value: period,
        };
        let result =
            unsafe { libc::timerfd_settime(self.fd.as_raw_fd(), 0, &spec, std::ptr::null_mut()) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

/// Arms and disarms a [`VsyncPump`]'s display ticks from another thread.
#[derive(Clone)]
pub struct VsyncGate {
    armed: Arc<AtomicBool>,
    /// The Choreographer pump's `ALooper` (0 before it exists).
    looper: Arc<Mutex<usize>>,
    timer: Option<Arc<DisplayTimer>>,
    /// `CLOCK_MONOTONIC` time of the latest tick's vsync (0 before the first).
    vsync_ns: Arc<AtomicU64>,
    /// A time at which SurfaceFlinger latches frames, from the latest tick
    /// (0 when the platform does not report frame timelines).
    deadline_ns: Arc<AtomicU64>,
    /// Expected display presentation time for the latest tick's preferred
    /// frame timeline (0 when unavailable).
    expected_present_ns: Arc<AtomicU64>,
    /// Live vsync spacing observed from consecutive Choreographer ticks (0
    /// while disarmed or before two consistent ticks arrive).
    observed_period_ns: Arc<AtomicU64>,
}

impl VsyncGate {
    fn new() -> Self {
        Self {
            armed: Arc::new(AtomicBool::new(true)),
            looper: Arc::new(Mutex::new(0)),
            timer: None,
            vsync_ns: Arc::new(AtomicU64::new(0)),
            deadline_ns: Arc::new(AtomicU64::new(0)),
            expected_present_ns: Arc::new(AtomicU64::new(0)),
            observed_period_ns: Arc::new(AtomicU64::new(0)),
        }
    }

    /// One of SurfaceFlinger's latch times: a frame queued, with its fence
    /// signalled, by this time or a whole number of display periods from it
    /// is shown at the following vsync. 0 when unknown.
    pub fn deadline_ns(&self) -> u64 {
        self.deadline_ns.load(Ordering::Acquire)
    }

    /// Expected display presentation time for the preferred Choreographer
    /// timeline (0 when unavailable).
    pub fn expected_present_ns(&self) -> u64 {
        self.expected_present_ns.load(Ordering::Acquire)
    }

    /// Live display vsync period observed from Choreographer, or `fallback_ns`
    /// when not yet established (or on external display timers).
    pub fn live_period_ns(&self, fallback_ns: u64) -> u64 {
        let observed = self.observed_period_ns.load(Ordering::Acquire);
        if observed != 0 {
            observed
        } else {
            fallback_ns
        }
    }

    /// The vsync of the latest tick (Choreographer frame time).
    pub fn vsync_ns(&self) -> u64 {
        self.vsync_ns.load(Ordering::Acquire)
    }

    /// Request display ticks (`true`), or let the pump sleep until re-armed.
    pub fn set_armed(&self, armed: bool) {
        if self.armed.swap(armed, Ordering::AcqRel) == armed {
            return;
        }
        if let Some(timer) = &self.timer {
            if let Err(error) = timer.arm(armed) {
                log::warn!("anland.vsync could not change display timer state: {error}");
            }
            return;
        }
        if !armed {
            self.observed_period_ns.store(0, Ordering::Release);
            return;
        }
        let looper = self.looper.lock().unwrap();
        if *looper != 0 {
            if let Some(api) = choreo_api() {
                // Thread-safe; the looper lives until the pump clears it.
                unsafe { (api.looper_wake)(*looper as *mut libc::c_void) };
            }
        }
    }

    pub fn set_refresh_millihz(&self, refresh_mhz: u32) {
        if let Some(timer) = &self.timer {
            let period = refresh_period_ns(refresh_mhz);
            if timer.period_ns.swap(period, Ordering::Relaxed) != period
                && self.armed.load(Ordering::Acquire)
            {
                if let Err(error) = timer.arm(true) {
                    log::warn!("anland.vsync could not retime display timer: {error}");
                }
            }
        }
    }
}

fn refresh_period_ns(refresh_mhz: u32) -> u64 {
    1_000_000_000_000
        / u64::from(if refresh_mhz == 0 {
            60_000
        } else {
            refresh_mhz
        })
}

/// Display ticks polled by the render loop alongside its damage and input
/// wakeups. The built-in display uses Choreographer's real frame timelines.
/// External displays use their own mode cadence and BufferQueue fences.
pub struct VsyncPump {
    tick: Arc<OwnedFd>,
    gate: VsyncGate,
    running: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    mode: &'static str,
    period_ns: u64,
}

impl VsyncPump {
    pub fn start(refresh_mhz: u32, external: bool) -> io::Result<Self> {
        let period_ns = refresh_period_ns(refresh_mhz);
        if external {
            match Self::start_display_timer(period_ns) {
                Ok(pump) => return Ok(pump),
                Err(error) => log::warn!("anland.vsync display timer unavailable: {error}"),
            }
        }
        let tick = Arc::new(make_eventfd()?);
        let running = Arc::new(AtomicBool::new(true));
        let gate = VsyncGate::new();
        let tick_fd = tick.as_raw_fd();
        let (mode, handle) = match choreo_api() {
            Some(api) => {
                let api = *api;
                let running = running.clone();
                let gate = gate.clone();
                let h = std::thread::Builder::new()
                    .name("anland-vsync".into())
                    .spawn(move || pump_thread_choreo(api, tick_fd, running, gate))
                    .map_err(|e| io::Error::other(e.to_string()))?;
                ("choreographer", h)
            }
            None => {
                let running = running.clone();
                let vsync_ns = gate.vsync_ns.clone();
                let h = std::thread::Builder::new()
                    .name("anland-vsync-timer".into())
                    .spawn(move || pump_thread_timer(tick_fd, running, period_ns, vsync_ns))
                    .map_err(|e| io::Error::other(e.to_string()))?;
                ("timer", h)
            }
        };
        log::info!("anland.vsync mode={mode} period_ns={period_ns}");
        Ok(Self {
            tick,
            gate,
            running,
            handle: Some(handle),
            mode,
            period_ns,
        })
    }

    fn start_display_timer(period_ns: u64) -> io::Result<Self> {
        let fd = unsafe {
            libc::timerfd_create(
                libc::CLOCK_MONOTONIC,
                libc::TFD_CLOEXEC | libc::TFD_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: timerfd_create returned a fresh descriptor.
        let tick = Arc::new(unsafe { OwnedFd::from_raw_fd(fd) });
        let timer = Arc::new(DisplayTimer {
            fd: tick.clone(),
            period_ns: AtomicU64::new(period_ns),
        });
        timer.arm(true)?;
        let gate = VsyncGate {
            timer: Some(timer),
            ..VsyncGate::new()
        };
        let mode = "external-display-timer";
        log::info!("anland.vsync mode={mode} period_ns={period_ns}");
        Ok(Self {
            tick,
            gate,
            running: Arc::new(AtomicBool::new(true)),
            handle: None,
            mode,
            period_ns,
        })
    }

    pub fn tick_fd(&self) -> &OwnedFd {
        &self.tick
    }

    pub fn gate(&self) -> VsyncGate {
        self.gate.clone()
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

/// Whether the render thread should wait for this vsync's input batch before
/// it selects: input is streaming (the previous vsync's batch reached KWin
/// after that vsync) and this vsync's batch has not arrived yet.
pub fn awaiting_input_batch(last_flush_ns: u64, vsync_ns: u64, period_ns: u64) -> bool {
    vsync_ns != 0
        && last_flush_ns < vsync_ns
        && last_flush_ns >= vsync_ns.saturating_sub(period_ns.saturating_mul(3) / 2)
}

/// The vsync a frame selected at `now_ns` starts from: the latest one the
/// pump reported, or the next one once it is due within `due_ns`. A slot's
/// release fence can hold the render thread until the next hardware vsync,
/// which comes just before the pump's tick (and that vsync's input batch).
/// None when the latest report is too old to extrapolate from.
pub fn current_vsync(latest_ns: u64, now_ns: u64, period_ns: u64, due_ns: u64) -> Option<u64> {
    if latest_ns == 0
        || period_ns == 0
        || now_ns > latest_ns.saturating_add(period_ns.saturating_mul(2))
    {
        return None;
    }
    let next = latest_ns + period_ns;
    Some(if now_ns.saturating_add(due_ns) >= next {
        next
    } else {
        latest_ns
    })
}

/// [`poll_readable`] with a nanosecond timeout, for sub-millisecond waits.
pub fn poll_readable_ns(fd: &OwnedFd, timeout_ns: u64) -> io::Result<bool> {
    let mut pfd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let ts = libc::timespec {
        tv_sec: (timeout_ns / 1_000_000_000) as libc::time_t,
        tv_nsec: (timeout_ns % 1_000_000_000) as libc::c_long,
    };
    let r = unsafe { libc::ppoll(&mut pfd, 1, &ts, std::ptr::null()) };
    if r < 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(err);
    }
    Ok(r > 0 && pfd.revents & libc::POLLIN != 0)
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

/// When a sync-file fence signalled (`CLOCK_MONOTONIC` ns), or `None` while
/// it is pending or when the kernel does not say. Does not take ownership.
pub fn fence_signal_ns(fence_fd: libc::c_int) -> Option<u64> {
    #[repr(C)]
    struct SyncFenceInfo {
        obj_name: [u8; 32],
        driver_name: [u8; 32],
        status: i32,
        flags: u32,
        timestamp_ns: u64,
    }
    #[repr(C)]
    struct SyncFileInfo {
        name: [u8; 32],
        status: i32,
        flags: u32,
        num_fences: u32,
        pad: u32,
        sync_fence_info: u64,
    }
    // _IOWR('>', 4, struct sync_file_info)
    const SYNC_IOC_FILE_INFO: u32 = 0xC038_3E04;
    const MAX_FENCES: usize = 4;
    let mut fences: [SyncFenceInfo; MAX_FENCES] = unsafe { std::mem::zeroed() };
    let mut info: SyncFileInfo = unsafe { std::mem::zeroed() };
    info.num_fences = MAX_FENCES as u32;
    info.sync_fence_info = fences.as_mut_ptr() as u64;
    let r = unsafe { libc::ioctl(fence_fd, SYNC_IOC_FILE_INFO as _, &mut info) };
    if r != 0 || info.status != 1 || info.num_fences as usize > MAX_FENCES {
        return None;
    }
    fences[..info.num_fences as usize]
        .iter()
        .map(|f| f.timestamp_ns)
        .max()
        .filter(|&t| t != 0)
}

/// Whether SurfaceFlinger holds a frame more than it shows, judged from the
/// buffer a frame start just dequeued at a display tick.
///
/// SurfaceFlinger shows a layer's frames in order, one per vsync. It takes a
/// frame whose GPU work is still running only when no other layer updates in
/// the same commit; otherwise the frame waits a vsync, and every later frame
/// of the gesture queues behind it. Nothing at queue time tells the two
/// apart. The buffers do: with four producer slots and no backlog, two are
/// free at a tick and the queue hands out the older, released a whole period
/// ago. With a frame waiting, the only free buffer is the one released at
/// this vsync. `release_age_ns` is how long ago the dequeued buffer's
/// release fence signalled (`Some(0)` while it has not).
pub fn backlogged(release_age_ns: Option<u64>, period_ns: u64) -> bool {
    release_age_ns.is_some_and(|age| age < period_ns / 2)
}

/// SurfaceFlinger needs a frame a little before its latch time: the buffer
/// reaches it in a binder transaction.
const LATCH_MARGIN_NS: u64 = 300_000;
/// Minimum lead time HWC/SurfaceFlinger needs between GPU completion and
/// display scanout.
const HWC_MIN_GPU_MARGIN_NS: u64 = 1_500_000;
/// Frames per bucket of the frame-time estimate, which is the worst of the
/// current and the previous bucket.
const FRAME_TIME_BUCKET: u32 = 8;
/// Number of recent queued frames tracked until their GPU fence and actual
/// presentation timestamp are observed.
const MAX_TRACKED_FRAMES: usize = 16;

/// The first slot on the `anchor_ns` lattice (step `period_ns`) at or after
/// `ready_ns + margin_ns`. `None` when either anchor or period is unknown.
pub fn slot_after(ready_ns: u64, margin_ns: u64, anchor_ns: u64, period_ns: u64) -> Option<u64> {
    if anchor_ns == 0 || period_ns == 0 {
        return None;
    }
    let t = ready_ns.saturating_add(margin_ns);
    Some(if t <= anchor_ns {
        anchor_ns - (anchor_ns - t) / period_ns * period_ns
    } else {
        anchor_ns + (t - anchor_ns).div_ceil(period_ns) * period_ns
    })
}

/// The first latch time at or after `queued_ns` (plus the margin), given any
/// one latch time and the display period. `None` when either is unknown.
pub fn latch_after(queued_ns: u64, deadline_ns: u64, period_ns: u64) -> Option<u64> {
    slot_after(queued_ns, LATCH_MARGIN_NS, deadline_ns, period_ns)
}

#[derive(Clone, Copy, Debug)]
struct TrackedFrame {
    seq: u64,
    started_ns: u64,
    buffer_ns: u64,
    queued_ns: u64,
    deadline_ns: u64,
    expected_present_ns: u64,
    /// Presentation slot (or latch slot when `expected_present_ns == 0`) that
    /// this frame is predicted or observed to occupy.
    present_slot_ns: u64,
    /// Observed GPU render-fence signal time (`CLOCK_MONOTONIC` ns), if done.
    gpu_done_ns: Option<u64>,
    /// Observed hardware presentation time (`CLOCK_MONOTONIC` ns), if known.
    actual_present_ns: Option<u64>,
}

/// Keeps the render loop to one frame per SurfaceFlinger presentation slot,
/// accounting for both CPU queue submission AND actual GPU completion and
/// display presentation timestamps.
///
/// SurfaceFlinger shows a layer's frames in order, one per vsync. A frame
/// only lands on a presentation slot when both its `queueBuffer` submission
/// meets the latch deadline AND its GPU render fence finishes in time for
/// SurfaceFlinger/HWC to scan it out. Relying on `queueBuffer` alone misses
/// multi-millisecond GPU tails (causing frames to pile up behind a slow GPU
/// frame) and misses real presentation slips reported by release fences.
///
/// A slow frame keeps the completion-time estimate up for the next 8 to 16
/// frames, so a heavy GPU load settles into a steady one-frame pipeline
/// instead of oscillating between making and missing its target vsync.
#[derive(Default)]
pub struct LatchPacer {
    /// Presentation (or latch) slot the latest queued frame occupies.
    last_present_ns: u64,
    /// Worst start-to-queued (CPU + acquire) time in current & previous bucket.
    frame_cpu_max_ns: [u64; 2],
    /// Worst buffer-to-queued (CPU work) time in current & previous bucket.
    work_cpu_max_ns: [u64; 2],
    /// Worst start-to-GPU-complete time in current & previous bucket.
    frame_complete_max_ns: [u64; 2],
    /// Worst buffer-to-GPU-complete time in current & previous bucket.
    work_complete_max_ns: [u64; 2],
    /// Worst GPU execution tail (`gpu_done_ns - queued_ns`) per bucket.
    gpu_tail_max_ns: [u64; 2],
    cpu_bucket_frames: u32,
    gpu_bucket_frames: u32,
    next_auto_seq: u64,
    present_slips: u64,
    /// Presentation slot of the most recently evicted frame from `recent`,
    /// preserving FIFO ordering across ring-buffer eviction.
    retired_last_slot_ns: u64,
    recent: std::collections::VecDeque<TrackedFrame>,
}

impl LatchPacer {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn present_slips(&self) -> u64 {
        self.present_slips
    }

    /// Estimated CPU + GPU work duration from buffer-ready to GPU completion.
    pub fn estimated_work_ns(&self) -> u64 {
        let cpu = self.work_cpu_max_ns[0].max(self.work_cpu_max_ns[1]);
        let complete = self.work_complete_max_ns[0].max(self.work_complete_max_ns[1]);
        let gpu_tail = self.estimated_gpu_tail_ns();
        complete.max(cpu.saturating_add(gpu_tail))
    }

    /// Estimated CPU + acquire + GPU duration from frame-start to GPU completion.
    pub fn estimated_frame_ns(&self) -> u64 {
        let cpu = self.frame_cpu_max_ns[0].max(self.frame_cpu_max_ns[1]);
        let complete = self.frame_complete_max_ns[0].max(self.frame_complete_max_ns[1]);
        let gpu_tail = self.estimated_gpu_tail_ns();
        complete.max(cpu.saturating_add(gpu_tail))
    }

    /// Estimated GPU execution time after `queueBuffer`.
    pub fn estimated_gpu_tail_ns(&self) -> u64 {
        self.gpu_tail_max_ns[0].max(self.gpu_tail_max_ns[1])
    }

    fn estimated_cpu_frame_ns(&self) -> u64 {
        self.frame_cpu_max_ns[0].max(self.frame_cpu_max_ns[1])
    }

    /// Estimated CPU work duration from buffer-ready to `queueBuffer`.
    pub fn estimated_cpu_work_ns(&self) -> u64 {
        self.work_cpu_max_ns[0].max(self.work_cpu_max_ns[1])
    }

    /// Latest `CLOCK_MONOTONIC` time at which the render loop may stop waiting
    /// for an input batch and still meet the earliest reachable latch and GPU
    /// completion deadlines, capped at `vsync_ns + cap_ns`.
    ///
    /// Returns `now_ns` (no wait) when `cap_ns == 0`, when `now_ns` is already
    /// past the cap, or when the remaining budget before the latch or GPU
    /// presentation cutoff cannot afford any extra wait.
    #[allow(clippy::too_many_arguments)]
    pub fn input_batch_deadline_ns(
        &self,
        vsync_ns: u64,
        now_ns: u64,
        cap_ns: u64,
        deadline_ns: u64,
        expected_present_ns: u64,
        period_ns: u64,
    ) -> u64 {
        const INPUT_WAIT_SAFETY_NS: u64 = 250_000;
        let max_wait = vsync_ns.saturating_add(cap_ns);
        if cap_ns == 0 || now_ns >= max_wait {
            return now_ns;
        }
        let Some((_, present_offset_ns, has_present)) =
            Self::timeline_params(deadline_ns, expected_present_ns, period_ns)
        else {
            return max_wait;
        };
        let Some(next_latch) = latch_after(now_ns, deadline_ns, period_ns) else {
            return max_wait;
        };
        let cpu_budget = self
            .estimated_cpu_work_ns()
            .saturating_add(LATCH_MARGIN_NS)
            .saturating_add(INPUT_WAIT_SAFETY_NS);
        let latest_for_submit = next_latch.saturating_sub(cpu_budget);

        let latest_for_gpu = if has_present {
            let target_present = next_latch.saturating_add(present_offset_ns);
            let gpu_margin = self.gpu_present_margin_ns(present_offset_ns, period_ns);
            let gpu_budget = self
                .estimated_work_ns()
                .saturating_add(gpu_margin)
                .saturating_add(INPUT_WAIT_SAFETY_NS);
            target_present.saturating_sub(gpu_budget)
        } else {
            let gpu_budget = self
                .estimated_work_ns()
                .saturating_add(LATCH_MARGIN_NS)
                .saturating_add(INPUT_WAIT_SAFETY_NS);
            next_latch.saturating_sub(gpu_budget)
        };

        let budget_deadline = latest_for_submit.min(latest_for_gpu);
        if budget_deadline <= now_ns {
            now_ns
        } else {
            max_wait.min(budget_deadline)
        }
    }

    fn timeline_params(
        deadline_ns: u64,
        expected_present_ns: u64,
        period_ns: u64,
    ) -> Option<(u64, u64, bool)> {
        if deadline_ns == 0 || period_ns == 0 {
            return None;
        }
        let has_present = expected_present_ns > deadline_ns
            && expected_present_ns - deadline_ns <= period_ns.saturating_mul(4);
        if has_present {
            Some((expected_present_ns, expected_present_ns - deadline_ns, true))
        } else {
            Some((deadline_ns, 0, false))
        }
    }

    fn gpu_present_margin_ns(&self, present_offset_ns: u64, period_ns: u64) -> u64 {
        // When Choreographer reports a multi-vsync app->present pipeline
        // (`present_offset_ns > period_ns`, e.g. 10.35 ms at 144 Hz where
        // period is 6.94 ms), SurfaceFlinger latches a vsync early while GPU
        // work runs; SurfaceFlinger/HWC's own commit window before scanout is
        // the remainder (`present_offset_ns % period_ns`, ~3.40 ms on Pad 3)
        // plus binder/fence jitter (`LATCH_MARGIN_NS`).
        if present_offset_ns > period_ns {
            (present_offset_ns % period_ns)
                .max(HWC_MIN_GPU_MARGIN_NS)
                .saturating_add(LATCH_MARGIN_NS)
        } else {
            present_offset_ns.saturating_add(LATCH_MARGIN_NS)
        }
    }

    /// Presentation slot (or latch slot when `expected_present_ns == 0`) that a
    /// frame submitted at `queued_ns` with GPU completion at `gpu_done_ns` can
    /// make.
    fn target_present_slot(
        &self,
        queued_ns: u64,
        gpu_done_ns: u64,
        deadline_ns: u64,
        expected_present_ns: u64,
        period_ns: u64,
    ) -> Option<u64> {
        let (anchor_ns, present_offset_ns, has_present) =
            Self::timeline_params(deadline_ns, expected_present_ns, period_ns)?;
        let latch_ns = latch_after(queued_ns, deadline_ns, period_ns)?;
        let from_submit = latch_ns.saturating_add(present_offset_ns);
        let effective_gpu_ns = gpu_done_ns.max(queued_ns);
        let from_gpu = if has_present {
            let gpu_margin = self.gpu_present_margin_ns(present_offset_ns, period_ns);
            slot_after(effective_gpu_ns, gpu_margin, anchor_ns, period_ns)?
        } else {
            latch_after(effective_gpu_ns, deadline_ns, period_ns)?
        };
        Some(from_submit.max(from_gpu))
    }

    fn snap_to_timeline(slot_ns: u64, anchor_ns: u64, period_ns: u64) -> Option<u64> {
        if slot_ns == 0 || anchor_ns == 0 || period_ns == 0 {
            return None;
        }
        // Round to the nearest slot on this callback's timeline so slight
        // display clock drift does not walk into the next slot.
        let before = slot_ns.saturating_sub(period_ns / 2);
        slot_after(before, 0, anchor_ns, period_ns)
    }

    fn last_present_on_timeline(
        &self,
        deadline_ns: u64,
        expected_present_ns: u64,
        period_ns: u64,
    ) -> Option<u64> {
        let (anchor_ns, _, _) = Self::timeline_params(deadline_ns, expected_present_ns, period_ns)?;
        Self::snap_to_timeline(self.last_present_ns, anchor_ns, period_ns)
    }

    fn reconcile_recent(&mut self, deadline_ns: u64, expected_present_ns: u64, period_ns: u64) {
        if self.recent.is_empty() || period_ns == 0 {
            return;
        }
        let est_tail = self.estimated_gpu_tail_ns();
        let mut prev_slot: Option<u64> =
            (self.retired_last_slot_ns != 0).then_some(self.retired_last_slot_ns);
        for idx in 0..self.recent.len() {
            let frame = self.recent[idx];
            let f_dl = if frame.deadline_ns != 0 {
                frame.deadline_ns
            } else {
                deadline_ns
            };
            let f_ep = if frame.expected_present_ns != 0 {
                frame.expected_present_ns
            } else {
                expected_present_ns
            };
            let new_slot = if let Some(actual) = frame.actual_present_ns {
                if let Some((anchor, _, _)) = Self::timeline_params(f_dl, f_ep, period_ns) {
                    Self::snap_to_timeline(actual, anchor, period_ns).unwrap_or(actual)
                } else {
                    actual
                }
            } else {
                let gpu_ns = frame
                    .gpu_done_ns
                    .unwrap_or_else(|| frame.queued_ns.saturating_add(est_tail));
                let own = self
                    .target_present_slot(frame.queued_ns, gpu_ns, f_dl, f_ep, period_ns)
                    .unwrap_or(frame.present_slot_ns);
                match prev_slot {
                    Some(prev) if prev != 0 => {
                        let aligned_prev = Self::timeline_params(f_dl, f_ep, period_ns)
                            .and_then(|(a, _, _)| Self::snap_to_timeline(prev, a, period_ns))
                            .unwrap_or(prev);
                        own.max(aligned_prev.saturating_add(period_ns))
                    }
                    _ => own,
                }
            };
            self.recent[idx].present_slot_ns = new_slot;
            if new_slot != 0 {
                prev_slot = Some(new_slot);
            }
        }
        if let Some(last) = prev_slot {
            self.last_present_ns = last;
        }
    }

    /// Whether a frame started now would only queue behind the latest one,
    /// accounting for both CPU submission and GPU completion time.
    pub fn should_defer_for_timeline(
        &self,
        now_ns: u64,
        deadline_ns: u64,
        expected_present_ns: u64,
        period_ns: u64,
    ) -> bool {
        let Some(occupied) =
            self.last_present_on_timeline(deadline_ns, expected_present_ns, period_ns)
        else {
            return false;
        };
        let est_submit = now_ns.saturating_add(self.estimated_cpu_frame_ns());
        let est_gpu = now_ns.saturating_add(self.estimated_frame_ns());
        self.target_present_slot(
            est_submit,
            est_gpu,
            deadline_ns,
            expected_present_ns,
            period_ns,
        )
        .is_some_and(|slot| slot <= occupied)
    }

    /// Convenience wrapper when only the latch deadline is available.
    pub fn should_defer(&self, now_ns: u64, deadline_ns: u64, period_ns: u64) -> bool {
        self.should_defer_for_timeline(now_ns, deadline_ns, 0, period_ns)
    }

    /// Whether a frame with its buffer ready now is expected to complete CPU
    /// and GPU work in time for the earliest reachable presentation slot.
    pub fn expects_on_time_for_timeline(
        &self,
        now_ns: u64,
        deadline_ns: u64,
        expected_present_ns: u64,
        period_ns: u64,
    ) -> bool {
        let Some(earliest) =
            self.target_present_slot(now_ns, now_ns, deadline_ns, expected_present_ns, period_ns)
        else {
            return false;
        };
        let est_submit = now_ns.saturating_add(self.estimated_cpu_work_ns());
        let est_gpu = now_ns.saturating_add(self.estimated_work_ns());
        self.target_present_slot(
            est_submit,
            est_gpu,
            deadline_ns,
            expected_present_ns,
            period_ns,
        ) == Some(earliest)
    }

    /// Convenience wrapper when only the latch deadline is available.
    pub fn expects_on_time(&self, now_ns: u64, deadline_ns: u64, period_ns: u64) -> bool {
        self.expects_on_time_for_timeline(now_ns, deadline_ns, 0, period_ns)
    }

    /// Record a frame queued to SurfaceFlinger. When `has_gpu_fence` is true,
    /// GPU completion is estimated from recent GPU tails until
    /// [`Self::frame_gpu_completed`] reports the actual fence timestamp; when
    /// false (software/synchronous), GPU completion equals `queued_ns`.
    #[allow(clippy::too_many_arguments)]
    pub fn frame_queued_timed(
        &mut self,
        seq: u64,
        started_ns: u64,
        buffer_ns: u64,
        queued_ns: u64,
        has_gpu_fence: bool,
        deadline_ns: u64,
        expected_present_ns: u64,
        period_ns: u64,
    ) {
        if self.cpu_bucket_frames >= FRAME_TIME_BUCKET {
            self.frame_cpu_max_ns = [0, self.frame_cpu_max_ns[0]];
            self.work_cpu_max_ns = [0, self.work_cpu_max_ns[0]];
            self.cpu_bucket_frames = 0;
        }
        self.cpu_bucket_frames += 1;
        self.frame_cpu_max_ns[0] =
            self.frame_cpu_max_ns[0].max(queued_ns.saturating_sub(started_ns));
        self.work_cpu_max_ns[0] = self.work_cpu_max_ns[0].max(queued_ns.saturating_sub(buffer_ns));

        let est_gpu_ns = if has_gpu_fence {
            queued_ns.saturating_add(self.estimated_gpu_tail_ns())
        } else {
            queued_ns
        };
        let slot = self
            .target_present_slot(
                queued_ns,
                est_gpu_ns,
                deadline_ns,
                expected_present_ns,
                period_ns,
            )
            .map(|own| {
                self.last_present_on_timeline(deadline_ns, expected_present_ns, period_ns)
                    .map_or(own, |occupied| own.max(occupied.saturating_add(period_ns)))
            })
            .unwrap_or(0);
        if slot != 0 {
            self.last_present_ns = slot;
        }
        if self.recent.len() >= MAX_TRACKED_FRAMES {
            if let Some(evicted) = self.recent.pop_front() {
                if evicted.present_slot_ns != 0 {
                    self.retired_last_slot_ns =
                        self.retired_last_slot_ns.max(evicted.present_slot_ns);
                }
            }
        }
        self.recent.push_back(TrackedFrame {
            seq,
            started_ns,
            buffer_ns,
            queued_ns,
            deadline_ns,
            expected_present_ns,
            present_slot_ns: slot,
            gpu_done_ns: None,
            actual_present_ns: None,
        });
        if !has_gpu_fence {
            self.frame_gpu_completed(seq, queued_ns, deadline_ns, expected_present_ns, period_ns);
        }
    }

    /// Record a frame queued synchronously (no async GPU fence).
    pub fn frame_queued(
        &mut self,
        started_ns: u64,
        buffer_ns: u64,
        queued_ns: u64,
        deadline_ns: u64,
        period_ns: u64,
    ) {
        self.next_auto_seq = self.next_auto_seq.saturating_add(1);
        let seq = self.next_auto_seq;
        self.frame_queued_timed(
            seq,
            started_ns,
            buffer_ns,
            queued_ns,
            false,
            deadline_ns,
            0,
            period_ns,
        );
    }

    /// Record the actual GPU completion timestamp (`SYNC_IOC_FILE_INFO`) for
    /// frame `seq` and reconcile the presentation pipeline if it slipped.
    pub fn frame_gpu_completed(
        &mut self,
        seq: u64,
        gpu_done_ns: u64,
        deadline_ns: u64,
        expected_present_ns: u64,
        period_ns: u64,
    ) {
        let Some(pos) = self.recent.iter().position(|f| f.seq == seq) else {
            return;
        };
        if self.recent[pos].gpu_done_ns.is_some() {
            return;
        }
        let frame = self.recent[pos];
        let gpu_done_ns = gpu_done_ns.max(frame.queued_ns);
        self.recent[pos].gpu_done_ns = Some(gpu_done_ns);

        if self.gpu_bucket_frames >= FRAME_TIME_BUCKET {
            self.frame_complete_max_ns = [0, self.frame_complete_max_ns[0]];
            self.work_complete_max_ns = [0, self.work_complete_max_ns[0]];
            self.gpu_tail_max_ns = [0, self.gpu_tail_max_ns[0]];
            self.gpu_bucket_frames = 0;
        }
        self.gpu_bucket_frames += 1;
        self.frame_complete_max_ns[0] =
            self.frame_complete_max_ns[0].max(gpu_done_ns.saturating_sub(frame.started_ns));
        self.work_complete_max_ns[0] =
            self.work_complete_max_ns[0].max(gpu_done_ns.saturating_sub(frame.buffer_ns));
        self.gpu_tail_max_ns[0] =
            self.gpu_tail_max_ns[0].max(gpu_done_ns.saturating_sub(frame.queued_ns));

        let prev_slot = frame.present_slot_ns;
        self.reconcile_recent(deadline_ns, expected_present_ns, period_ns);
        if prev_slot != 0
            && period_ns != 0
            && self.recent[pos].present_slot_ns > prev_slot.saturating_add(period_ns / 2)
        {
            self.present_slips = self.present_slips.saturating_add(1);
        }
    }

    /// Observe that frame `seq`'s GPU fence is still unsignaled at `now_ns`:
    /// acts as a live lower bound on GPU completion so an active GPU stall
    /// updates occupied presentation slots before the fence even signals.
    pub fn note_gpu_pending(
        &mut self,
        seq: u64,
        now_ns: u64,
        deadline_ns: u64,
        expected_present_ns: u64,
        period_ns: u64,
    ) {
        let Some(pos) = self.recent.iter().position(|f| f.seq == seq) else {
            return;
        };
        let frame = self.recent[pos];
        if frame.gpu_done_ns.is_some()
            || frame.actual_present_ns.is_some()
            || now_ns <= frame.queued_ns
        {
            return;
        }
        let f_dl = if frame.deadline_ns != 0 {
            frame.deadline_ns
        } else {
            deadline_ns
        };
        let f_ep = if frame.expected_present_ns != 0 {
            frame.expected_present_ns
        } else {
            expected_present_ns
        };
        let Some(lower_slot) =
            self.target_present_slot(frame.queued_ns, now_ns, f_dl, f_ep, period_ns)
        else {
            return;
        };
        if lower_slot > frame.present_slot_ns {
            self.recent[pos].present_slot_ns = lower_slot;
            let mut prev = lower_slot;
            for idx in (pos + 1)..self.recent.len() {
                if self.recent[idx].actual_present_ns.is_none() {
                    let next_min = prev.saturating_add(period_ns);
                    if next_min > self.recent[idx].present_slot_ns {
                        self.recent[idx].present_slot_ns = next_min;
                    }
                }
                if self.recent[idx].present_slot_ns != 0 {
                    prev = self.recent[idx].present_slot_ns;
                }
            }
            self.last_present_ns = self.last_present_ns.max(prev);
        }
    }

    /// Record an observed hardware presentation timestamp for frame `seq`
    /// (e.g., from the superseded slot's release fence) and reconciles the
    /// predicted presentation timeline.
    pub fn frame_presented(
        &mut self,
        seq: u64,
        actual_present_ns: u64,
        deadline_ns: u64,
        expected_present_ns: u64,
        period_ns: u64,
    ) {
        let Some(pos) = self.recent.iter().position(|f| f.seq == seq) else {
            return;
        };
        let frame = self.recent[pos];
        // Ignore stale or out-of-order release timestamps (e.g. after idle).
        if actual_present_ns < frame.queued_ns
            || actual_present_ns.saturating_sub(frame.queued_ns) >= 500_000_000
        {
            return;
        }
        let prev_slot = frame.present_slot_ns;
        self.recent[pos].actual_present_ns = Some(actual_present_ns);
        // A late present is not evidence that SurfaceFlinger needs more GPU
        // lead time: a frame that SurfaceFlinger missed once leaves every
        // later frame queued a vsync behind it. Widening the GPU margin on
        // that made every start look late, which disabled both the defer and
        // the drain that clear such a backlog (10-27 frames a vsync late on
        // the Pad 3). The reconciled slots below let the next start defer.
        if prev_slot != 0
            && period_ns != 0
            && actual_present_ns > prev_slot.saturating_add(period_ns / 2)
        {
            self.present_slips = self.present_slips.saturating_add(1);
        }
        self.reconcile_recent(deadline_ns, expected_present_ns, period_ns);
    }
}

/// Wait (up to `timeout_ms`) for a sync-file fence fd to signal.
/// A sync file signals POLLIN when the GPU work completes.
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
mod pacing_tests {
    use super::*;

    #[test]
    fn external_clock_follows_the_mode_and_sleeps_without_damage() {
        let pump = VsyncPump::start(60_000, true).unwrap();
        assert_eq!(pump.mode(), "external-display-timer");
        let gate = pump.gate();
        let interval = || {
            let mut spec = unsafe { std::mem::zeroed::<libc::itimerspec>() };
            assert_eq!(
                unsafe { libc::timerfd_gettime(pump.tick_fd().as_raw_fd(), &mut spec) },
                0
            );
            spec.it_interval.tv_nsec
        };
        assert_eq!(interval(), 16_666_666);
        gate.set_armed(false);
        gate.set_refresh_millihz(120_000);
        assert_eq!(interval(), 0);
        assert!(!poll_readable(pump.tick_fd(), 0).unwrap());
        gate.set_armed(true);
        assert_eq!(interval(), 8_333_333);
        gate.set_refresh_millihz(59_940);
        assert_eq!(interval(), 16_683_350);
    }
    use std::cell::RefCell;

    #[test]
    fn a_message_without_fds_still_reports_its_bytes() {
        let mut pair = [0; 2];
        // SAFETY: `pair` holds the two descriptors socketpair writes.
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pair.as_mut_ptr()) },
            0
        );
        // SAFETY: socketpair returned two new descriptors owned here.
        let (tx, rx) = unsafe { (OwnedFd::from_raw_fd(pair[0]), OwnedFd::from_raw_fd(pair[1])) };
        // A skipped frame's render-done message is one byte and no fence.
        // SAFETY: writes one byte from a live buffer.
        assert_eq!(
            unsafe { libc::write(tx.as_raw_fd(), [1u8].as_ptr().cast(), 1) },
            1
        );
        let mut byte = [0u8; 1];
        let mut fds = [-1i32; 1];
        let got = recv_fds(&rx, &mut byte, &mut fds, libc::MSG_DONTWAIT).unwrap();
        assert_eq!(got, Received { bytes: 1, fds: 0 });
        assert_eq!((byte[0], fds[0]), (1, -1));
        // A rendered frame's message carries its fence.
        send_fds(&tx, &[0u8], &[rx.as_raw_fd()]).unwrap();
        let got = recv_fds(&rx, &mut byte, &mut fds, libc::MSG_DONTWAIT).unwrap();
        assert_eq!(got, Received { bytes: 1, fds: 1 });
        assert!(fds[0] >= 0);
        // SAFETY: the received descriptor is new and owned here.
        drop(unsafe { OwnedFd::from_raw_fd(fds[0]) });
    }

    #[test]
    fn latch_times_form_a_lattice_around_the_known_deadline() {
        let period = 8_333_333;
        let deadline = 1_000_000_000;
        let m = LATCH_MARGIN_NS;
        assert_eq!(latch_after(deadline - m, deadline, period), Some(deadline));
        assert_eq!(
            latch_after(deadline - m + 1, deadline, period),
            Some(deadline + period)
        );
        assert_eq!(
            latch_after(deadline - 3 * period, deadline, period),
            Some(deadline - 2 * period)
        );
        assert_eq!(latch_after(5, 0, period), None);
        assert_eq!(latch_after(5, deadline, 0), None);
    }

    #[test]
    fn a_kick_frame_does_not_share_a_latch_with_the_next_tick() {
        let period = 8_333_333;
        let tick = 1_000_000_000;
        // SurfaceFlinger latches 5.33 ms after each tick.
        let deadline = tick + 5_333_333;
        let mut pacer = LatchPacer::default();
        // Frames take 2.5 ms; one that started at the tick makes its latch.
        pacer.frame_queued(tick, tick, tick + 2_500_000, deadline, period);
        assert!(!pacer.should_defer(tick + period, deadline, period));
        pacer.frame_queued(
            tick + period,
            tick + period,
            tick + period + 2_500_000,
            deadline,
            period,
        );
        // A kick 4 ms into the next interval is queued past that latch, so
        // its frame takes the following one...
        let kick = tick + 2 * period + 4_000_000;
        assert!(!pacer.should_defer(kick, deadline, period));
        pacer.frame_queued(kick, kick, kick + 2_500_000, deadline, period);
        // ...which is the latch a frame started at the next tick would take.
        assert!(pacer.should_defer(tick + 3 * period, deadline, period));
        assert!(!pacer.should_defer(tick + 4 * period, deadline, period));
    }

    #[test]
    fn slow_first_frames_cost_one_start_not_a_gesture_of_latency() {
        let period = 8_333_333;
        let tick = 1_000_000_000;
        let deadline = tick + 5_333_333;
        let mut pacer = LatchPacer::default();
        // The first frame after idle takes 7 ms and misses its latch.
        pacer.frame_queued(tick, tick, tick + 7_000_000, deadline, period);
        // While frames are expected to take that long, each start lands on
        // the latch after the previous frame's: a steady pipeline, no skips.
        let mut t = tick + period;
        let mut skipped_at = None;
        for frame in 1..60 {
            if pacer.should_defer(t, deadline, period) {
                skipped_at = Some(frame);
                break;
            }
            pacer.frame_queued(t, t, t + 2_000_000, deadline, period);
            t += period;
        }
        // Once the slow frame has left the estimate, one start is skipped...
        let skipped_at = skipped_at.expect("the pipeline never drained");
        assert!((8..=17).contains(&skipped_at), "skipped at {skipped_at}");
        // ...and frames make their own latch again.
        t += period;
        for _ in 0..100 {
            assert!(!pacer.should_defer(t, deadline, period));
            pacer.frame_queued(t, t, t + 2_000_000, deadline, period);
            t += period;
        }
    }

    #[test]
    fn a_load_that_keeps_missing_the_latch_is_never_skipped() {
        let period = 8_333_333;
        let tick = 1_000_000_000;
        let deadline = tick + 5_333_333;
        let mut pacer = LatchPacer::default();
        let mut t = tick;
        for frame in 0..500 {
            assert!(!pacer.should_defer(t, deadline, period), "frame {frame}");
            // Every tenth frame is too slow for its latch.
            let took = if frame % 10 == 0 {
                6_000_000
            } else {
                3_000_000
            };
            pacer.frame_queued(t, t, t + took, deadline, period);
            t += period;
        }
    }

    #[test]
    fn a_faster_display_does_not_accumulate_latch_drift() {
        let nominal_period = 8_333_333;
        let actual_period = 8_300_000;
        let mut pacer = LatchPacer::default();
        for frame in 0..500 {
            let tick = 1_000_000_000 + frame * actual_period;
            let deadline = tick + 5_000_000;
            assert!(
                !pacer.should_defer(tick, deadline, nominal_period),
                "frame {frame} was deferred on an empty latch"
            );
            pacer.frame_queued(tick, tick, tick + 2_500_000, deadline, nominal_period);
        }
    }

    #[test]
    fn a_backlog_shows_in_the_age_of_the_dequeued_buffer() {
        let period = 8_333_333;
        // Released a period before this tick: SurfaceFlinger is up to date.
        assert!(!backlogged(Some(9_600_000), period));
        // Released at this vsync, or not yet: it holds a frame too many.
        assert!(backlogged(Some(1_250_000), period));
        assert!(backlogged(Some(0), period));
        // No fence, or no timestamp: unknown, so never skip a frame for it.
        assert!(!backlogged(None, period));
    }

    #[test]
    fn frames_are_expected_on_time_only_while_they_are_fast() {
        let period = 8_333_333;
        let tick = 1_000_000_000;
        let deadline = tick + 5_333_333;
        let mut pacer = LatchPacer::default();
        assert!(!pacer.expects_on_time(tick, 0, period));
        pacer.frame_queued(tick, tick, tick + 2_500_000, deadline, period);
        assert!(pacer.expects_on_time(tick + period, deadline, period));
        pacer.frame_queued(
            tick + period,
            tick + period,
            tick + period + 6_000_000,
            deadline,
            period,
        );
        assert!(!pacer.expects_on_time(tick + 2 * period, deadline, period));
        // A wait for a buffer is not work: it ends when the backlog does.
        let mut pacer = LatchPacer::default();
        pacer.frame_queued(tick, tick + 5_000_000, tick + 7_000_000, deadline, period);
        assert!(pacer.expects_on_time(tick + period, deadline, period));
    }

    #[test]
    fn frames_are_never_deferred_without_a_known_deadline() {
        let mut pacer = LatchPacer::default();
        pacer.frame_queued(0, 0, 1_000, 0, 8_333_333);
        assert!(!pacer.should_defer(1_001, 0, 8_333_333));
    }

    #[test]
    fn slow_gpu_completion_after_fast_queue_is_paced_not_ignored() {
        let period = 6_944_444; // 144 Hz
        let tick = 1_000_000_000;
        let deadline = tick + 4_500_000;
        let expected_present = tick + 2 * period;
        let mut pacer = LatchPacer::default();

        // Frame 1 submits fast on CPU (2.0 ms), well before deadline, but GPU
        // finishes at +11.5 ms (9.5 ms GPU tail), missing expected_present.
        pacer.frame_queued_timed(
            1,
            tick,
            tick,
            tick + 2_000_000,
            true,
            deadline,
            expected_present,
            period,
        );
        pacer.frame_gpu_completed(1, tick + 11_500_000, deadline, expected_present, period);
        assert!(pacer.present_slips() >= 1);
        assert!(pacer.estimated_gpu_tail_ns() >= 9_500_000);
        // Because GPU completion takes 11 ms, expects_on_time must now be false.
        assert!(!pacer.expects_on_time_for_timeline(
            tick + period,
            deadline + period,
            expected_present + period,
            period,
        ));

        // Subsequent frames speed back up (GPU finishes at +4.5 ms). Once the
        // slow GPU bucket rolls out, pacer drains one start so latency drops.
        let mut t = tick + period;
        let mut seq = 2u64;
        let mut drained_at = None;
        for frame in 1..40 {
            let dl = t + 4_500_000;
            let ep = t + 2 * period;
            if pacer.should_defer_for_timeline(t, dl, ep, period) {
                drained_at = Some(frame);
                break;
            }
            pacer.frame_queued_timed(seq, t, t, t + 2_000_000, true, dl, ep, period);
            pacer.frame_gpu_completed(seq, t + 4_500_000, dl, ep, period);
            seq += 1;
            t += period;
        }
        let drained_at = drained_at.expect("slow GPU backlog was never drained");
        assert!((8..=18).contains(&drained_at), "drained at {drained_at}");
    }

    #[test]
    fn pad3_144hz_pipeline_does_not_halve_rate_for_normal_gpu_work() {
        // Real Pad 3 144 Hz Choreographer timeline: period=6.945ms,
        // dl=+10.333ms (lattice at +3.388ms), ep=+20.680ms (lattice at +13.735ms).
        let period = 6_945_000;
        let mut pacer = LatchPacer::default();
        for i in 0..120u64 {
            let t = 1_000_000_000 + i * period;
            let dl = t + 10_333_000;
            let ep = t + 20_680_000;
            assert!(
                !pacer.should_defer_for_timeline(t + 400_000, dl, ep, period),
                "frame {i} falsely deferred at 144 Hz"
            );
            // CPU submits at +2.7 ms, GPU finishes at +6.9 ms (4.2 ms GPU tail).
            pacer.frame_queued_timed(
                i + 1,
                t + 400_000,
                t + 500_000,
                t + 2_700_000,
                true,
                dl,
                ep,
                period,
            );
            pacer.frame_gpu_completed(i + 1, t + 6_900_000, dl, ep, period);
            if i >= 2 {
                // Frame i-1 presented cleanly on its N+2 slot (+13.735ms).
                let t_prev = 1_000_000_000 + (i - 2) * period;
                pacer.frame_presented(i - 1, t_prev + 13_735_000, dl, ep, period);
            }
        }
        assert_eq!(pacer.present_slips(), 0);
    }

    #[test]
    fn input_batch_wait_respects_latch_and_gpu_deadlines() {
        let period = 6_945_000; // 144 Hz
        let vsync = 1_000_000_000;
        let dl = vsync + 10_333_000; // lattice at vsync + 3.388 ms
        let ep = vsync + 20_680_000; // lattice at vsync + 13.735 ms
        let cap = 1_500_000;

        // Without timeline, falls back to vsync + cap.
        let mut pacer = LatchPacer::default();
        assert_eq!(
            pacer.input_batch_deadline_ns(vsync, vsync + 300_000, cap, 0, 0, period),
            vsync + cap
        );

        // Light frame (CPU 1.2 ms, GPU +2.0 ms = 3.2 ms): plenty of headroom ->
        // keeps the full 1.5 ms cap (3.388 - 1.2 - 0.55 = 1.638 ms > 1.5 ms).
        pacer.frame_queued_timed(1, vsync, vsync, vsync + 1_200_000, true, dl, ep, period);
        pacer.frame_gpu_completed(1, vsync + 3_200_000, dl, ep, period);
        assert_eq!(
            pacer.input_batch_deadline_ns(vsync, vsync + 300_000, cap, dl, ep, period),
            vsync + cap
        );

        // Moderate CPU frame (1.6 ms CPU -> 3.388 - 1.6 - 0.55 = 1.238 ms):
        // shrinks wait below 1.5 ms so KWin still submits before the latch.
        pacer.frame_queued_timed(2, vsync, vsync, vsync + 1_600_000, true, dl, ep, period);
        pacer.frame_gpu_completed(2, vsync + 3_600_000, dl, ep, period);
        let shrunk = pacer.input_batch_deadline_ns(vsync, vsync + 300_000, cap, dl, ep, period);
        assert!(
            shrunk > vsync + 300_000 && shrunk < vsync + cap,
            "shrunk={}",
            shrunk - vsync
        );

        // Heavy CPU frame (2.9 ms CPU): cannot afford any wait -> returns now (0 wait).
        pacer.frame_queued_timed(3, vsync, vsync, vsync + 2_900_000, true, dl, ep, period);
        pacer.frame_gpu_completed(3, vsync + 5_000_000, dl, ep, period);
        assert_eq!(
            pacer.input_batch_deadline_ns(vsync, vsync + 400_000, cap, dl, ep, period),
            vsync + 400_000
        );

        // Fast CPU (1.0 ms) but heavy GPU (9.5 ms total): GPU deadline trims wait to 0.
        let mut gpu_heavy = LatchPacer::default();
        gpu_heavy.frame_queued_timed(1, vsync, vsync, vsync + 1_000_000, true, dl, ep, period);
        gpu_heavy.frame_gpu_completed(1, vsync + 9_500_000, dl, ep, period);
        assert_eq!(
            gpu_heavy.input_batch_deadline_ns(vsync, vsync + 400_000, cap, dl, ep, period),
            vsync + 400_000
        );
    }

    #[test]
    fn active_unsignaled_gpu_stall_updates_pacer_before_fence_signals() {
        let period = 6_944_444;
        let tick = 1_000_000_000;
        let deadline = tick + 4_500_000;
        let expected_present = tick + 2 * period;
        let mut pacer = LatchPacer::default();

        // Prime fast baseline (CPU 1.5 ms, GPU +2.5 ms = 4.0 ms total).
        pacer.frame_queued_timed(
            1,
            tick,
            tick,
            tick + 1_500_000,
            true,
            deadline,
            expected_present,
            period,
        );
        pacer.frame_gpu_completed(1, tick + 4_000_000, deadline, expected_present, period);

        // Frame 2 submits fast, but its GPU fence is still unsignaled 8 ms later.
        let t2 = tick + period;
        let dl2 = t2 + 4_500_000;
        let ep2 = t2 + 2 * period;
        pacer.frame_queued_timed(2, t2, t2, t2 + 1_500_000, true, dl2, ep2, period);
        // At tick 3 + 1 ms, frame 2 is STILL pending on GPU (now = t2 + 11.5 ms):
        // frame 2 cannot hit ep2, so frame 3 must defer rather than pile up.
        let t3 = tick + 2 * period;
        let dl3 = t3 + 4_500_000;
        let ep3 = t3 + 2 * period;
        pacer.note_gpu_pending(2, t2 + 11_500_000, dl3, ep3, period);
        assert!(pacer.should_defer_for_timeline(t3, dl3, ep3, period));
    }

    #[test]
    fn one_late_present_does_not_stop_the_pacer_from_draining() {
        // Pad 3 at 144 Hz: frames queue 1.5 ms after the vsync with their GPU
        // work done at +3.8 ms, and present 13.735 ms after the vsync. From
        // frame 10 SurfaceFlinger shows every frame a vsync late (it missed
        // one and the queue now holds an extra frame). The pacer must keep
        // judging normal frames on time and defer one start to drain it.
        let period = 6_945_000;
        let t = |i: u64| 1_000_000_000 + i * period;
        let mut pacer = LatchPacer::default();
        let mut slipped = false;
        let mut drained_at = None;
        for i in 0..40u64 {
            let (dl, ep) = (t(i) + 10_333_000, t(i) + 20_680_000);
            let now = t(i) + 400_000;
            if slipped && pacer.should_defer_for_timeline(now, dl, ep, period) {
                drained_at = Some(i);
                break;
            }
            assert!(
                pacer.expects_on_time_for_timeline(now, dl, ep, period),
                "frame {i} looked late"
            );
            let seq = i + 1;
            pacer.frame_queued_timed(
                seq,
                now,
                now + 100_000,
                t(i) + 1_500_000,
                true,
                dl,
                ep,
                period,
            );
            pacer.frame_gpu_completed(seq, t(i) + 3_800_000, dl, ep, period);
            if i >= 1 {
                // The previous frame's release fence reports its present.
                let mut present = t(i - 1) + 13_735_000;
                if i - 1 >= 10 {
                    present += period;
                    slipped = true;
                }
                pacer.frame_presented(seq - 1, present, dl, ep, period);
            }
        }
        let at = drained_at.expect("the backlog was never drained");
        assert!(at <= 13, "drained at frame {at}");
    }

    #[test]
    fn actual_presentation_timestamp_reconciles_late_surfaceflinger_present() {
        let period = 6_944_444;
        let tick = 1_000_000_000;
        let deadline = tick + 4_500_000;
        let expected_present = tick + 2 * period;
        let mut pacer = LatchPacer::default();

        // Frame 1 finishes CPU+GPU on time for expected_present...
        pacer.frame_queued_timed(
            1,
            tick,
            tick,
            tick + 1_500_000,
            true,
            deadline,
            expected_present,
            period,
        );
        pacer.frame_gpu_completed(1, tick + 4_000_000, deadline, expected_present, period);
        // ...but SurfaceFlinger actually presented it one vsync late!
        pacer.frame_presented(
            1,
            expected_present + period,
            deadline,
            expected_present,
            period,
        );
        assert_eq!(pacer.present_slips(), 1);
        // Next tick would collide with frame 1's actual late slot -> defer once.
        let t2 = tick + period;
        assert!(pacer.should_defer_for_timeline(t2, t2 + 4_500_000, t2 + 2 * period, period,));
    }

    #[test]
    fn waits_only_for_the_input_batch_of_a_streaming_vsync() {
        let period = 8_333_333;
        let vsync = 1_000_000_000;
        // The previous vsync's batch arrived after it: wait for this one.
        assert!(awaiting_input_batch(
            vsync - period + 700_000,
            vsync,
            period
        ));
        // This vsync's batch is already with KWin.
        assert!(!awaiting_input_batch(vsync + 400_000, vsync, period));
        // Input stopped frames ago, or never came: never wait.
        assert!(!awaiting_input_batch(vsync - 3 * period, vsync, period));
        assert!(!awaiting_input_batch(0, vsync, period));
        // No vsync time yet.
        assert!(!awaiting_input_batch(5, 0, period));
    }

    #[test]
    fn a_frame_starts_from_the_vsync_that_is_due() {
        let period = 6_944_444;
        let due = 1_000_000;
        let vsync = 1_000_000_000;
        // Selected right after the tick: that vsync.
        assert_eq!(
            current_vsync(vsync, vsync + 300_000, period, due),
            Some(vsync)
        );
        // Held by a release fence until the next hardware vsync, before the
        // pump reported it: the next vsync, whose input batch is coming.
        assert_eq!(
            current_vsync(vsync, vsync + period - 150_000, period, due),
            Some(vsync + period)
        );
        // Ticks stopped long ago: no grid to extrapolate.
        assert_eq!(current_vsync(vsync, vsync + 3 * period, period, due), None);
        assert_eq!(current_vsync(0, vsync, period, due), None);
    }

    #[test]
    fn continuous_input_wakes_once_and_never_shortens_an_animation() {
        let demand = AtomicU64::new(0);
        let until = now_ns() + 60_000_000_000;
        assert!(extend_demand_until(&demand, until));
        // A high-rate pointer stream grants demand; display ticks, not these
        // samples, drive the subsequent frames.
        for _ in 0..1000 {
            assert!(!extend_demand_until(&demand, until - 1));
        }
        assert_eq!(demand.load(Ordering::Acquire), until);
        // Input after an idle/disarmed period must wake immediately again.
        demand.store(now_ns() - 1, Ordering::Release);
        assert!(extend_demand_until(&demand, until));
    }

    #[test]
    fn concurrent_input_and_damage_have_one_idle_wake() {
        let demand = Arc::new(AtomicU64::new(0));
        let start = Arc::new(std::sync::Barrier::new(8));
        let until = now_ns() + 60_000_000_000;
        let workers: Vec<_> = (0..8)
            .map(|i| {
                let demand = demand.clone();
                let start = start.clone();
                std::thread::spawn(move || {
                    start.wait();
                    extend_demand_until(&demand, until + i) as usize
                })
            })
            .collect();
        assert_eq!(
            workers
                .into_iter()
                .map(|w| w.join().unwrap())
                .sum::<usize>(),
            1
        );
        assert_eq!(demand.load(Ordering::Acquire), until + 7);
    }

    struct FakeLooper {
        pending: Vec<(FrameCallback64, usize)>,
        max_pending: usize,
        polls: usize,
        running: Arc<AtomicBool>,
        gate: VsyncGate,
    }

    thread_local! {
        static LOOPER: RefCell<Option<FakeLooper>> = const { RefCell::new(None) };
    }

    unsafe extern "C" fn prepare(_: i32) -> *mut libc::c_void {
        std::ptr::dangling_mut()
    }
    unsafe extern "C" fn instance() -> *mut libc::c_void {
        std::ptr::dangling_mut()
    }
    unsafe extern "C" fn noop(_: *mut libc::c_void) {}
    unsafe extern "C" fn post(_: *mut libc::c_void, cb: FrameCallback64, data: *mut libc::c_void) {
        LOOPER.with(|state| {
            let mut state = state.borrow_mut();
            let state = state.as_mut().unwrap();
            state.pending.push((cb, data as usize));
            state.max_pending = state.max_pending.max(state.pending.len());
        });
    }
    unsafe extern "C" fn poll(_: i32, _: *mut i32, _: *mut i32, _: *mut *mut libc::c_void) -> i32 {
        let callbacks = LOOPER.with(|state| {
            let mut state = state.borrow_mut();
            let state = state.as_mut().unwrap();
            state.polls += 1;
            // Disarm while a callback is in flight, then re-arm after it
            // fires. The disarmed callback must not leave a stale tick.
            if state.polls == 2 {
                state.gate.armed.store(false, Ordering::Release);
            }
            if state.polls == 4 {
                state.gate.armed.store(true, Ordering::Release);
            }
            if state.polls == 12 {
                state.running.store(false, Ordering::Release);
            }
            if state.polls.is_multiple_of(3) {
                std::mem::take(&mut state.pending)
            } else {
                Vec::new()
            }
        });
        for (cb, data) in callbacks {
            unsafe { cb(0, data as *mut libc::c_void) };
        }
        -1 // Wakes and unrelated fd callbacks are not display frames.
    }

    #[test]
    fn looper_wakes_do_not_duplicate_vsync_callbacks() {
        let tick = make_eventfd().unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let gate = VsyncGate::new();
        LOOPER.with(|state| {
            *state.borrow_mut() = Some(FakeLooper {
                pending: Vec::new(),
                max_pending: 0,
                polls: 0,
                running: running.clone(),
                gate: gate.clone(),
            })
        });
        pump_thread_choreo(
            ChoreoApi {
                _lib: std::ptr::null_mut(),
                looper_prepare: prepare,
                looper_poll_once: poll,
                looper_release: noop,
                looper_wake: noop,
                get_instance: instance,
                post_frame_callback64: post,
                vsync_data: None,
            },
            tick.as_raw_fd(),
            running,
            gate.clone(),
        );
        let state = LOOPER.with(|state| state.borrow_mut().take().unwrap());
        assert_eq!(state.max_pending, 1, "only one callback may be outstanding");
        assert_eq!(
            eventfd_read(&tick).unwrap(),
            3,
            "no tick from the disarmed callback"
        );
        assert_eq!(
            *gate.looper.lock().unwrap(),
            0,
            "retired loopers cannot be woken"
        );
    }
}
