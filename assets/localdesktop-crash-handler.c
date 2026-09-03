// Best-effort in-process crash capture for KWin under Android/PRoot.
//
// Nested gdb can be denied by the host tracer before it sees the guest
// process. This preload runs inside that process instead, records the fault
// registers and a glibc backtrace, then re-raises the original signal so the
// wrapper still observes the real exit status.
#define _GNU_SOURCE

#include <dlfcn.h>
#include <errno.h>
#include <execinfo.h>
#include <fcntl.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <ucontext.h>
#include <unistd.h>

#ifndef AT_EMPTY_PATH
#define AT_EMPTY_PATH 0x1000
#endif

int fstat(int fd, struct stat *buf) {
    static int (*real_fstat)(int, struct stat *) = NULL;
    if (real_fstat == NULL) {
        real_fstat = (int (*)(int, struct stat *))dlsym(RTLD_NEXT, "fstat");
    }
    int ret = real_fstat ? real_fstat(fd, buf) : -1;
    if (ret < 0 && errno == ENOENT) {
#if defined(SYS_newfstatat)
        ret = syscall(SYS_newfstatat, fd, "", buf, AT_EMPTY_PATH);
#elif defined(SYS_fstatat64)
        ret = syscall(SYS_fstatat64, fd, "", buf, AT_EMPTY_PATH);
#endif
        if (ret == 0) {
            errno = 0;
        }
    }
    return ret;
}

int fstat64(int fd, struct stat64 *buf) {
    static int (*real_fstat64)(int, struct stat64 *) = NULL;
    if (real_fstat64 == NULL) {
        real_fstat64 = (int (*)(int, struct stat64 *))dlsym(RTLD_NEXT, "fstat64");
    }
    int ret = real_fstat64 ? real_fstat64(fd, buf) : -1;
    if (ret < 0 && errno == ENOENT) {
#if defined(SYS_newfstatat)
        ret = syscall(SYS_newfstatat, fd, "", buf, AT_EMPTY_PATH);
#elif defined(SYS_fstatat64)
        ret = syscall(SYS_fstatat64, fd, "", buf, AT_EMPTY_PATH);
#endif
        if (ret == 0) {
            errno = 0;
        }
    }
    return ret;
}

static char crash_path[512] = "/tmp/localdesktop-kwin-backtrace.log";
static char attempt_id[128] = "unknown";
static volatile sig_atomic_t handling_crash;

static void write_all(int fd, const char *text, size_t length) {
    while (length > 0) {
        ssize_t written = write(fd, text, length);
        if (written <= 0) {
            if (written < 0 && errno == EINTR) {
                continue;
            }
            return;
        }
        text += written;
        length -= (size_t)written;
    }
}

static void write_text(int fd, const char *text) {
    if (text != NULL) {
        write_all(fd, text, strlen(text));
    }
}

static void write_unsigned(int fd, uintptr_t value) {
    char digits[2 + sizeof(uintptr_t) * 2];
    size_t index = sizeof(digits);
    const char *hex = "0123456789abcdef";
    do {
        digits[--index] = hex[value & 0xfu];
        value >>= 4;
    } while (value != 0);
    write_all(fd, &digits[index], sizeof(digits) - index);
}

static void write_decimal(int fd, unsigned long value) {
    char digits[3 * sizeof(unsigned long) + 1];
    size_t index = sizeof(digits);
    do {
        digits[--index] = (char)('0' + (value % 10u));
        value /= 10u;
    } while (value != 0);
    write_all(fd, &digits[index], sizeof(digits) - index);
}

static void write_pointer(int fd, const char *label, uintptr_t value) {
    write_text(fd, label);
    write_text(fd, "=0x");
    write_unsigned(fd, value);
    write_text(fd, "\n");
}

static void write_symbol_details(int fd, const char *label, uintptr_t address) {
    Dl_info info;
    if (address != 0 && dladdr((void *)address, &info) != 0) {
        write_text(fd, label);
        write_text(fd, "_object=");
        write_text(fd, info.dli_fname);
        if (info.dli_sname != NULL) {
            write_text(fd, " symbol=");
            write_text(fd, info.dli_sname);
        }
        write_text(fd, "\n");
    }
}

static void write_maps(int fd) {
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

static void write_start_marker(void) {
    int fd = open(crash_path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0644);
    if (fd < 0) {
        return;
    }
    write_text(fd, "localdesktop-crash-handler-start pid=");
    write_decimal(fd, (unsigned long)getpid());
    write_text(fd, " attempt=");
    write_text(fd, attempt_id);
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

static void crash_handler(int signal_number, siginfo_t *info, void *raw_context) {
    if (handling_crash) {
        _exit(128 + signal_number);
    }
    handling_crash = 1;

    int fd = open(crash_path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0644);
    if (fd < 0) {
        fd = STDERR_FILENO;
    }
    write_text(fd, "localdesktop-crash-handler signal=");
    write_decimal(fd, (unsigned long)signal_number);
    write_text(fd, " pid=");
    write_decimal(fd, (unsigned long)getpid());
    write_text(fd, " attempt=");
    write_text(fd, attempt_id);
    write_text(fd, "\n");
    if (info != NULL) {
        write_pointer(fd, "fault_address", (uintptr_t)info->si_addr);
    }

    ucontext_t *context = (ucontext_t *)raw_context;
#if defined(__aarch64__)
    if (context != NULL) {
        const uintptr_t pc = (uintptr_t)context->uc_mcontext.pc;
        const uintptr_t lr = (uintptr_t)context->uc_mcontext.regs[30];
        const uintptr_t sp = (uintptr_t)context->uc_mcontext.sp;
        // Write raw register values before calling dladdr/backtrace. Those
        // helpers may allocate or consult partially-unwound loader state.
        write_pointer(fd, "pc", pc);
        write_pointer(fd, "lr", lr);
        write_pointer(fd, "sp", sp);
        write_maps(fd);
        write_symbol_details(fd, "pc", pc);
        write_symbol_details(fd, "lr", lr);
    } else {
        write_text(fd, "registers=unavailable\n");
        write_maps(fd);
    }
#elif defined(__x86_64__)
    if (context != NULL) {
        const uintptr_t pc = (uintptr_t)context->uc_mcontext.gregs[REG_RIP];
        const uintptr_t lr = (uintptr_t)context->uc_mcontext.gregs[REG_RBP];
        const uintptr_t sp = (uintptr_t)context->uc_mcontext.gregs[REG_RSP];
        write_pointer(fd, "pc", pc);
        write_pointer(fd, "lr", lr);
        write_pointer(fd, "sp", sp);
        write_maps(fd);
        write_symbol_details(fd, "pc", pc);
        write_symbol_details(fd, "lr", lr);
    } else {
        write_text(fd, "registers=unavailable\n");
        write_maps(fd);
    }
#elif defined(__arm__)
    if (context != NULL) {
        const uintptr_t pc = (uintptr_t)context->uc_mcontext.arm_pc;
        const uintptr_t lr = (uintptr_t)context->uc_mcontext.arm_lr;
        const uintptr_t sp = (uintptr_t)context->uc_mcontext.arm_sp;
        write_pointer(fd, "pc", pc);
        write_pointer(fd, "lr", lr);
        write_pointer(fd, "sp", sp);
        write_maps(fd);
        write_symbol_details(fd, "pc", pc);
        write_symbol_details(fd, "lr", lr);
    } else {
        write_text(fd, "registers=unavailable\n");
        write_maps(fd);
    }
#else
    write_text(fd, "registers=unsupported-architecture\n");
    write_maps(fd);
#endif

    void *frames[64];
    int frame_count = backtrace(frames, (int)(sizeof(frames) / sizeof(frames[0])));
    write_text(fd, "backtrace_begin\n");
    if (frame_count > 0) {
        backtrace_symbols_fd(frames, frame_count, fd);
    }
    write_text(fd, "backtrace_end\n");
    if (fd != STDERR_FILENO) {
        close(fd);
    }

    signal(signal_number, SIG_DFL);
    sigset_t unblock;
    sigemptyset(&unblock);
    sigaddset(&unblock, signal_number);
    sigprocmask(SIG_UNBLOCK, &unblock, NULL);
    raise(signal_number);
    _exit(128 + signal_number);
}

__attribute__((constructor)) static void install_handlers(void) {
    const char *configured_path = getenv("LOCALDESKTOP_CRASH_LOG");
    if (configured_path != NULL && configured_path[0] != '\0') {
        strncpy(crash_path, configured_path, sizeof(crash_path) - 1);
        crash_path[sizeof(crash_path) - 1] = '\0';
    }
    const char *configured_attempt = getenv("LOCALDESKTOP_ATTEMPT_ID");
    if (configured_attempt != NULL && configured_attempt[0] != '\0') {
        strncpy(attempt_id, configured_attempt, sizeof(attempt_id) - 1);
        attempt_id[sizeof(attempt_id) - 1] = '\0';
    }
    write_start_marker();

    struct sigaction action;
    memset(&action, 0, sizeof(action));
    sigemptyset(&action.sa_mask);
    action.sa_sigaction = crash_handler;
    action.sa_flags = SA_SIGINFO | SA_RESETHAND;
    sigaction(SIGSEGV, &action, NULL);
    sigaction(SIGABRT, &action, NULL);
    sigaction(SIGBUS, &action, NULL);
    sigaction(SIGILL, &action, NULL);
    sigaction(SIGFPE, &action, NULL);
}
