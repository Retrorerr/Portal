/*
 * Project Anland damage hint — guest-native ARM64 glibc preload for KWin.
 *
 * Anland presentation is consumer-driven: the host selects a buffer, KWin
 * renders into it. Without input, the host only selects on a 1 Hz heartbeat,
 * so anything that animates on its own (the Plasma panel sliding in at
 * startup, splash fade, notifications, KWin effects, video) crawled at
 * 1 fps. KWin knows exactly when it has something to paint; this shim
 * forwards that knowledge to the host.
 *
 * Every scene repaint request funnels through the exported
 *   KWin::RenderLoop::scheduleRepaint(Item *, RenderLayer *, OutputLayer *)
 * and Portal's libkwin is linked without -Bsymbolic, so its internal calls go
 * through the PLT and are interposable. Surface commits, effect animations,
 * cursor and layer repaints pass a non-null item/layer; the Anland backend's
 * own per-select scheduleRepaint() passes all nulls and is NOT forwarded
 * (forwarding it would turn every presented frame into a request for the
 * next one). The original functions always run, unchanged.
 *
 * The Anland layer's buffer catch-up is excluded too: after every frame its
 * doEndFrame() calls OutputLayer::addRepaint(infiniteRegion()) while another
 * window buffer still owes damage, and that full repaint makes every other
 * buffer owe damage again, so forwarding it would present forever. Effects
 * and scene repaints reach addRepaint() with real device rects, so only the
 * infinite region (KWin's infiniteRegion(), origin INT_MIN / 2) is muted.
 * The muted repaint is still recorded by KWin; it renders on the next
 * select, so skipping the hint never skips owed damage.
 *
 * A hint is one byte on a non-blocking datagram socket (ANLAND_DAMAGE_SOCKET,
 * default /tmp/anland/damage.sock), sent at most once per 2 ms. Any failure
 * (host not listening, socket gone) is silent and retried at most once per
 * second: without the host listener KWin behaves exactly as before.
 *
 * Preloaded ONLY in Anland sessions (see localdesktop-kwin-wrapper-v2.sh).
 *
 *   clang --target=aarch64-linux-gnu -fuse-ld=lld -nostdlib -shared -fPIC \
 *       --sysroot=target/guest-support-sysroot \
 *       -isystem target/guest-support-sysroot/usr/include/aarch64-linux-gnu \
 *       -O2 -Wall -Wextra -fno-stack-protector \
 *       -Wl,-soname,anland-damage.so -Wl,--build-id=sha1 \
 *       -o assets/guest-arm64/anland-damage.so assets/guest-arm64/anland-damage.c
 *
 * Undefined glibc symbols (socket, connect, send, clock_gettime, getenv,
 * dlsym) resolve from KWin's own libc at load time; nothing links bionic.
 */
#include <errno.h>
#include <stddef.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <time.h>

#define RTLD_NEXT ((void *)-1L)
extern void *dlsym(void *handle, const char *symbol);
extern char *getenv(const char *name);
extern int close(int fd);

#define SCHEDULE_REPAINT_SYMBOL \
    "_ZN4KWin10RenderLoop15scheduleRepaintEPNS_4ItemEPNS_11RenderLayerEPNS_11OutputLayerE"
#define ADD_REPAINT_SYMBOL "_ZN4KWin11OutputLayer10addRepaintERK7QRegion"

typedef void (*schedule_repaint_fn)(void *self, void *item, void *layer, void *output_layer);
typedef void (*add_repaint_fn)(void *self, const void *region);

/* QRect is four ints, returned in registers like this struct (AAPCS64). */
struct qrect {
    int x1, y1, x2, y2;
};
extern struct qrect _ZNK7QRegion12boundingRectEv(const void *region);

static schedule_repaint_fn real_schedule_repaint;
static add_repaint_fn real_add_repaint;
/* KWin's compositing is single-threaded; set only around the muted call. */
static int muting_catch_up;
static int hint_fd = -1;
static long long last_hint_ns;
static long long last_connect_ns = -1000000000LL;

static long long now_ns(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000000000LL + ts.tv_nsec;
}

static void hint_connect(long long now)
{
    if (now - last_connect_ns < 1000000000LL) {
        return;
    }
    last_connect_ns = now;
    const char *path = getenv("ANLAND_DAMAGE_SOCKET");
    if (!path || !*path) {
        path = "/tmp/anland/damage.sock";
    }
    struct sockaddr_un addr;
    memset(&addr, 0, sizeof(addr));
    addr.sun_family = AF_UNIX;
    size_t len = strlen(path);
    if (len >= sizeof(addr.sun_path)) {
        return;
    }
    memcpy(addr.sun_path, path, len);
    int fd = socket(AF_UNIX, SOCK_DGRAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
    if (fd < 0) {
        return;
    }
    if (connect(fd, (struct sockaddr *)&addr, sizeof(addr)) != 0) {
        close(fd);
        return;
    }
    hint_fd = fd;
}

static void damage_hint(void)
{
    long long now = now_ns();
    if (now - last_hint_ns < 2000000LL) {
        return;
    }
    last_hint_ns = now;
    if (hint_fd < 0) {
        hint_connect(now);
        if (hint_fd < 0) {
            return;
        }
    }
    const char byte = 1;
    if (send(hint_fd, &byte, 1, MSG_DONTWAIT | MSG_NOSIGNAL) < 0) {
        /* EAGAIN: the host already has hints queued, nothing lost. Anything
         * else (host restarted, socket gone): reconnect later. */
        if (errno != EAGAIN && errno != EWOULDBLOCK) {
            close(hint_fd);
            hint_fd = -1;
        }
    }
}

__attribute__((visibility("default"))) void
_ZN4KWin10RenderLoop15scheduleRepaintEPNS_4ItemEPNS_11RenderLayerEPNS_11OutputLayerE(
    void *self, void *item, void *layer, void *output_layer)
{
    if (!muting_catch_up && (item || layer || output_layer)) {
        damage_hint();
    }
    if (!real_schedule_repaint) {
        real_schedule_repaint = (schedule_repaint_fn)dlsym(RTLD_NEXT, SCHEDULE_REPAINT_SYMBOL);
        if (!real_schedule_repaint) {
            return;
        }
    }
    real_schedule_repaint(self, item, layer, output_layer);
}

__attribute__((visibility("default"))) void
_ZN4KWin11OutputLayer10addRepaintERK7QRegion(void *self, const void *region)
{
    if (!real_add_repaint) {
        real_add_repaint = (add_repaint_fn)dlsym(RTLD_NEXT, ADD_REPAINT_SYMBOL);
        if (!real_add_repaint) {
            return;
        }
    }
    const int infinite = _ZNK7QRegion12boundingRectEv(region).x1 <= -1000000000;
    muting_catch_up += infinite;
    real_add_repaint(self, region);
    muting_catch_up -= infinite;
}
