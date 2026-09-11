/*
 * Project Anland DRM render-device shim — guest-native ARM64 glibc sources.
 *
 * KWin's Anland backend requires a DRM render device at init
 * (openRenderDevice) even for the surfaceless path, but /dev/dri/renderD128 is
 * not openable from the Android app sandbox (SELinux/DAC). Without a device,
 * KWin logs "no usable DRM render device; cannot bring up OpenGL compositing"
 * and exits.
 *
 * This shim fakes ONLY the open()+version probe so init can proceed:
 *   - intercept open/open64/openat of /dev/dri/renderD128 and /dev/dri/card0:
 *     return a real fd for /dev/null, log it, remember the fd;
 *   - intercept ioctl on remembered fds:
 *     DRM_IOCTL_VERSION -> report driver "msm" v1.1.0 (length query + fill);
 *     anything else -> log the number, fail ENOTTY.
 * Every other call passes through untouched. NOTE: close() is deliberately
 * NOT interposed. A raw-SVC close replacement breaks the process under PRoot
 * (proven by bisect: interposing only close makes cat/python fail with
 * EFAULT after successful reads; open/openat/ioctl-only variants are clean).
 * fds are therefore tracked monotonically (bounded table); a closed fake fd
 * whose number is recycled for a real file is harmless because only DRM
 * nodes ever receive ('d',0) version ioctls, and all real DRM nodes are
 * faked from the start.
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
 * Freestanding (-nostdlib): raw AArch64 SVC for openat/write/ioctl/close so
 * the shim never recurses into libc. errno is set through libc's own
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

#define SYS_dup 23
#define SYS_dup3 24
#define SYS_fcntl 25
#define SYS_openat 56
#define SYS_write 64
#define SYS_ioctl 29
#define F_DUPFD 0
#define F_DUPFD_CLOEXEC 1030
#define AT_FDCWD (-100)
#define O_RDONLY 0
#define O_CREAT 0x40
#define O_TMPFILE 0x410000
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
static int fake_fds[MAX_FAKE_FDS];
static int fake_count = 0;

static int is_fake_fd(int fd) {
    for (int i = 0; i < fake_count; i++) {
        if (fake_fds[i] == fd)
            return 1;
    }
    return 0;
}

static void remember_fd(int fd) {
    if (is_fake_fd(fd))
        return;
    if (fake_count < MAX_FAKE_FDS)
        fake_fds[fake_count++] = fd;
}

static int is_dri_node(const char *path) {
    return shim_streq(path, "/dev/dri/renderD128") || shim_streq(path, "/dev/dri/card0");
}

/* Open /dev/null for real and present it as the DRM node. */
static int fake_open_node(const char *label, const char *path) {
    long fd = svc4(SYS_openat, AT_FDCWD, (long)"/dev/null", O_RDONLY, 0);
    if (fd < 0 || fd > 0x7FFFFFFF)
        return -1;
    remember_fd((int)fd);
    shim_log("drmshim: faked ");
    shim_log(label);
    shim_log("(");
    shim_log(path);
    shim_log(") -> fd ");
    shim_log_int((int)fd);
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
 * them is safe. */
static long raw_dup3(long oldfd, long newfd, long flags) {
    return raw_syscall6(SYS_dup3, oldfd, newfd, flags, 0, 0, 0);
}

int dup(int oldfd) {
    long rc = raw_syscall6(SYS_dup, (long)oldfd, 0, 0, 0, 0, 0);
    if (rc < 0 || rc > 0x7FFFFFFF)
        return set_errno_ret(rc);
    if (is_fake_fd(oldfd))
        remember_fd((int)rc);
    return (int)rc;
}

int dup2(int oldfd, int newfd) {
    long rc = raw_dup3((long)oldfd, (long)newfd, 0);
    if (rc < 0 || rc > 0x7FFFFFFF)
        return set_errno_ret(rc);
    if (is_fake_fd(oldfd))
        remember_fd((int)rc);
    return (int)rc;
}

int dup3(int oldfd, int newfd, int flags) {
    long rc = raw_dup3((long)oldfd, (long)newfd, (long)flags);
    if (rc < 0 || rc > 0x7FFFFFFF)
        return set_errno_ret(rc);
    if (is_fake_fd(oldfd))
        remember_fd((int)rc);
    return (int)rc;
}

static int do_fcntl(int fd, int cmd, long arg) {
    long rc = raw_syscall6(SYS_fcntl, (long)fd, (long)cmd, arg, 0, 0, 0);
    if (rc < 0 || rc > 0x7FFFFFFF)
        return set_errno_ret(rc);
    if ((cmd == F_DUPFD || cmd == F_DUPFD_CLOEXEC) && is_fake_fd(fd))
        remember_fd((int)rc);
    return (int)rc;
}

int fcntl(int fd, int cmd, ...) {
    long arg = 0;
    {
        __builtin_va_list ap;
        __builtin_va_start(ap, cmd);
        arg = __builtin_va_arg(ap, long);
        __builtin_va_end(ap);
    }
    return do_fcntl(fd, cmd, arg);
}

/* fcntl64 is a separate symbol on LP64 and the one Mesa's os_dupfd_cloexec
 * binds to: without it, dups of the fake render node escape tracking, the
 * version query on the copy bypasses the shim, hits the real /dev/null
 * backing with ENOTTY, and GBM creation dies ("cannot get version").
 * Proven on-device: the failing version query arrived on an untracked
 * /dev/null-backed fd with no dup/fcntl/open logging. */
int fcntl64(int fd, int cmd, ...) {
    long arg = 0;
    {
        __builtin_va_list ap;
        __builtin_va_start(ap, cmd);
        arg = __builtin_va_arg(ap, long);
        __builtin_va_end(ap);
    }
    return do_fcntl(fd, cmd, arg);
}

static const char version_name[] = "msm";
static const char version_desc[] = "MSM Snapdragon DRM (portal-shimmed)";

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
    if (!is_fake_fd(fd))
        return set_errno_ret(raw_syscall6(SYS_ioctl, (long)fd, (long)request, (long)arg, 0, 0, 0));
    if (DRM_IOCTL_TYPE(request) == DRM_IOCTL_VERSION_TYPE &&
        DRM_IOCTL_NR(request) == DRM_IOCTL_VERSION_NR) {
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
