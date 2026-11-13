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
    vsync_data: Option<VsyncDataApi>,
}

/// Vsync callback with frame timelines: records the latch deadline, then
/// ticks like the plain frame callback.
unsafe extern "C" fn vsync_trampoline(callback_data: *const libc::c_void, data: *mut libc::c_void) {
    if data.is_null() || callback_data.is_null() {
        return;
    }
    let holder = unsafe { &mut *(data as *mut TickHolder) };
    let Some(api) = holder.vsync_data else {
        return;
    };
    let (when_ns, deadline_ns) = unsafe {
        let index = (api.preferred_index)(callback_data);
        (
            (api.frame_time)(callback_data),
            (api.deadline)(callback_data, index),
        )
    };
    holder
        .deadline_ns
        .store(deadline_ns.max(0) as u64, Ordering::Release);
    unsafe { frame_trampoline(when_ns, data) };
}

unsafe extern "C" fn frame_trampoline(when_ns: i64, data: *mut libc::c_void) {
    if data.is_null() {
        return;
    }
    let holder = unsafe { &mut *(data as *mut TickHolder) };
    holder.pending = false;
    if !holder.armed.load(Ordering::Acquire) {
        return;
    }
    holder.count += 1;
    holder
        .vsync_ns
        .store(when_ns.max(0) as u64, Ordering::Release);
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

/// Display-VSYNC tick source for demand-driven presentation.
///
/// Preferred mode is Android Choreographer on a dedicated looper thread (one
/// eventfd write per display vsync, no polling, no timers). When the NDK
/// symbols are unavailable it degrades to a nanosleep timer at the panel
/// rate. The render loop polls the tick fd alongside its kick fd, so both
/// modes share the pacing logic.
/// Arms and disarms a [`VsyncPump`]'s display ticks from another thread.
#[derive(Clone)]
pub struct VsyncGate {
    armed: Arc<AtomicBool>,
    /// The Choreographer pump's `ALooper` (0 before it exists, and always
    /// with the timer pump, which ignores the gate).
    looper: Arc<Mutex<usize>>,
    /// `CLOCK_MONOTONIC` time of the latest tick's vsync (0 before the first).
    vsync_ns: Arc<AtomicU64>,
    /// A time at which SurfaceFlinger latches frames, from the latest tick
    /// (0 when the platform does not report frame timelines).
    deadline_ns: Arc<AtomicU64>,
}

impl VsyncGate {
    fn new() -> Self {
        Self {
            armed: Arc::new(AtomicBool::new(true)),
            looper: Arc::new(Mutex::new(0)),
            vsync_ns: Arc::new(AtomicU64::new(0)),
            deadline_ns: Arc::new(AtomicU64::new(0)),
        }
    }

    /// One of SurfaceFlinger's latch times: a frame queued, with its fence
    /// signalled, by this time or a whole number of display periods from it
    /// is shown at the following vsync. 0 when unknown.
    pub fn deadline_ns(&self) -> u64 {
        self.deadline_ns.load(Ordering::Acquire)
    }

    /// The vsync of the latest tick (Choreographer frame time).
    pub fn vsync_ns(&self) -> u64 {
        self.vsync_ns.load(Ordering::Acquire)
    }

    /// Request display ticks (`true`), or let the pump sleep until re-armed.
    pub fn set_armed(&self, armed: bool) {
        if self.armed.swap(armed, Ordering::AcqRel) == armed || !armed {
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
}

pub struct VsyncPump {
    tick: OwnedFd,
    gate: VsyncGate,
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

/// Non-blocking probe: whether a sync-file fence fd has already signalled.
/// Does not take ownership of `fence_fd`.
pub fn fence_signaled(fence_fd: libc::c_int) -> bool {
    let mut pfd = libc::pollfd {
        fd: fence_fd,
        events: libc::POLLIN,
        revents: 0,
    };
    unsafe { libc::poll(&mut pfd, 1, 0) == 1 && pfd.revents & libc::POLLIN != 0 }
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
/// Frames per bucket of the frame-time estimate, which is the worst of the
/// current and the previous bucket.
const FRAME_TIME_BUCKET: u32 = 8;

/// The first latch time at or after `queued_ns` (plus the margin), given any
/// one latch time and the display period. `None` when either is unknown.
pub fn latch_after(queued_ns: u64, deadline_ns: u64, period_ns: u64) -> Option<u64> {
    if deadline_ns == 0 || period_ns == 0 {
        return None;
    }
    let t = queued_ns.saturating_add(LATCH_MARGIN_NS);
    Some(if t <= deadline_ns {
        deadline_ns - (deadline_ns - t) / period_ns * period_ns
    } else {
        deadline_ns + (t - deadline_ns).div_ceil(period_ns) * period_ns
    })
}

/// Keeps the render loop to one frame per SurfaceFlinger latch.
///
/// SurfaceFlinger takes a layer's frames in order, one per vsync, and what
/// decides the vsync is when the frame was queued (it latches a buffer whose
/// GPU work is still running). A frame queued just past a latch time is
/// shown a vsync late, and the frame after it, queued in time, waits behind
/// it: from then on every frame of the gesture is shown a vsync late. The
/// first frames after an idle spell are slow (clocks ramp up), so this
/// happened at the start of most gestures.
///
/// The pacer tracks which latch each frame takes and asks the loop to skip a
/// frame start that would only queue behind the previous frame.
///
/// A slow frame keeps the frame-time estimate up for the next 8 to 16
/// frames, so a load that cannot make the latch settles into a steady
/// one-frame pipeline instead of alternating between making and missing it.
#[derive(Default)]
pub struct LatchPacer {
    /// Latch time the latest frame takes (0 before the first).
    last_latch_ns: u64,
    /// Worst start-to-queued time in the current and previous bucket.
    frame_max_ns: [u64; 2],
    /// The same without the wait for a free buffer: the work itself.
    work_max_ns: [u64; 2],
    bucket_frames: u32,
}

impl LatchPacer {
    /// Whether a frame started now would only queue behind the latest one.
    pub fn should_defer(&self, now_ns: u64, deadline_ns: u64, period_ns: u64) -> bool {
        let estimate = self.frame_max_ns[0].max(self.frame_max_ns[1]);
        latch_after(now_ns.saturating_add(estimate), deadline_ns, period_ns)
            .is_some_and(|latch| latch <= self.last_latch_ns)
    }

    /// Whether a frame that has its buffer now is expected to be queued for
    /// the next latch. False when the deadline is unknown.
    pub fn expects_on_time(&self, now_ns: u64, deadline_ns: u64, period_ns: u64) -> bool {
        let estimate = self.work_max_ns[0].max(self.work_max_ns[1]);
        latch_after(now_ns, deadline_ns, period_ns).is_some_and(|next| {
            latch_after(now_ns.saturating_add(estimate), deadline_ns, period_ns) == Some(next)
        })
    }

    /// A frame whose start was decided at `started_ns` had its buffer at
    /// `buffer_ns` and was queued at `queued_ns`.
    pub fn frame_queued(
        &mut self,
        started_ns: u64,
        buffer_ns: u64,
        queued_ns: u64,
        deadline_ns: u64,
        period_ns: u64,
    ) {
        if self.bucket_frames >= FRAME_TIME_BUCKET {
            self.frame_max_ns = [0, self.frame_max_ns[0]];
            self.work_max_ns = [0, self.work_max_ns[0]];
            self.bucket_frames = 0;
        }
        self.bucket_frames += 1;
        self.frame_max_ns[0] = self.frame_max_ns[0].max(queued_ns.saturating_sub(started_ns));
        self.work_max_ns[0] = self.work_max_ns[0].max(queued_ns.saturating_sub(buffer_ns));
        if let Some(latch) = latch_after(queued_ns, deadline_ns, period_ns) {
            // Frames are shown in order, one per latch.
            self.last_latch_ns = latch.max(self.last_latch_ns.saturating_add(period_ns));
        }
    }
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

#[cfg(test)]
mod pacing_tests {
    use super::*;
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
