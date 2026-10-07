/*
 * Managed by Portal: lets iputils `ping` start inside Android's app sandbox.
 *
 * ping calls prctl(PR_SET_KEEPCAPS, 1) before dropping capabilities and
 * exits on failure. Android locks the securebits of every app process
 * (SECBIT_KEEP_CAPS_LOCKED), so that call fails with EPERM even though ping
 * holds no capabilities to keep: it then pings through an unprivileged ICMP
 * socket, which Android allows. This preload, used only by Portal's
 * /usr/local/bin/ping wrapper, makes that one request succeed without
 * effect and passes every other prctl through.
 *
 * Build (guest-native ARM64 glibc, no libc at link time):
 *   clang --target=aarch64-linux-gnu -shared -fPIC -nostdlib -O2 \
 *       -fuse-ld=lld -o ping-keepcaps.so ping-keepcaps.c
 */
#define PR_SET_KEEPCAPS 8

extern long syscall(long number, ...);

int prctl(int option, unsigned long arg2, unsigned long arg3, unsigned long arg4,
          unsigned long arg5) {
    if (option == PR_SET_KEEPCAPS)
        return 0;
    /* __NR_prctl on AArch64; glibc's syscall() sets errno. */
    return (int)syscall(167, option, arg2, arg3, arg4, arg5);
}
