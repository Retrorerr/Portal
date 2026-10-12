/*
 * Managed by Portal: lets Wine make patched DLL pages executable.
 *
 * Wine maps a PE image from its file (MAP_PRIVATE), applies relocations and
 * hybrid patches to the pages, then mprotects them to PROT_EXEC. On a file
 * that has dirty private pages this needs SELinux execmod, which Android
 * never grants to apps, so mprotect fails with EACCES ("noexec filesystem?"
 * in Wine's log) and the first call into ntdll.dll crashes. Every Proton
 * game dies about a second after launch, before it shows a window.
 *
 * When mprotect asks for PROT_EXEC on a private mapping and gets EACCES,
 * this preload copies the range into anonymous memory at the same address
 * (contents unchanged) and retries. Anonymous memory may become executable.
 * Shared mappings and every other error are left alone.
 *
 * Build (guest-native ARM64 glibc, no libc at link time):
 *   clang --target=aarch64-linux-gnu -shared -fPIC -nostdlib -O2 \
 *       -fuse-ld=lld -ffreestanding -fno-stack-protector \
 *       -o portal-execmod.so portal-execmod.c
 */
#include <stddef.h>
#include <stdint.h>

#define PROT_READ 1
#define PROT_WRITE 2
#define PROT_EXEC 4
#define MAP_PRIVATE 0x02
#define MAP_FIXED 0x10
#define MAP_ANONYMOUS 0x20
#define MAP_FAILED ((void *)-1)
#define EACCES 13
#define RTLD_NEXT ((void *)-1l)
#define O_RDONLY 0

extern void *dlsym(void *handle, const char *symbol);
extern int *__errno_location(void);
extern void *mmap(void *addr, size_t len, int prot, int flags, int fd, long off);
extern int munmap(void *addr, size_t len);
extern int open(const char *path, int flags, ...);
extern long read(int fd, void *buf, size_t len);
extern int close(int fd);
extern void *memcpy(void *dst, const void *src, size_t len);

typedef int (*mprotect_fn)(void *, size_t, int);

static mprotect_fn real_mprotect(void) {
    static mprotect_fn fn;
    if (!fn) fn = (mprotect_fn)dlsym(RTLD_NEXT, "mprotect");
    return fn;
}

static uintptr_t parse_hex(const char **p) {
    uintptr_t v = 0;
    for (;; (*p)++) {
        char c = **p;
        if (c >= '0' && c <= '9') v = v * 16 + (uintptr_t)(c - '0');
        else if (c >= 'a' && c <= 'f') v = v * 16 + (uintptr_t)(c - 'a' + 10);
        else return v;
    }
}

/* True when every page of [start, end) lies in private mappings. */
static int range_is_private(uintptr_t start, uintptr_t end) {
    char buf[4096], line[512];
    size_t used = 0;
    long n;
    uintptr_t covered = start;
    int fd = open("/proc/self/maps", O_RDONLY);
    if (fd < 0) return 0;
    while ((n = read(fd, buf, sizeof buf)) > 0) {
        for (long i = 0; i < n; i++) {
            if (buf[i] != '\n') {
                if (used < sizeof line - 1) line[used++] = buf[i];
                continue;
            }
            line[used] = 0;
            used = 0;
            const char *p = line;
            uintptr_t lo = parse_hex(&p);
            if (*p++ != '-') continue;
            uintptr_t hi = parse_hex(&p);
            if (*p++ != ' ' || p[0] == 0 || p[1] == 0 || p[2] == 0) continue;
            if (hi <= covered || lo > covered) continue;
            if (p[3] != 'p') { close(fd); return 0; }
            covered = hi;
            if (covered >= end) { close(fd); return 1; }
        }
    }
    close(fd);
    return 0;
}

int mprotect(void *addr, size_t len, int prot) {
    mprotect_fn real = real_mprotect();
    int ret = real(addr, len, prot);
    if (ret == 0 || !(prot & PROT_EXEC) || *__errno_location() != EACCES || len == 0)
        return ret;
    uintptr_t start = (uintptr_t)addr;
    if (!range_is_private(start, start + len)) {
        *__errno_location() = EACCES;
        return -1;
    }
    void *copy = mmap(0, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (copy == MAP_FAILED) {
        *__errno_location() = EACCES;
        return -1;
    }
    /* Pages may be PROT_NONE or write-only; reading needs PROT_READ. */
    if (real(addr, len, PROT_READ) != 0) {
        munmap(copy, len);
        *__errno_location() = EACCES;
        return -1;
    }
    memcpy(copy, addr, len);
    if (mmap(addr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0) != addr) {
        munmap(copy, len);
        *__errno_location() = EACCES;
        return -1;
    }
    memcpy(addr, copy, len);
    munmap(copy, len);
    return real(addr, len, prot);
}
