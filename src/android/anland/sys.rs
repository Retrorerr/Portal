//! Raw fd plumbing for the Anland consumer/broker (Android-only).
//!
//! Mirrors `third_party/anland/common/socket_utils.c` plus the
//! eventfd/memfd/shm setup from `display_consumer.c`. All fds are owned via
//! [`OwnedFd`]-style wrappers so fallback teardown cannot leak or double-close.

use std::io;
use std::mem;
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};

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

pub fn eventfd_write(fd: &OwnedFd, value: u64) -> io::Result<()> {
    let bytes = value.to_ne_bytes();
    send_all(fd, &bytes)
}

pub fn eventfd_read(fd: &OwnedFd) -> io::Result<u64> {
    let mut bytes = [0u8; 8];
    // Non-blocking drain: the producer may have signalled several frames.
    let n = unsafe {
        libc::recv(
            fd.as_raw_fd(),
            bytes.as_mut_ptr() as *mut libc::c_void,
            bytes.len(),
            libc::MSG_DONTWAIT,
        )
    };
    if n != 8 {
        return Err(io::Error::last_os_error());
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
