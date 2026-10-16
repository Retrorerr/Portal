/*
 * Portal V4L2 front — guest-native ARM64 glibc sources.
 *
 * Android gives the guest no /dev/video* device: the tablet's cameras reach
 * the guest as PipeWire Video/Source nodes ("Portal Front Camera", ...)
 * published by Portal's capture helper. PipeWire-native apps (GStreamer
 * pipewiresrc, OBS, xdg-desktop-portal's Camera portal) use those directly.
 * Most webcam apps (Chromium/Electron, Firefox's libwebrtc, ffmpeg, Cheese,
 * Zoom) speak V4L2 instead. PipeWire ships the V4L2 emulation for them,
 * libpw-v4l2.so (the `pw-v4l2` wrapper's preload), which serves /dev/videoN
 * from those nodes.
 *
 * libpw-v4l2 is meant to wrap single apps: preloaded session-wide it would
 * run pw_init() in every process on its first open() and take a lock on
 * every ioctl/close/munmap, KWin's GPU ioctls included. This front is what
 * the session preloads instead. Until a process opens /dev/videoN it only
 * forwards (one relaxed load and a branch per call). On that first open it
 * dlopen()s libpw-v4l2 and from then on hands it exactly the descriptors
 * and mappings it created; everything else still goes straight on. It must
 * come before drmshim.so in LD_PRELOAD, whose open/ioctl do not chain.
 *
 * PRoot binds /dev/video0../dev/videoN to /dev/null so directory scans
 * (Chromium) and S_ISCHR checks (GStreamer v4l2src) see a device; without
 * this front, opening one gives /dev/null and VIDIOC_QUERYCAP fails ENOTTY,
 * which apps treat as "not a camera".
 *
 * Build (Debian trixie, aarch64): see portal-v4l2-recipe.txt.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <linux/videodev2.h>
#include <poll.h>
#include <sys/mman.h>
#include <sys/types.h>
#include <unistd.h>

#define EXPORT __attribute__((visibility("default")))

static const char *const PW_V4L2_PATHS[] = {
	"/usr/lib/aarch64-linux-gnu/pipewire-0.3/v4l2/libpw-v4l2.so",
	"/usr/local/lib/portal/libpw-v4l2.so",
};

struct fops {
	int (*openat)(int, const char *, int, ...);
	int (*dup)(int);
	int (*close)(int);
	int (*ioctl)(int, unsigned long, ...);
	void *(*mmap)(void *, size_t, int, int, int, off_t);
	int (*munmap)(void *, size_t);
};

/* Every open spelling forwards to its own next definition (drmshim's or
 * libc's), so paths other than /dev/videoN behave exactly as without this
 * front. */
struct next_ops {
	struct fops f;
	int (*open)(const char *, int, ...);
	int (*open64)(const char *, int, ...);
	int (*openat64)(int, const char *, int, ...);
	int (*open_2)(const char *, int);
	int (*open64_2)(const char *, int);
	int (*openat_2)(int, const char *, int);
	int (*openat64_2)(int, const char *, int);
};

static struct next_ops next_ops;
static struct fops pw_ops;
static pthread_once_t next_once = PTHREAD_ONCE_INIT;
static pthread_mutex_t load_lock = PTHREAD_MUTEX_INITIALIZER;
/* 0 = not loaded, 1 = loaded, -1 = failed (stay passthrough) */
static int pw_state;

#define OWNED_FDS 65536
/* Per-descriptor flags: OWN = libpw-v4l2's; APP_NONBLOCK = the app asked for
 * O_NONBLOCK; STREAMING = between STREAMON and STREAMOFF. */
#define OWN 1
#define APP_NONBLOCK 2
#define STREAMING 4
static unsigned char owned[OWNED_FDS];

#define OWNED_MAPS 128
static void *maps[OWNED_MAPS];
static pthread_mutex_t map_lock = PTHREAD_MUTEX_INITIALIZER;

static int (*next_fcntl)(int, int, ...);
static int (*next_fcntl64)(int, int, ...);

static void resolve_next(void)
{
	next_ops.f.openat = dlsym(RTLD_NEXT, "openat");
	next_ops.f.dup = dlsym(RTLD_NEXT, "dup");
	next_ops.f.close = dlsym(RTLD_NEXT, "close");
	next_ops.f.ioctl = dlsym(RTLD_NEXT, "ioctl");
	next_ops.f.mmap = dlsym(RTLD_NEXT, "mmap64");
	next_ops.f.munmap = dlsym(RTLD_NEXT, "munmap");
	next_ops.open = dlsym(RTLD_NEXT, "open");
	next_ops.open64 = dlsym(RTLD_NEXT, "open64");
	next_ops.openat64 = dlsym(RTLD_NEXT, "openat64");
	next_ops.open_2 = dlsym(RTLD_NEXT, "__open_2");
	next_ops.open64_2 = dlsym(RTLD_NEXT, "__open64_2");
	next_ops.openat_2 = dlsym(RTLD_NEXT, "__openat_2");
	next_ops.openat64_2 = dlsym(RTLD_NEXT, "__openat64_2");
	next_fcntl = dlsym(RTLD_NEXT, "fcntl");
	next_fcntl64 = dlsym(RTLD_NEXT, "fcntl64");
}

static inline const struct next_ops *nx(void)
{
	pthread_once(&next_once, resolve_next);
	return &next_ops;
}

static inline const struct fops *next(void)
{
	return &nx()->f;
}

__attribute__((constructor(101))) static void portal_v4l2_init(void)
{
	nx();
}

static inline int loaded(void)
{
	return __atomic_load_n(&pw_state, __ATOMIC_ACQUIRE) == 1;
}

static inline int is_owned(int fd)
{
	return fd >= 0 && fd < OWNED_FDS && __atomic_load_n(&owned[fd], __ATOMIC_RELAXED);
}

static inline void set_owned(int fd, unsigned char value)
{
	if (fd >= 0 && fd < OWNED_FDS)
		__atomic_store_n(&owned[fd], value, __ATOMIC_RELAXED);
}

static inline unsigned char fd_flags(int fd)
{
	return fd >= 0 && fd < OWNED_FDS ? __atomic_load_n(&owned[fd], __ATOMIC_RELAXED) : 0;
}

static inline void update_flags(int fd, unsigned char set, unsigned char clear)
{
	if (fd >= 0 && fd < OWNED_FDS) {
		__atomic_or_fetch(&owned[fd], set, __ATOMIC_RELAXED);
		__atomic_and_fetch(&owned[fd], (unsigned char)~clear, __ATOMIC_RELAXED);
	}
}

/* "/dev/videoN" with N all digits. */
static int is_video_path(const char *path)
{
	if (path == NULL || strncmp(path, "/dev/video", 10) != 0)
		return 0;
	const char *p = path + 10;
	if (*p < '0' || *p > '9')
		return 0;
	while (*p >= '0' && *p <= '9')
		p++;
	return *p == '\0';
}

static int load_pw(void)
{
	int state = __atomic_load_n(&pw_state, __ATOMIC_ACQUIRE);
	if (state != 0)
		return state == 1;

	pthread_mutex_lock(&load_lock);
	state = pw_state;
	if (state == 0) {
		void *handle = NULL;
		for (size_t i = 0; i < sizeof(PW_V4L2_PATHS) / sizeof(PW_V4L2_PATHS[0]) && !handle; i++)
			handle = dlopen(PW_V4L2_PATHS[i], RTLD_NOW | RTLD_LOCAL);
		if (handle) {
			pw_ops.openat = dlsym(handle, "openat64");
			pw_ops.dup = dlsym(handle, "dup");
			pw_ops.close = dlsym(handle, "close");
			pw_ops.ioctl = dlsym(handle, "ioctl");
			pw_ops.mmap = dlsym(handle, "mmap64");
			pw_ops.munmap = dlsym(handle, "munmap");
		}
		state = handle && pw_ops.openat && pw_ops.dup && pw_ops.close &&
				pw_ops.ioctl && pw_ops.mmap && pw_ops.munmap ? 1 : -1;
		__atomic_store_n(&pw_state, state, __ATOMIC_RELEASE);
	}
	pthread_mutex_unlock(&load_lock);
	return state == 1;
}

/* A camera open: 1 with *fd set when PipeWire answered, 0 to fall through
 * to the real file. With no camera node behind the number (ENOENT) the
 * /dev/null bind answers, so the app sees ENOTTY on VIDIOC_QUERYCAP and
 * skips it as "not a camera". */
static pthread_mutex_t claim_lock = PTHREAD_MUTEX_INITIALIZER;
static unsigned claimed;

/* libpw-v4l2 gives each /dev/videoN the first camera node no other number
 * has claimed yet in this process, so on its own "/dev/video1" opened first
 * would be the first camera. Opening the lower numbers once, in order, makes
 * N the N-th camera in every process, as with real devices. */
static void claim_lower(unsigned n)
{
	char path[24];
	pthread_mutex_lock(&claim_lock);
	for (; claimed < n && claimed < 64; claimed++) {
		snprintf(path, sizeof(path), "/dev/video%u", claimed);
		int fd = pw_ops.openat(AT_FDCWD, path, O_RDWR | O_CLOEXEC, 0);
		if (fd < 0)
			break;
		pw_ops.close(fd);
	}
	if (claimed < n)
		claimed = n;
	pthread_mutex_unlock(&claim_lock);
}

static int video_open(int dirfd, const char *path, int flags, mode_t mode, int *fd)
{
	if (!is_video_path(path) || !load_pw())
		return 0;
	unsigned n = (unsigned)strtoul(path + 10, NULL, 10);
	if (n > __atomic_load_n(&claimed, __ATOMIC_RELAXED))
		claim_lower(n);
	/* libpw-v4l2 is always opened non-blocking: its blocking DQBUF sleeps on
	 * the eventfd until the next frame, STREAMOFF and close() do not wake it,
	 * and while it sleeps it holds the file (and the camera) open. Blocking
	 * apps get it emulated in dqbuf() below. */
	*fd = pw_ops.openat(dirfd, path, flags | O_NONBLOCK, mode);
	if (*fd >= 0) {
		set_owned(*fd, OWN | (flags & O_NONBLOCK ? APP_NONBLOCK : 0));
		return 1;
	}
	return errno != ENOENT;
}

static inline int opened(int fd)
{
	if (fd >= 0 && loaded())
		set_owned(fd, 0);
	return fd;
}

static inline int needs_mode(int flags)
{
	return (flags & O_CREAT) || ((flags & O_TMPFILE) == O_TMPFILE);
}

#define MODE_ARG(flags, last, mode)                \
	do {                                       \
		if (needs_mode(flags)) {           \
			va_list ap;                \
			va_start(ap, last);        \
			mode = va_arg(ap, mode_t); \
			va_end(ap);                \
		}                                  \
	} while (0)

EXPORT int open(const char *path, int flags, ...)
{
	mode_t mode = 0;
	int fd;
	MODE_ARG(flags, flags, mode);
	if (video_open(AT_FDCWD, path, flags, mode, &fd))
		return fd;
	return opened(nx()->open(path, flags, mode));
}

EXPORT int open64(const char *path, int flags, ...)
{
	mode_t mode = 0;
	int fd;
	MODE_ARG(flags, flags, mode);
	if (video_open(AT_FDCWD, path, flags | O_LARGEFILE, mode, &fd))
		return fd;
	return opened(nx()->open64(path, flags, mode));
}

EXPORT int openat(int dirfd, const char *path, int flags, ...)
{
	mode_t mode = 0;
	int fd;
	MODE_ARG(flags, flags, mode);
	if (video_open(dirfd, path, flags, mode, &fd))
		return fd;
	return opened(nx()->f.openat(dirfd, path, flags, mode));
}

EXPORT int openat64(int dirfd, const char *path, int flags, ...)
{
	mode_t mode = 0;
	int fd;
	MODE_ARG(flags, flags, mode);
	if (video_open(dirfd, path, flags | O_LARGEFILE, mode, &fd))
		return fd;
	return opened(nx()->openat64(dirfd, path, flags, mode));
}

/* _FORTIFY_SOURCE spellings; O_CREAT never reaches these. */
EXPORT int __open_2(const char *path, int flags)
{
	int fd;
	if (video_open(AT_FDCWD, path, flags, 0, &fd))
		return fd;
	return opened(nx()->open_2(path, flags));
}

EXPORT int __open64_2(const char *path, int flags)
{
	int fd;
	if (video_open(AT_FDCWD, path, flags | O_LARGEFILE, 0, &fd))
		return fd;
	return opened(nx()->open64_2(path, flags));
}

EXPORT int __openat_2(int dirfd, const char *path, int flags)
{
	int fd;
	if (video_open(dirfd, path, flags, 0, &fd))
		return fd;
	return opened(nx()->openat_2(dirfd, path, flags));
}

EXPORT int __openat64_2(int dirfd, const char *path, int flags)
{
	int fd;
	if (video_open(dirfd, path, flags | O_LARGEFILE, 0, &fd))
		return fd;
	return opened(nx()->openat64_2(dirfd, path, flags));
}

EXPORT int dup(int fd)
{
	if (loaded() && is_owned(fd)) {
		int copy = pw_ops.dup(fd);
		set_owned(copy, fd_flags(fd) & (OWN | APP_NONBLOCK));
		return copy;
	}
	int copy = next()->dup(fd);
	if (copy >= 0 && loaded())
		set_owned(copy, 0);
	return copy;
}

EXPORT int close(int fd)
{
	if (loaded() && is_owned(fd)) {
		set_owned(fd, 0);
		return pw_ops.close(fd);
	}
	return next()->close(fd);
}

/* A blocking VIDIOC_DQBUF on the non-blocking libpw-v4l2 descriptor: wait
 * for a frame in short slices so STREAMOFF or close() from another thread
 * ends the wait, as with a real driver. A camera that stalls for a second
 * gives EAGAIN, which capture loops (libwebrtc, ffmpeg) treat as "try again"
 * and which lets their stop flag be seen. */
static int dqbuf(int fd, void *arg)
{
	for (int waited = 0;; waited += 100) {
		int res = pw_ops.ioctl(fd, VIDIOC_DQBUF, arg);
		if (res == 0 || errno != EAGAIN || (fd_flags(fd) & APP_NONBLOCK))
			return res;
		if (!(fd_flags(fd) & OWN) || !(fd_flags(fd) & STREAMING)) {
			errno = EINVAL;
			return -1;
		}
		if (waited >= 1000) {
			errno = EAGAIN;
			return -1;
		}
		struct pollfd pfd = { .fd = fd, .events = POLLIN };
		if (poll(&pfd, 1, 100) < 0 && errno != EINTR)
			return -1;
		if (pfd.revents & POLLNVAL) {
			errno = EBADF;
			return -1;
		}
	}
}

EXPORT int ioctl(int fd, unsigned long request, ...)
{
	va_list ap;
	va_start(ap, request);
	void *arg = va_arg(ap, void *);
	va_end(ap);
	if (loaded() && is_owned(fd)) {
		if (request == VIDIOC_DQBUF)
			return dqbuf(fd, arg);
		if (request == VIDIOC_STREAMOFF)
			update_flags(fd, 0, STREAMING);
		int res = pw_ops.ioctl(fd, request, arg);
		if (request == VIDIOC_STREAMON && res == 0)
			update_flags(fd, STREAMING, 0);
		return res;
	}
	return next()->ioctl(fd, request, arg);
}

/* O_NONBLOCK set or cleared later by the app is its own; the descriptor
 * itself stays non-blocking. */
static long owned_fcntl(int fd, int cmd, unsigned long arg,
		int (*real)(int, int, ...))
{
	if (cmd == F_GETFL) {
		int fl = real(fd, F_GETFL);
		if (fl >= 0 && !(fd_flags(fd) & APP_NONBLOCK))
			fl &= ~O_NONBLOCK;
		return fl;
	}
	if (cmd == F_SETFL) {
		if (arg & O_NONBLOCK)
			update_flags(fd, APP_NONBLOCK, 0);
		else
			update_flags(fd, 0, APP_NONBLOCK);
		return real(fd, F_SETFL, arg | O_NONBLOCK);
	}
	return real(fd, cmd, arg);
}

EXPORT int fcntl(int fd, int cmd, ...)
{
	va_list ap;
	va_start(ap, cmd);
	unsigned long arg = va_arg(ap, unsigned long);
	va_end(ap);
	nx();
	if (loaded() && is_owned(fd) && (cmd == F_GETFL || cmd == F_SETFL))
		return owned_fcntl(fd, cmd, arg, next_fcntl);
	return next_fcntl(fd, cmd, arg);
}

EXPORT int fcntl64(int fd, int cmd, ...)
{
	va_list ap;
	va_start(ap, cmd);
	unsigned long arg = va_arg(ap, unsigned long);
	va_end(ap);
	nx();
	int (*real)(int, int, ...) = next_fcntl64 ? next_fcntl64 : next_fcntl;
	if (loaded() && is_owned(fd) && (cmd == F_GETFL || cmd == F_SETFL))
		return owned_fcntl(fd, cmd, arg, real);
	return real(fd, cmd, arg);
}

static void remember_map(void *addr)
{
	pthread_mutex_lock(&map_lock);
	for (int i = 0; i < OWNED_MAPS; i++) {
		if (maps[i] == NULL) {
			maps[i] = addr;
			break;
		}
	}
	pthread_mutex_unlock(&map_lock);
}

static int forget_map(void *addr)
{
	int found = 0;
	pthread_mutex_lock(&map_lock);
	for (int i = 0; i < OWNED_MAPS; i++) {
		if (maps[i] == addr) {
			maps[i] = NULL;
			found = 1;
			break;
		}
	}
	pthread_mutex_unlock(&map_lock);
	return found;
}

EXPORT void *mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset)
{
	if (loaded() && is_owned(fd)) {
		void *res = pw_ops.mmap(addr, length, prot, flags, fd, offset);
		if (res != MAP_FAILED)
			remember_map(res);
		return res;
	}
	return next()->mmap(addr, length, prot, flags, fd, offset);
}

EXPORT void *mmap64(void *addr, size_t length, int prot, int flags, int fd, off_t offset)
{
	return mmap(addr, length, prot, flags, fd, offset);
}

EXPORT int munmap(void *addr, size_t length)
{
	if (loaded() && forget_map(addr))
		return pw_ops.munmap(addr, length);
	return next()->munmap(addr, length);
}
