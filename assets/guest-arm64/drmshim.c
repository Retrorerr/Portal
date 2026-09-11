/*
 * Project Anland DRM render-device shim — guest-native ARM64 glibc sources.
 *
 * KWin's Anland backend requires a DRM render device at init
 * (openRenderDevice) even for the surfaceless path, but /dev/dri/renderD128 is
 * not openable from the Android app sandbox (SELinux/DAC). Without a device,
 * KWin logs "no usable DRM render device; cannot bring up OpenGL compositing"
 * and exits.
 *
 * This shim presents the render node backed by the real /dev/kgsl-3d0 plus
 * a correct version probe so init can proceed to real GPU:
 *   - intercept open/open64/openat/openat64 of /dev/dri/renderD128 and
 *     /dev/dri/card0: return a real fd for /dev/kgsl-3d0 (/dev/null fallback
 *     where KGSL is absent), log it with its backing, remember the fd;
 *   - track dups of remembered fds (dup/dup2/dup3/fcntl/fcntl64);
 *   - intercept ioctl on remembered fds:
 *     DRM_IOCTL_VERSION -> report driver "kgsl" v1.1.0 (length query + fill);
 *     non-DRM ioctls -> pass through to the real backing device;
 *     other DRM ioctls -> log the number, fail ENOTTY.
 * Every other call passes through untouched. NOTE: close() is deliberately
 * NOT interposed. A raw-SVC close replacement breaks processes under PRoot
 * (proven by bisect: interposing only close makes cat/python fail with
 * EFAULT after successful reads; open/openat/ioctl-only variants are clean).
 *
 * fd tracking is bounded (32 entries) and validated, not monotonic:
 * because close() is not interposed, a closed fake number can be recycled
 * for an ordinary file. Before treating a tracked fd as fake on the
 * DRM-version path, the shim re-validates the live descriptor with fstat
 * (dev+ino captured at fake-open time) via a raw SVC that never changes fd
 * lifetime and is safe under PRoot. A mismatch (or EBADF) forgets the
 * entry and treats the fd as ordinary. Non-DRM ioctls (notably the hot
 * KGSL 'k' path) intentionally skip validation and pass through directly:
 * fake and non-fake handling there is byte-identical, so skipping the
 * extra fstat keeps per-frame GPU ioctls cheap while the version probe
 * (rare, security-sensitive) stays protected. If fstat itself fails with
 * an unexpected error (not EBADF, not a mismatch), the entry is kept as
 * fake (monotonic fallback) so a validation-syscall quirk can never break
 * the proven GPU path. A dup of an ordinary fd never becomes fake, and an
 * overwrite of a tracked number by an ordinary fd forgets that number.
 *
 * dup2 POSIX semantics: dup2(fd,fd) with a valid fd returns fd without
 * duplicating (Linux dup3(fd,fd,0) would fail EINVAL). Implemented via an
 * F_GETFD probe + early return; invalid fds fail EBADF without closing
 * anything. dup3 keeps kernel EINVAL for oldfd==newfd.
 *
 * fcntl ABI: fcntl(fd,cmd,...) only takes a third argument for commands
 * that require one. Unconditionally consuming varargs is UB for void
 * commands (F_GETFD/F_GETFL/F_GETOWN/F_GETSIG/F_GETLEASE/F_GETPIPE_SZ/
 * F_GET_SEALS). Commands are classified below from Linux asm-generic
 * fcntl.h + linux/fcntl.h for AArch64 (LP64): void commands forward 0
 * without touching varargs; integer commands consume `int`; pointer
 * commands consume `void *`; unknown commands consume `long` (kernel
 * ignores the third slot for unknown void commands, while int/pointer
 * unknowns forward correctly). F_DUPFD/F_DUPFD_CLOEXEC keep fake-fd
 * tracking.
 *
 * CRITICAL LAYOUT NOTE: struct drm_version is INTERLEAVED
 * (int major, minor, patch; then size_t len + pointer PER FIELD:
 * name_len/name/date_len/date/desc_len/desc = 64 bytes on LP64). An earlier
 * revision used a grouped layout (all lengths, then all pointers); the ioctl
 * number still matched (same 64-byte size) but every field after name_len was
 * misplaced, so Mesa's version query failed with garbage lengths/pointers and
 * KWin died at "Failed to create gbm device". The layout below is asserted at
 * compile time. Do not "simplify" it.
 *
 * Freestanding (-nostdlib): raw AArch64 SVC for openat/write/ioctl/dup/fcntl
 * so the shim never recurses into libc. errno is set through libc's own
 * __errno_location (resolved from the already-loaded libc at process start).
 *
 * Preloaded ONLY when ANLAND_SOCKET is set (see
 * assets/localdesktop-kwin-wrapper-v2.sh); never in QPainter mode.
 *
 * Build (Windows host, NDK clang 18):
 *   clang --target=aarch64-linux-gnu -shared -fPIC -nostdlib -O2 \
 *       -ffreestanding -fno-stack-protector -o drmshim.so drmshim.c
 *
 * Verify in-guest (lfdevs KWin + Mesa env + a broker on the socket):
 *   LD_PRELOAD=/tmp/drmshim.so /usr/bin/kwin_wayland --anland --no-lockscreen
 */
typedef unsigned long size_t;
typedef long ssize_t;

/* Correct drm/drm.h layout (LP64): 3x int + pad, then len+ptr per field. */
struct drm_version {
    int version_major;
    int version_minor;
    int version_patchlevel;
    int _pad;
    size_t name_len;
    char *name;
    size_t date_len;
    char *date;
    size_t desc_len;
    char *desc;
};
_Static_assert(sizeof(struct drm_version) == 64, "struct drm_version must be 64 bytes on LP64");
_Static_assert(sizeof(void *) == 8, "LP64 required");

/* Kernel struct stat for AArch64 (asm-generic/stat.h, LP64). Used only for
 * dev+ino identity via the fstat SVC; never exposed to libc. */
struct kstat {
    unsigned long st_dev;
    unsigned long st_ino;
    unsigned int st_mode;
    unsigned int st_nlink;
    unsigned int st_uid;
    unsigned int st_gid;
    unsigned long st_rdev;
    unsigned long __pad1;
    long st_size;
    int st_blksize;
    int __pad2;
    long st_blocks;
    long st_atime;
    unsigned long st_atime_nsec;
    long st_mtime;
    unsigned long st_mtime_nsec;
    long st_ctime;
    unsigned long st_ctime_nsec;
    unsigned int __unused4;
    unsigned int __unused5;
};

#define SYS_dup 23
#define SYS_dup3 24
#define SYS_fcntl 25
#define SYS_openat 56
#define SYS_write 64
#define SYS_ioctl 29
#define SYS_fstat 80
/* fcntl commands for AArch64 (asm-generic/fcntl.h + linux/fcntl.h, LP64).
 * Explicit because the shim is -nostdlib. Do not guess: values below match
 * the NDK sysroot headers (27.2.12479018) and Linux UAPI. */
#define F_DUPFD 0
#define F_GETFD 1
#define F_SETFD 2
#define F_GETFL 3
#define F_SETFL 4
#define F_GETLK 5
#define F_SETLK 6
#define F_SETLKW 7
#define F_SETOWN 8
#define F_GETOWN 9
#define F_SETSIG 10
#define F_GETSIG 11
#define F_SETOWN_EX 15
#define F_GETOWN_EX 16
#define F_GETOWNER_UIDS 17
#define F_OFD_GETLK 36
#define F_OFD_SETLK 37
#define F_OFD_SETLKW 38
#define F_LINUX_SPECIFIC_BASE 1024
#define F_SETLEASE 1024
#define F_GETLEASE 1025
#define F_NOTIFY 1026
#define F_CANCELLK 1029
#define F_DUPFD_CLOEXEC 1030
#define F_SETPIPE_SZ 1031
#define F_GETPIPE_SZ 1032
#define F_ADD_SEALS 1033
#define F_GET_SEALS 1034
#define F_GET_RW_HINT 1035
#define F_SET_RW_HINT 1036
#define F_GET_FILE_RW_HINT 1037
#define F_SET_FILE_RW_HINT 1038
#define AT_FDCWD (-100)
#define O_RDONLY 0
#define O_CREAT 0x40
#define O_TMPFILE 0x410000
#define EBADF 9
#define EINVAL 22
#define ENOTTY 25
#define STDERR_FD 2

#define DRM_IOCTL_NR(req) ((unsigned)((req) & 0xFFu))
#define DRM_IOCTL_TYPE(req) ((unsigned)(((req) >> 8) & 0xFFu))
#define DRM_IOCTL_VERSION_NR 0x00u
#define DRM_IOCTL_VERSION_TYPE ((unsigned)'d')

extern int *__errno_location(void);

/* All argument registers are pinned explicitly: a plain "r" constraint lets
 * the compiler pick any register, which silently misroutes syscall arguments
 * (observed as spurious EFAULT "Bad address" failures). */
static long raw_syscall6(long n, long a, long b, long c, long d, long e, long f) {
    register long r0 __asm__("x0") = a;
    register long r1 __asm__("x1") = b;
    register long r2 __asm__("x2") = c;
    register long r3 __asm__("x3") = d;
    register long r4 __asm__("x4") = e;
    register long r5 __asm__("x5") = f;
    register long r8 __asm__("x8") = n;
    __asm__ volatile("svc #0"
                     : "+r"(r0)
                     : "r"(r1), "r"(r2), "r"(r3), "r"(r4), "r"(r5), "r"(r8)
                     : "memory", "cc");
    return r0;
}

static long svc3(long n, long a, long b, long c) {
    return raw_syscall6(n, a, b, c, 0, 0, 0);
}

static long svc4(long n, long a, long b, long c, long d) {
    return raw_syscall6(n, a, b, c, d, 0, 0);
}

static int set_errno_ret(long svc_ret) {
    if (svc_ret < 0 && svc_ret > -4096) {
        *__errno_location() = (int)-svc_ret;
        return -1;
    }
    return (int)svc_ret;
}

static size_t shim_strlen(const char *s) {
    size_t n = 0;
    while (s[n])
        n++;
    return n;
}

static int shim_streq(const char *a, const char *b) {
    while (*a && *a == *b) {
        a++;
        b++;
    }
    return *a == *b;
}

static void shim_write_all(const char *s, size_t n) {
    while (n > 0) {
        long r = svc3(SYS_write, STDERR_FD, (long)s, (long)n);
        if (r <= 0)
            return;
        s += r;
        n -= (size_t)r;
    }
}

static void shim_log(const char *s) {
    shim_write_all(s, shim_strlen(s));
}

static void shim_log_ulong_hex(unsigned long v) {
    char buf[18];
    buf[0] = '0';
    buf[1] = 'x';
    for (int i = 0; i < 16; i++) {
        unsigned digit = (unsigned)((v >> (60 - 4 * i)) & 0xFu);
        buf[2 + i] = (char)(digit < 10 ? '0' + digit : 'a' + digit - 10);
    }
    shim_write_all(buf, 18);
}

static void shim_log_int(int v) {
    char buf[12];
    int neg = v < 0;
    unsigned u = neg ? (unsigned)-(v + 1) + 1u : (unsigned)v;
    int i = 11;
    do {
        buf[--i] = (char)('0' + u % 10u);
        u /= 10u;
    } while (u && i > 1);
    if (neg && i > 0)
        buf[--i] = '-';
    shim_write_all(buf + i, (size_t)(11 - i));
}

#define MAX_FAKE_FDS 32
struct fake_entry {
    int fd;
    unsigned long dev;
    unsigned long ino;
};
static struct fake_entry fake_tab[MAX_FAKE_FDS];
static int fake_count = 0;

static int find_fake(int fd) {
    for (int i = 0; i < fake_count; i++) {
        if (fake_tab[i].fd == fd)
            return i;
    }
    return -1;
}

/* Fast table lookup without syscalls: used for the hot non-DRM passthrough
 * path where fake/non-fake handling is identical. */
static int is_fake_fast(int fd) {
    return find_fake(fd) >= 0;
}

static void forget_fd(int fd) {
    int idx = find_fake(fd);
    if (idx < 0)
        return;
    fake_tab[idx] = fake_tab[--fake_count];
}

/* Validated lookup: re-checks the live descriptor's dev+ino so a recycled
 * number is never misclassified. Safe under PRoot (read-only fstat SVC,
 * no fd lifetime change). Cheap enough for the dup/version paths; the hot
 * KGSL ioctl path intentionally uses is_fake_fast (see ioctl below).
 * Fallback policy: EBADF or dev/ino mismatch -> forget, not fake.
 * Any other fstat failure -> keep as fake (never break the proven GPU
 * path on a validation-syscall quirk). Entries with dev==0&&ino==0
 * (fstat failed at remember time) stay monotonic. */
static int is_fake_validated(int fd) {
    int idx = find_fake(fd);
    if (idx < 0)
        return 0;
    unsigned long exp_dev = fake_tab[idx].dev;
    unsigned long exp_ino = fake_tab[idx].ino;
    if (exp_dev == 0 && exp_ino == 0)
        return 1;
    struct kstat st;
    long rc = raw_syscall6(SYS_fstat, (long)fd, (long)&st, 0, 0, 0, 0);
    if (rc == -(long)EBADF) {
        forget_fd(fd);
        return 0;
    }
    if (rc < 0 && rc > -4096) {
        return 1;
    }
    if (st.st_dev == exp_dev && st.st_ino == exp_ino)
        return 1;
    forget_fd(fd);
    return 0;
}

static void remember_fd(int fd) {
    int idx = find_fake(fd);
    struct kstat st;
    long rc = raw_syscall6(SYS_fstat, (long)fd, (long)&st, 0, 0, 0, 0);
    unsigned long dev = 0, ino = 0;
    if (rc >= 0 || rc <= -4096) {
        dev = st.st_dev;
        ino = st.st_ino;
    }
    if (idx >= 0) {
        fake_tab[idx].dev = dev;
        fake_tab[idx].ino = ino;
        return;
    }
    if (fake_count < MAX_FAKE_FDS) {
        fake_tab[fake_count].fd = fd;
        fake_tab[fake_count].dev = dev;
        fake_tab[fake_count].ino = ino;
        fake_count++;
        return;
    }
    /* Table full: reclaim entries whose fds are closed/recycled, then
     * retry once. Stale entries must not permanently wedge the table. */
    for (int i = 0; i < fake_count;) {
        struct kstat cur;
        long r = raw_syscall6(SYS_fstat, (long)fake_tab[i].fd, (long)&cur, 0, 0, 0, 0);
        if (r == -(long)EBADF ||
            ((r >= 0 || r <= -4096) &&
             (cur.st_dev != fake_tab[i].dev || cur.st_ino != fake_tab[i].ino) &&
             !(fake_tab[i].dev == 0 && fake_tab[i].ino == 0))) {
            fake_tab[i] = fake_tab[--fake_count];
        } else {
            i++;
        }
    }
    if (fake_count < MAX_FAKE_FDS) {
        fake_tab[fake_count].fd = fd;
        fake_tab[fake_count].dev = dev;
        fake_tab[fake_count].ino = ino;
        fake_count++;
    }
}

static int is_dri_node(const char *path) {
    return shim_streq(path, "/dev/dri/renderD128") || shim_streq(path, "/dev/dri/card0");
}

/* Present the DRM node using the real KGSL device as backing where
 * possible. Rationale (proven on-device, see recipe): Mesa's freedreno
 * fd_device_new routes by DRM version name — "msm" selects the MSM winsys
 * (whose GEM ioctls need a real MSM DRM node, absent here), anything else
 * selects the kgsl winsys, which probes the PASSED fd itself with
 * KGSL_PROP_DEVICE_INFO. A /dev/null backing fails that probe (ENOTTY) so
 * GPU init can never succeed; a real /dev/kgsl-3d0 backing answers it from
 * the kernel, and all subsequent KGSL ioctls pass through to real hardware.
 * DRM ioctls on the tracked fd keep failing ENOTTY, which callers treat as
 * "unsupported" (graceful), never as fatal. Falls back to /dev/null when
 * KGSL is absent (old behavior: version probe only). */
static int fake_open_node(const char *label, const char *path) {
    long fd = svc4(SYS_openat, AT_FDCWD, (long)"/dev/kgsl-3d0", 2 /* O_RDWR */, 0);
    const char *backing = "kgsl-3d0";
    if (fd < 0 || fd > 0x7FFFFFFF) {
        fd = svc4(SYS_openat, AT_FDCWD, (long)"/dev/null", O_RDONLY, 0);
        backing = "/dev/null";
    }
    if (fd < 0 || fd > 0x7FFFFFFF)
        return -1;
    remember_fd((int)fd);
    shim_log("drmshim: faked ");
    shim_log(label);
    shim_log("(");
    shim_log(path);
    shim_log(") -> fd ");
    shim_log_int((int)fd);
    shim_log(" backing=");
    shim_log(backing);
    shim_log("\n");
    return (int)fd;
}

int open(const char *path, int flags, ...) {
    int mode = 0;
    if (flags & (O_CREAT | O_TMPFILE)) {
        __builtin_va_list ap;
        __builtin_va_start(ap, flags);
        mode = __builtin_va_arg(ap, int);
        __builtin_va_end(ap);
    }
    if (is_dri_node(path)) {
        int fd = fake_open_node("open", path);
        if (fd >= 0)
            return fd;
    }
    return set_errno_ret(svc4(SYS_openat, AT_FDCWD, (long)path, (long)flags, (long)mode));
}

int open64(const char *path, int flags, ...) {
    int mode = 0;
    if (flags & (O_CREAT | O_TMPFILE)) {
        __builtin_va_list ap;
        __builtin_va_start(ap, flags);
        mode = __builtin_va_arg(ap, int);
        __builtin_va_end(ap);
    }
    if (is_dri_node(path)) {
        int fd = fake_open_node("open64", path);
        if (fd >= 0)
            return fd;
    }
    return set_errno_ret(svc4(SYS_openat, AT_FDCWD, (long)path, (long)flags, (long)mode));
}

int openat(int dirfd, const char *path, int flags, ...) {
    int mode = 0;
    if (flags & (O_CREAT | O_TMPFILE)) {
        __builtin_va_list ap;
        __builtin_va_start(ap, flags);
        mode = __builtin_va_arg(ap, int);
        __builtin_va_end(ap);
    }
    if (dirfd == AT_FDCWD && is_dri_node(path)) {
        int fd = fake_open_node("openat", path);
        if (fd >= 0)
            return fd;
    }
    return set_errno_ret(svc4(SYS_openat, (long)dirfd, (long)path, (long)flags, (long)mode));
}

/* openat64 is a separate symbol (and the one some callers bind to); same
 * policy as openat. Without it, DRM-node opens via openat64 bypass the
 * fake entirely (proven: untracked /dev/null fd receiving a version query
 * and dying with ENOTTY).
 *
 * The __*_nocancel variants are glibc's internal aliases (used by fopen
 * and other libc-internal paths); they bypass the public symbols, so a
 * DRM-node open through them would likewise escape the fake. Same policy
 * here: passthrough is behavior-identical, faking only covers the two
 * DRM nodes, and the wrappers use raw SVCs (no libc calls, no recursion)
 * so they are safe at any init stage. */
int openat64(int dirfd, const char *path, int flags, ...) {
    int mode = 0;
    if (flags & (O_CREAT | O_TMPFILE)) {
        __builtin_va_list ap;
        __builtin_va_start(ap, flags);
        mode = __builtin_va_arg(ap, int);
        __builtin_va_end(ap);
    }
    if (dirfd == AT_FDCWD && is_dri_node(path)) {
        int fd = fake_open_node("openat64", path);
        if (fd >= 0)
            return fd;
    }
    return set_errno_ret(svc4(SYS_openat, (long)dirfd, (long)path, (long)flags, (long)mode));
}

/* Mesa's loader dups the render node (fcntl F_DUPFD_CLOEXEC) and queries
 * the VERSION on the copy. A dup of a fake fd must stay fake, otherwise the
 * version query bypasses the shim, hits the real /dev/null backing and
 * dies with ENOTTY ("cannot get version"). dup/dup2/dup3/fcntl are pure
 * fd-table operations (no lifecycle effects like close), so interposing
 * them is safe. A dup of an ordinary fd must never become fake, and an
 * overwrite of a tracked number by an ordinary fd forgets that number so
 * recycled descriptors are not misclassified. */
static long raw_dup3(long oldfd, long newfd, long flags) {
    return raw_syscall6(SYS_dup3, oldfd, newfd, flags, 0, 0, 0);
}

int dup(int oldfd) {
    long rc = raw_syscall6(SYS_dup, (long)oldfd, 0, 0, 0, 0, 0);
    if (rc < 0 || rc > 0x7FFFFFFF)
        return set_errno_ret(rc);
    if (is_fake_validated(oldfd))
        remember_fd((int)rc);
    else
        forget_fd((int)rc);
    return (int)rc;
}

int dup2(int oldfd, int newfd) {
    /* POSIX dup2(fd,fd): valid fd returns fd with no duplication.
     * Linux dup3(fd,fd,0) fails EINVAL instead, so handle it explicitly.
     * Probe validity with F_GETFD (raw SVC, no recursion, no fd lifetime
     * change). We never close newfd ourselves, avoiding the known
     * close-interposition hazard under PRoot. */
    if (oldfd == newfd) {
        long probe = raw_syscall6(SYS_fcntl, (long)oldfd, (long)F_GETFD, 0, 0, 0, 0);
        if (probe < 0 && probe > -4096)
            return set_errno_ret(probe);
        /* Valid: refresh stale tracking as a side effect (a recycled
         * ordinary number previously tracked as fake is forgotten here). */
        (void)is_fake_validated(oldfd);
        return newfd;
    }
    long rc = raw_dup3((long)oldfd, (long)newfd, 0);
    if (rc < 0 || rc > 0x7FFFFFFF)
        return set_errno_ret(rc);
    if (is_fake_validated(oldfd))
        remember_fd((int)rc);
    else
        forget_fd((int)rc);
    return (int)rc;
}

int dup3(int oldfd, int newfd, int flags) {
    /* dup3 keeps kernel EINVAL for oldfd==newfd (POSIX/Linux). */
    long rc = raw_dup3((long)oldfd, (long)newfd, (long)flags);
    if (rc < 0 || rc > 0x7FFFFFFF)
        return set_errno_ret(rc);
    if (is_fake_validated(oldfd))
        remember_fd((int)rc);
    else
        forget_fd((int)rc);
    return (int)rc;
}

/* fcntl command classification (Linux/AArch64, LP64). Sources:
 * asm-generic/fcntl.h (F_DUPFD..F_OFD_SETLKW) + linux/fcntl.h
 * (F_SETLEASE..F_SET_FILE_RW_HINT). Void commands take no third argument
 * and must not consume varargs (UB); integer commands take `int`;
 * pointer commands take `void *`. */
static int fcntl_is_void(int cmd) {
    switch (cmd) {
    case F_GETFD:
    case F_GETFL:
    case F_GETOWN:
    case F_GETSIG:
    case F_GETLEASE:
    case F_GETPIPE_SZ:
    case F_GET_SEALS:
        return 1;
    default:
        return 0;
    }
}

static int fcntl_is_int(int cmd) {
    switch (cmd) {
    case F_DUPFD:
    case F_SETFD:
    case F_SETFL:
    case F_SETOWN:
    case F_SETSIG:
    case F_SETLEASE:
    case F_NOTIFY:
    case F_DUPFD_CLOEXEC:
    case F_SETPIPE_SZ:
    case F_ADD_SEALS:
        return 1;
    default:
        return 0;
    }
}

static int fcntl_is_ptr(int cmd) {
    switch (cmd) {
    case F_GETLK:
    case F_SETLK:
    case F_SETLKW:
    case F_SETOWN_EX:
    case F_GETOWN_EX:
    case F_GETOWNER_UIDS:
    case F_OFD_GETLK:
    case F_OFD_SETLK:
    case F_OFD_SETLKW:
    case F_CANCELLK:
    case F_GET_RW_HINT:
    case F_SET_RW_HINT:
    case F_GET_FILE_RW_HINT:
    case F_SET_FILE_RW_HINT:
        return 1;
    default:
        return 0;
    }
}

static int do_fcntl(int fd, int cmd, long arg) {
    long rc = raw_syscall6(SYS_fcntl, (long)fd, (long)cmd, arg, 0, 0, 0);
    if (rc < 0 || rc > 0x7FFFFFFF)
        return set_errno_ret(rc);
    if (cmd == F_DUPFD || cmd == F_DUPFD_CLOEXEC) {
        if (is_fake_validated(fd))
            remember_fd((int)rc);
        else
            forget_fd((int)rc);
    }
    return (int)rc;
}

static long fcntl_fetch_arg(int cmd, __builtin_va_list *ap) {
    if (fcntl_is_int(cmd)) {
        int v = __builtin_va_arg(*ap, int);
        return (long)v;
    }
    if (fcntl_is_ptr(cmd)) {
        void *p = __builtin_va_arg(*ap, void *);
        return (long)p;
    }
    /* Unknown command: consume a register-sized slot. The kernel ignores
     * the third slot for unknown void commands, while unknown int/pointer
     * commands forward correctly. */
    {
        long v = __builtin_va_arg(*ap, long);
        return v;
    }
}

int fcntl(int fd, int cmd, ...) {
    if (fcntl_is_void(cmd))
        return do_fcntl(fd, cmd, 0);
    {
        __builtin_va_list ap;
        __builtin_va_start(ap, cmd);
        long arg = fcntl_fetch_arg(cmd, &ap);
        __builtin_va_end(ap);
        return do_fcntl(fd, cmd, arg);
    }
}

/* fcntl64 is a separate symbol on LP64 and the one Mesa's os_dupfd_cloexec
 * binds to: without it, dups of the fake render node escape tracking, the
 * version query on the copy bypasses the shim, hits the real /dev/null
 * backing with ENOTTY, and GBM creation dies ("cannot get version").
 * Proven on-device: the failing version query arrived on an untracked
 * /dev/null-backed fd with no dup/fcntl/open logging. */
int fcntl64(int fd, int cmd, ...) {
    if (fcntl_is_void(cmd))
        return do_fcntl(fd, cmd, 0);
    {
        __builtin_va_list ap;
        __builtin_va_start(ap, cmd);
        long arg = fcntl_fetch_arg(cmd, &ap);
        __builtin_va_end(ap);
        return do_fcntl(fd, cmd, arg);
    }
}

/* The reported name steers Mesa's winsys selection (see above): anything
 * but "msm" selects the kgsl winsys, which then probes the (real,
 * kgsl-backed) fd itself. "msm" would select the MSM winsys whose GEM
 * ioctls cannot work here. Version numbers are unused on the kgsl path. */
static const char version_name[] = "kgsl";
static const char version_desc[] = "KGSL-backed DRM node (portal-shimmed)";

static size_t bounded_copy(char *dst, size_t dst_len, const char *src, size_t src_len) {
    size_t n = dst_len < src_len ? dst_len : src_len;
    for (size_t i = 0; i < n; i++)
        dst[i] = src[i];
    return n;
}

static int handle_version_ioctl(struct drm_version *v) {
    v->version_major = 1;
    v->version_minor = 1;
    v->version_patchlevel = 0;
    if (v->name)
        bounded_copy(v->name, v->name_len, version_name, sizeof(version_name));
    v->name_len = sizeof(version_name);
    if (v->date)
        bounded_copy(v->date, v->date_len, "", 1);
    v->date_len = 1;
    if (v->desc)
        bounded_copy(v->desc, v->desc_len, version_desc, sizeof(version_desc));
    v->desc_len = sizeof(version_desc);
    return 0;
}

int ioctl(int fd, unsigned long request, ...) {
    void *arg = 0;
    {
        __builtin_va_list ap;
        __builtin_va_start(ap, request);
        arg = __builtin_va_arg(ap, void *);
        __builtin_va_end(ap);
    }
    if (!is_fake_fast(fd))
        return set_errno_ret(raw_syscall6(SYS_ioctl, (long)fd, (long)request, (long)arg, 0, 0, 0));
    if (DRM_IOCTL_TYPE(request) != DRM_IOCTL_VERSION_TYPE) {
        /* Non-DRM ioctls (notably KGSL 'k' ioctls when the backing is the
         * real /dev/kgsl-3d0) must reach the real device: the kgsl winsys
         * probes with KGSL_PROP_DEVICE_INFO and drives the GPU through
         * them. Failing them here would break the very path the shim
         * exists to enable. On a /dev/null backing they fail ENOTTY
         * naturally, identical to before. Fake and non-fake handling is
         * identical here, so validation is intentionally skipped to keep
         * per-frame GPU ioctls cheap (no extra fstat). */
        return set_errno_ret(raw_syscall6(SYS_ioctl, (long)fd, (long)request, (long)arg, 0, 0, 0));
    }
    /* DRM-type ioctls are rare and security-sensitive (the version probe
     * steers Mesa's winsys): validate the tracked fd so a recycled
     * ordinary number is never answered as "kgsl". */
    if (!is_fake_validated(fd))
        return set_errno_ret(raw_syscall6(SYS_ioctl, (long)fd, (long)request, (long)arg, 0, 0, 0));
    if (DRM_IOCTL_NR(request) == DRM_IOCTL_VERSION_NR) {
        return handle_version_ioctl((struct drm_version *)arg);
    }
    shim_log("drmshim: UNHANDLED ioctl ");
    shim_log_ulong_hex(request);
    shim_log(" on fake fd ");
    shim_log_int(fd);
    shim_log(" (failing ENOTTY)\n");
    *__errno_location() = ENOTTY;
    return -1;
}
