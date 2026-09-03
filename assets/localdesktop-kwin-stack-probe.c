// Diagnostic-only in-process SIGUSR2 stack capture for the real kwin_wayland
// process. This preload installs SIGUSR2 only; it does not replace the
// production crash handler's SIGSEGV/SIGABRT behavior.
#define _GNU_SOURCE

#include <errno.h>
#include <execinfo.h>
#include <fcntl.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <ucontext.h>
#include <unistd.h>

static char stack_path[512] = "/var/lib/localdesktop/kwin-stack.log";
static volatile sig_atomic_t capture_started;

static void write_all(int fd, const char *text, size_t length)
{
    while (length > 0) {
        ssize_t written = write(fd, text, length);
        if (written > 0) {
            text += written;
            length -= (size_t)written;
        } else if (written < 0 && errno == EINTR) {
            continue;
        } else {
            return;
        }
    }
}

static void write_text(int fd, const char *text)
{
    if (text != NULL) {
        write_all(fd, text, strlen(text));
    }
}

static void write_decimal(int fd, unsigned long value)
{
    char digits[3 * sizeof(unsigned long) + 1];
    size_t index = sizeof(digits);
    do {
        digits[--index] = (char)('0' + (value % 10u));
        value /= 10u;
    } while (value != 0);
    write_all(fd, &digits[index], sizeof(digits) - index);
}

static void write_hex(int fd, uintptr_t value)
{
    char digits[2 * sizeof(uintptr_t)];
    size_t index = sizeof(digits);
    const char *hex = "0123456789abcdef";
    do {
        digits[--index] = hex[value & 0xfu];
        value >>= 4;
    } while (value != 0);
    write_all(fd, &digits[index], sizeof(digits) - index);
}

static void write_pointer(int fd, const char *name, uintptr_t value)
{
    write_text(fd, name);
    write_text(fd, "=0x");
    write_hex(fd, value);
    write_text(fd, "\n");
}

static void write_maps(int fd)
{
    int maps = open("/proc/self/maps", O_RDONLY | O_CLOEXEC);
    if (maps < 0) {
        write_text(fd, "maps=unavailable\n");
        return;
    }
    write_text(fd, "maps_begin\n");
    char buffer[4096];
    for (;;) {
        ssize_t length = read(maps, buffer, sizeof(buffer));
        if (length <= 0) {
            break;
        }
        write_all(fd, buffer, (size_t)length);
    }
    write_text(fd, "maps_end\n");
    close(maps);
}

static void write_context(int fd, void *raw_context)
{
    ucontext_t *context = (ucontext_t *)raw_context;
#if defined(__aarch64__)
    if (context != NULL) {
        write_pointer(fd, "pc", (uintptr_t)context->uc_mcontext.pc);
        write_pointer(fd, "lr", (uintptr_t)context->uc_mcontext.regs[30]);
        write_pointer(fd, "sp", (uintptr_t)context->uc_mcontext.sp);
    } else {
        write_text(fd, "registers=unavailable\n");
    }
#elif defined(__x86_64__)
    if (context != NULL) {
        write_pointer(fd, "pc", (uintptr_t)context->uc_mcontext.gregs[REG_RIP]);
        write_pointer(fd, "sp", (uintptr_t)context->uc_mcontext.gregs[REG_RSP]);
    } else {
        write_text(fd, "registers=unavailable\n");
    }
#else
    (void)context;
    write_text(fd, "registers=unsupported-architecture\n");
#endif
}

static void capture_signal(int signal_number, siginfo_t *info, void *raw_context)
{
    const int saved_errno = errno;
    if (capture_started) {
        errno = saved_errno;
        return;
    }
    capture_started = 1;

    int fd = open(stack_path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0644);
    if (fd < 0) {
        capture_started = 0;
        errno = saved_errno;
        return;
    }
    write_text(fd, "kwin-stack-probe signal=");
    write_decimal(fd, (unsigned long)signal_number);
    write_text(fd, " pid=");
    write_decimal(fd, (unsigned long)getpid());
    if (info != NULL) {
        write_text(fd, " si_code=");
        write_decimal(fd, (unsigned long)info->si_code);
    }
    write_text(fd, "\n");
    write_context(fd, raw_context);
    write_maps(fd);

    // Best effort only: raw registers/maps are written before backtrace
    // unwinding in case the loader or allocator cannot be safely re-entered.
    void *frames[96];
    int frame_count = backtrace(frames, (int)(sizeof(frames) / sizeof(frames[0])));
    write_text(fd, "backtrace_begin\n");
    if (frame_count > 0) {
        backtrace_symbols_fd(frames, frame_count, fd);
    }
    write_text(fd, "backtrace_end\n");
    close(fd);
    errno = saved_errno;
}

static void write_start_marker(void)
{
    int fd = open(stack_path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0644);
    if (fd < 0) {
        return;
    }
    write_text(fd, "kwin-stack-probe-start role=exec pid=");
    write_decimal(fd, (unsigned long)getpid());
    write_text(fd, " nonce=");
    const char *nonce = getenv("LOCALDESKTOP_KWIN_STACK_NONCE");
    if (nonce != NULL) {
        write_text(fd, nonce);
    } else {
        write_text(fd, "unset");
    }
    write_text(fd, " exe=");
    char executable[256];
    ssize_t length = readlink("/proc/self/exe", executable, sizeof(executable) - 1);
    if (length > 0) {
        write_all(fd, executable, (size_t)length);
    } else {
        write_text(fd, "unavailable");
    }
    write_text(fd, "\n");
    close(fd);
}

__attribute__((constructor)) static void install_probe(void)
{
    const char *configured_path = getenv("LOCALDESKTOP_KWIN_STACK_LOG");
    if (configured_path != NULL && configured_path[0] != '\0') {
        strncpy(stack_path, configured_path, sizeof(stack_path) - 1);
        stack_path[sizeof(stack_path) - 1] = '\0';
    }

    struct sigaction action;
    memset(&action, 0, sizeof(action));
    sigemptyset(&action.sa_mask);
    action.sa_sigaction = capture_signal;
    action.sa_flags = SA_SIGINFO | SA_RESTART;
    if (sigaction(SIGUSR2, &action, NULL) == 0) {
        write_start_marker();
    }
}
