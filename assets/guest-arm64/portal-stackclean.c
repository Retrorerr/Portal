/*
 * Managed by Portal: gives every process a clean stack below main().
 *
 * The dynamic loader leaves its search paths and ld.so.cache fragments in
 * the dead stack below the point where main() starts. Some programs read
 * that memory without initialising it: Steam's steam-launch-wrapper calls
 * readlink("/proc/self/exe") into an uninitialised 4 KiB buffer, ignores
 * the length, and runs strrchr on the result. With three or more preloaded
 * libraries the leftovers sit in that buffer, the wrapper computes a bogus
 * "pw-audio-namespace" path and exits 255 before the game starts.
 *
 * Constructors run after the loader has finished and before main(), so
 * this one zeroes the dead stack the loader used. Main thread only: other
 * threads may have small stacks.
 *
 * Build (guest-native ARM64 glibc, no libc at link time):
 *   clang --target=aarch64-linux-gnu -shared -fPIC -nostdlib -O2 \
 *       -fuse-ld=lld -ffreestanding -fno-stack-protector \
 *       -o portal-stackclean.so portal-stackclean.c
 */
#define CLEAN_BYTES (24 * 1024)
#define GUARD_BYTES 512

static long raw_syscall0(long nr)
{
    register long x8 __asm__("x8") = nr;
    register long x0 __asm__("x0");
    __asm__ volatile("svc 0" : "=r"(x0) : "r"(x8) : "memory");
    return x0;
}

__attribute__((constructor(101))) static void portal_stackclean(void)
{
    /* getpid == gettid only on a process's first thread. */
    if (raw_syscall0(172) != raw_syscall0(178))
        return;
    unsigned long sp;
    __asm__ volatile("mov %0, sp" : "=r"(sp));
    unsigned long hi = (sp - GUARD_BYTES) & ~15UL;
    unsigned long lo = hi - CLEAN_BYTES;
    __asm__ volatile(
        "1: stp xzr, xzr, [%0], #16\n"
        "   cmp %0, %1\n"
        "   b.lo 1b\n"
        : "+r"(lo)
        : "r"(hi)
        : "memory", "cc");
}
