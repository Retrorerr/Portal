//! Keep Portal's descriptors out of the helpers it starts.

use std::os::unix::process::CommandExt;
use std::process::Command;

/// `CLOSE_RANGE_CLOEXEC`, which the libc crate doesn't define for Android.
const CLOSE_RANGE_CLOEXEC: libc::c_uint = 1 << 2;

/// Mark every descriptor above stderr close-on-exec in `command`'s child.
///
/// Android hands the app its SurfaceView buffers and GPU handles without
/// close-on-exec. A helper that inherits them pins that memory for as long as
/// it runs, long after the surface is gone. Only for helpers that take nothing
/// but stdio from Portal.
pub fn close_inherited_fds(command: &mut Command) {
    // SAFETY: the closure only makes async-signal-safe system calls.
    unsafe {
        command.pre_exec(|| {
            let done = libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, CLOSE_RANGE_CLOEXEC);
            if done != 0 {
                for fd in 3..4096 {
                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                }
            }
            Ok(())
        });
    }
}
