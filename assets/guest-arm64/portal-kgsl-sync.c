/*
 * Managed by Portal: waits for each GPU submission of Steam's web helper to
 * finish before the submitting call returns.
 *
 * Steam's window is drawn by two steamwebhelper processes: CEF's GPU process
 * renders the page into shared dma-bufs and the browser process composites
 * them into the SDL window, which Xwayland copies again. On desktop Linux the
 * kernel orders those reads and writes through the dma-buf's implicit fences.
 * KGSL attaches none, and Mesa's KGSL backend submits and returns, so a
 * reader can sample a buffer the writer's GPU work has not finished (or has
 * already started overwriting): parts of the Steam window blink out as other
 * parts redraw.
 *
 * Waiting for the submission's timestamp right after IOCTL_KGSL_GPU_COMMAND
 * turns every flush into a finished write, which gives the cross-process
 * ordering implicit sync would have given. It costs the GPU/CPU overlap of a
 * UI that draws little; games never load this (exe check below).
 *
 * Must precede drmshim.so in LD_PRELOAD: drmshim answers KGSL ioctls itself.
 *
 * Build (guest-native ARM64 glibc, no libc at link time):
 *   aarch64-linux-gnu-gcc -shared -fPIC -nostdlib -O2 -ffreestanding \
 *       -fno-stack-protector -Wl,-z,max-page-size=16384 -Wl,-s \
 *       -o portal-kgsl-sync.so portal-kgsl-sync.c
 */
#include <stdarg.h>
#include <stdint.h>

#define RTLD_NEXT ((void *)-1l)

extern void *dlsym(void *handle, const char *symbol);
extern long readlink(const char *path, char *buf, unsigned long size);

/* msm_kgsl.h */
struct kgsl_gpu_command {
    uint64_t flags;
    uint64_t cmdlist;
    unsigned int cmdsize;
    unsigned int numcmds;
    uint64_t objlist;
    unsigned int objsize;
    unsigned int numobjs;
    uint64_t synclist;
    unsigned int syncsize;
    unsigned int numsyncs;
    unsigned int context_id;
    unsigned int timestamp;
};

struct kgsl_device_waittimestamp_ctxtid {
    unsigned int context_id;
    unsigned int timestamp;
    unsigned int timeout; /* ms */
};

/* _IOWR(0x09, 0x4A, struct kgsl_gpu_command) */
#define IOCTL_KGSL_GPU_COMMAND 0xC040094Au
/* _IOW(0x09, 0x07, struct kgsl_device_waittimestamp_ctxtid) */
#define IOCTL_KGSL_DEVICE_WAITTIMESTAMP_CTXTID 0x400C0907u

typedef int (*ioctl_fn)(int, unsigned long, ...);

static ioctl_fn real_ioctl;
static int active = -1;

static int in_web_helper(void) {
    static const char name[] = "/steamwebhelper";
    char exe[512];
    long n = readlink("/proc/self/exe", exe, sizeof exe);
    long len = sizeof name - 1;
    if (n < len) return 0;
    for (long i = 0; i < len; i++)
        if (exe[n - len + i] != name[i]) return 0;
    return 1;
}

int ioctl(int fd, unsigned long request, ...) {
    va_list ap;
    va_start(ap, request);
    void *arg = va_arg(ap, void *);
    va_end(ap);

    if (!real_ioctl) real_ioctl = (ioctl_fn)dlsym(RTLD_NEXT, "ioctl");
    int ret = real_ioctl(fd, request, arg);

    if (ret != 0 || (unsigned int)request != IOCTL_KGSL_GPU_COMMAND)
        return ret;
    if (active < 0) active = in_web_helper();
    if (!active) return ret;

    struct kgsl_gpu_command *cmd = arg;
    struct kgsl_device_waittimestamp_ctxtid wait = {
        .context_id = cmd->context_id,
        .timestamp = cmd->timestamp,
        .timeout = 1000,
    };
    /* A failed or timed-out wait only loses the ordering, never the submit. */
    real_ioctl(fd, IOCTL_KGSL_DEVICE_WAITTIMESTAMP_CTXTID, &wait);
    return 0;
}
