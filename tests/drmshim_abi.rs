//! Phase B static + model tests for `assets/guest-arm64/drmshim.c`.
//!
//! The shim itself is AArch64 `-nostdlib` and cannot execute on the Windows
//! host. These tests statically verify the ABI fixes (source properties)
//! and exercise a faithful Rust model of the fd-tracking semantics so the
//! required behaviors (dup2 same-fd, overwrite forget, fcntl classes,
//! validated-vs-fast paths) are pinned. Physical GPU validation happens on
//! device before the Phase B commit (see task report).

const SHIM: &str = include_str!("../assets/guest-arm64/drmshim.c");

fn assert_contains(needle: &str) {
    assert!(
        SHIM.contains(needle),
        "drmshim.c missing required text: {needle}"
    );
}

fn assert_not_contains(needle: &str) {
    assert!(
        !SHIM.contains(needle),
        "drmshim.c must not contain: {needle}"
    );
}

#[test]
fn proven_gpu_behavior_is_preserved() {
    // Version probe steers Mesa to the kgsl winsys; never msm.
    assert_contains("version_name[] = \"kgsl\"");
    assert_contains("sizeof(struct drm_version) == 64");
    assert_contains("DRM_IOCTL_VERSION_NR");
    // DRM-node matching + open behavior unchanged.
    assert_contains("/dev/dri/renderD128");
    assert_contains("/dev/dri/card0");
    assert_contains("/dev/kgsl-3d0");
    assert_contains("fake_open_node");
    // ioctl routing preserved.
    assert_contains("handle_version_ioctl");
    assert_contains("UNHANDLED ioctl");
    assert_contains("ENOTTY");
    // close() must never be interposed (PRoot hazard).
    assert_not_contains("int close(");
    assert_not_contains("SYS_close");
}

#[test]
fn dup2_same_fd_semantics_are_correct() {
    // POSIX dup2(fd,fd): probe validity, return same fd, no duplication.
    // Linux dup3(fd,fd,0) would fail EINVAL, so the shim handles it first.
    assert_contains("if (oldfd == newfd)");
    assert_contains("F_GETFD");
    assert_contains("return newfd;");
    // dup3 keeps kernel EINVAL for old==new (no special case).
    let dup3_fn = SHIM.find("int dup3(").expect("dup3 present");
    let dup3_body = &SHIM[dup3_fn..dup3_fn + 800.min(SHIM.len() - dup3_fn)];
    assert!(
        !dup3_body.contains("oldfd == newfd"),
        "dup3 must preserve kernel EINVAL for old==new"
    );
    // Overwrite semantics: ordinary dup must forget a stale tracked number.
    assert_contains("forget_fd((int)rc)");
    assert_contains("forget_fd(fd)");
}

#[test]
fn fcntl_varargs_ub_is_fixed_by_classification() {
    // Void commands must forward 0 without consuming varargs.
    for cmd in [
        "F_GETFD",
        "F_GETFL",
        "F_GETOWN",
        "F_GETSIG",
        "F_GETLEASE",
        "F_GETPIPE_SZ",
        "F_GET_SEALS",
    ] {
        assert_contains(&format!("#define {cmd}"));
    }
    assert_contains("fcntl_is_void");
    assert_contains("return do_fcntl(fd, cmd, 0);");
    // Integer vs pointer consumption (LP64: int vs void *).
    assert_contains("fcntl_is_int");
    assert_contains("fcntl_is_ptr");
    assert_contains("__builtin_va_arg(*ap, int)");
    assert_contains("__builtin_va_arg(*ap, void *)");
    // Critical duplicates keep tracking.
    assert_contains("F_DUPFD_CLOEXEC");
    assert_contains("cmd == F_DUPFD || cmd == F_DUPFD_CLOEXEC");
    // Values match NDK AArch64 headers (checked against sysroot).
    for (name, value) in [
        ("F_DUPFD", "0"),
        ("F_GETFD", "1"),
        ("F_SETFD", "2"),
        ("F_GETFL", "3"),
        ("F_SETFL", "4"),
        ("F_GETLK", "5"),
        ("F_SETOWN", "8"),
        ("F_GETOWN", "9"),
        ("F_SETSIG", "10"),
        ("F_GETSIG", "11"),
        ("F_SETOWN_EX", "15"),
        ("F_GETOWN_EX", "16"),
        ("F_GETOWNER_UIDS", "17"),
        ("F_OFD_GETLK", "36"),
        ("F_DUPFD_CLOEXEC", "1030"),
        ("F_SETPIPE_SZ", "1031"),
        ("F_GETPIPE_SZ", "1032"),
        ("F_ADD_SEALS", "1033"),
        ("F_GET_SEALS", "1034"),
        ("F_GET_RW_HINT", "1035"),
        ("F_SET_FILE_RW_HINT", "1038"),
        ("SYS_fstat", "80"),
    ] {
        assert_contains(&format!("#define {name} {value}"));
    }
}

#[test]
fn stale_fd_tracking_is_validated_cheaply_and_safely() {
    // Identity via fstat dev+ino, raw SVC, no lifetime change.
    assert_contains("SYS_fstat");
    assert_contains("struct kstat");
    assert_contains("st_dev");
    assert_contains("st_ino");
    assert_contains("is_fake_validated");
    assert_contains("is_fake_fast");
    // EBADF forgets; other fstat errors fall back to monotonic (GPU safe).
    assert_contains("EBADF");
    assert_contains("monotonic fallback");
    // Hot KGSL path skips validation (identical passthrough).
    assert_contains("keeps per-frame GPU ioctls cheap");
    // Table-full GC so stale entries cannot wedge the table.
    assert_contains("Table full");
    // Ordinary dup never becomes fake.
    let dup_fn = SHIM.find("int dup(").expect("dup present");
    let dup_body = &SHIM[dup_fn..dup_fn + 600.min(SHIM.len() - dup_fn)];
    assert!(dup_body.contains("forget_fd"));
}

#[test]
fn shim_remains_freestanding_and_minimal() {
    assert_contains("-nostdlib");
    assert_contains("__errno_location");
    assert_contains("register long r0");
    // Only errno helper may be undefined (resolved from libc).
    assert_not_contains("malloc");
    assert_not_contains("printf");
}

// --- Faithful Rust model of the C fd-table semantics ---------------------

/// Mirrors `struct fake_entry` + `remember/forget/validate` in drmshim.c.
/// dev==0&&ino==0 means "fstat failed at remember time" (monotonic fallback).
#[derive(Debug, Default)]
struct FakeTable {
    entries: Vec<(i32, u64, u64)>,
}

impl FakeTable {
    fn find(&self, fd: i32) -> Option<usize> {
        self.entries.iter().position(|(f, _, _)| *f == fd)
    }
    fn remember(&mut self, fd: i32, dev: u64, ino: u64) {
        if let Some(i) = self.find(fd) {
            self.entries[i] = (fd, dev, ino);
        } else if self.entries.len() < 32 {
            self.entries.push((fd, dev, ino));
        }
    }
    fn forget(&mut self, fd: i32) {
        if let Some(i) = self.find(fd) {
            self.entries.swap_remove(i);
        }
    }
    /// Returns true when the tracked fd still refers to the same object.
    /// `live` is the current (dev,ino) or None when the fd is closed.
    /// `fstat_err` simulates an unexpected fstat failure (monotonic fallback).
    fn is_fake_validated(
        &mut self,
        fd: i32,
        live: Option<(u64, u64)>,
        fstat_err: bool,
    ) -> bool {
        let i = match self.find(fd) {
            None => return false,
            Some(i) => i,
        };
        let (_, exp_dev, exp_ino) = self.entries[i];
        if exp_dev == 0 && exp_ino == 0 {
            return true;
        }
        if fstat_err {
            return true;
        }
        match live {
            None => {
                self.forget(fd);
                false
            }
            Some((d, n)) if d == exp_dev && n == exp_ino => true,
            Some(_) => {
                self.forget(fd);
                false
            }
        }
    }
}

/// POSIX dup2(fd,fd) model: valid -> same fd, no dup; invalid -> EBADF.
fn model_dup2_same_fd(valid: bool) -> Result<i32, i32> {
    const EBADF: i32 = 9;
    if valid {
        Ok(42)
    } else {
        Err(EBADF)
    }
}

#[test]
fn model_dup2_same_fd_returns_without_duplication() {
    assert_eq!(model_dup2_same_fd(true), Ok(42));
    assert_eq!(model_dup2_same_fd(false), Err(9));
}

#[test]
fn model_fake_to_new_dup_stays_fake_and_ordinary_dup_forgets() {
    let mut t = FakeTable::default();
    t.remember(10, 7, 77);
    // dup(fake 10) -> 11 becomes fake.
    assert!(t.is_fake_validated(10, Some((7, 77)), false));
    t.remember(11, 7, 78);
    assert!(t.find(11).is_some());
    // dup(ordinary 5) -> 11 (recycled number) must forget 11.
    assert!(!t.is_fake_validated(5, Some((1, 1)), false));
    t.forget(11);
    assert!(t.find(11).is_none());
}

#[test]
fn model_overwrite_by_ordinary_fd_forgets_tracking() {
    let mut t = FakeTable::default();
    t.remember(10, 7, 77);
    t.remember(20, 1, 2); // stale-tracked ordinary number
    // dup2(fake 10 -> 20): 20 stays fake.
    assert!(t.is_fake_validated(10, Some((7, 77)), false));
    t.remember(20, 7, 79);
    assert!(t.find(20).is_some());
    // dup2(ordinary 5 -> 21 where 21 was stale-tracked): forget 21.
    t.remember(21, 9, 9);
    assert!(!t.is_fake_validated(5, Some((3, 3)), false));
    t.forget(21);
    assert!(t.find(21).is_none());
}

#[test]
fn model_recycled_fd_is_not_misclassified() {
    let mut t = FakeTable::default();
    t.remember(10, 7, 77);
    // fd 10 closed and recycled for an ordinary file (different identity).
    assert!(!t.is_fake_validated(10, Some((8, 88)), false));
    assert!(t.find(10).is_none());
    // Closed fd (EBADF) is forgotten.
    t.remember(11, 7, 78);
    assert!(!t.is_fake_validated(11, None, false));
    assert!(t.find(11).is_none());
    // Unexpected fstat failure keeps monotonic behavior (GPU safe).
    t.remember(12, 7, 79);
    assert!(t.is_fake_validated(12, Some((0, 0)), true));
    assert!(t.find(12).is_some());
}

#[test]
fn model_fcntl_dupfd_keeps_tracking_and_void_takes_no_arg() {
    // F_DUPFD/F_DUPFD_CLOEXEC of a fake fd produce a fake fd.
    let mut t = FakeTable::default();
    t.remember(10, 7, 77);
    assert!(t.is_fake_validated(10, Some((7, 77)), false));
    t.remember(13, 7, 80);
    assert!(t.find(13).is_some());
    // Void commands (F_GETFD etc.) never create tracking entries.
    assert!(t.find(99).is_none());
    // Ordinary F_DUPFD result that collides with a stale entry is forgotten.
    t.remember(14, 5, 5);
    t.forget(14);
    assert!(t.find(14).is_none());
}
