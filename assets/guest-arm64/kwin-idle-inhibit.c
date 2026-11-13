/* Forward KWin's effective Wayland idle inhibitors to Android.
 *
 * KWin calls these two exported methods after applying its visibility and
 * virtual desktop rules. Both packaged KWin builds call through the PLT.
 * The original methods still run. Only the window identities are tracked,
 * so this shim needs no Qt or KWin object layout. An open socket keeps the
 * display awake, and process exit also releases it.
 *
 *   clang --target=aarch64-linux-gnu -fuse-ld=lld -nostdlib -shared -fPIC \
 *       --sysroot=target/guest-support-sysroot \
 *       -isystem target/guest-support-sysroot/usr/include/aarch64-linux-gnu \
 *       -O2 -Wall -Wextra -Werror -fno-stack-protector -fvisibility=hidden \
 *       -Wl,-soname,kwin-idle-inhibit.so -Wl,--build-id=sha1 -Wl,-z,max-page-size=16384 -s \
 *       -o assets/guest-arm64/kwin-idle-inhibit.so assets/guest-arm64/kwin-idle-inhibit.c
 */
#include <dlfcn.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#define ADD_INHIBITOR _ZN4KWin16InputRedirection16addIdleInhibitorEPNS_6WindowE
#define REMOVE_INHIBITOR _ZN4KWin16InputRedirection19removeIdleInhibitorEPNS_6WindowE
#define STRINGIFY_IMPL(symbol) #symbol
#define STRINGIFY(symbol) STRINGIFY_IMPL(symbol)

typedef void (*inhibitor_fn)(void *input, void *window);

struct inhibitor {
    void *window;
    struct inhibitor *next;
};

/* The KWin compositor thread owns this list, just as it owns the originals. */
static struct inhibitor *windows;
static int lease = -1;

static int open_lease(void)
{
    const char *name = getenv("PORTAL_SCREEN_INHIBIT_SOCKET");
    struct sockaddr_un address = { .sun_family = AF_UNIX };
    if (!name || !*name || strlen(name) >= sizeof(address.sun_path)) {
        return -1;
    }
    size_t length = strlen(name);
    memcpy(address.sun_path + 1, name, length);
    int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
    if (fd >= 0 && connect(fd, (struct sockaddr *)&address,
                           offsetof(struct sockaddr_un, sun_path) + 1 + length) != 0) {
        close(fd);
        return -1;
    }
    return fd;
}

static struct inhibitor **find_window(void *window)
{
    struct inhibitor **entry = &windows;
    while (*entry && (*entry)->window != window) {
        entry = &(*entry)->next;
    }
    return entry;
}

__attribute__((visibility("default"))) void ADD_INHIBITOR(void *input, void *window)
{
    static inhibitor_fn original;
    if (!original) {
        original = (inhibitor_fn)dlsym(RTLD_NEXT, STRINGIFY(ADD_INHIBITOR));
    }
    if (!original) {
        return;
    }
    if (!*find_window(window)) {
        struct inhibitor *entry = malloc(sizeof(*entry));
        if (entry) {
            *entry = (struct inhibitor){ .window = window, .next = windows };
            windows = entry;
        }
    }
    original(input, window);
    if (windows && lease < 0) {
        lease = open_lease();
    }
}

__attribute__((visibility("default"))) void REMOVE_INHIBITOR(void *input, void *window)
{
    static inhibitor_fn original;
    if (!original) {
        original = (inhibitor_fn)dlsym(RTLD_NEXT, STRINGIFY(REMOVE_INHIBITOR));
    }
    if (!original) {
        return;
    }
    struct inhibitor **entry = find_window(window);
    if (*entry) {
        struct inhibitor *removed = *entry;
        *entry = removed->next;
        free(removed);
    }
    original(input, window);
    if (!windows && lease >= 0) {
        close(lease);
        lease = -1;
    }
}
