/* Runtime-control syscall handlers — guest-side denials and shadow
 * virtualization for operations that would otherwise compromise
 * tawcroot's own invariants.
 *
 * Surface (notes/tawcroot/sigsys-handler.md §"Guest signal/seccomp control"):
 *   - Guest seccomp filters: accept without installing another filter.
 *     This compatibility runtime provides no guest sandbox isolation.
 *   - `rt_sigaction(SIGSYS, ...)`: virtualize. The guest's intended
 *     disposition lives in a shadow buffer; reads/writes of SIGSYS
 *     hit the shadow and never the kernel. The real kernel disposition
 *     stays our SIGSYS handler.
 *   - `rt_sigprocmask`: SIGSYS is reserved and cannot be blocked.
 *     Strip it from requested masks and report the actual kernel mask.
 *     No per-thread shadow or thread-exit bookkeeping is needed.
 *   - `sigaltstack`: native; loader auxv advertises the stack budget.
 *   - Other signals are unaffected.
 */

#include <stddef.h>
#include <stdint.h>
#include <ucontext.h>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <sys/mount.h>
#include "path.h"
#include "path_scratch.h"
#include "io.h"
#include "tawc_string.h"
#include "tawc_uapi.h"

#include "dispatch.h"
#include "errno_neg.h"
#include "identity.h"
#include "raw_sys.h"
#include "signal_shadow.h"
#include "syscalls_control.h"
#include "sysnr.h"
#include "usercopy.h"

#ifndef PR_GET_SECCOMP
# define PR_GET_SECCOMP 21
#endif
#ifndef PR_SET_SECCOMP
# define PR_SET_SECCOMP 22
#endif

#ifndef SIGSYS
# define SIGSYS 31
#endif
/* sigset bit position is (signo - 1). */
#define SIGSYS_BIT (1ULL << (SIGSYS - 1))

#ifndef SIG_BLOCK
# define SIG_BLOCK   0
# define SIG_UNBLOCK 1
# define SIG_SETMASK 2
#endif

/* Compatibility only: never stack guest BPF over our SIGSYS routing.
 * Validate readable storage, not BPF semantics. Android's filter remains
 * active; no application filesystem/network policy is enforced here. */
static long accept_guest_filter(long pointer)
{
	struct sock_fprog program;
	if (tawc_copy_from_guest(&program, sizeof program, (void *)(uintptr_t)pointer) < 0)
		return TAWC_EFAULT;
	if (!program.len || program.len > 4096) return TAWC_EINVAL;
	struct sock_filter instruction;
	for (unsigned i = 0; i < program.len; i++)
		if (tawc_copy_from_guest(&instruction, sizeof instruction, program.filter + i) < 0)
			return TAWC_EFAULT;
	return 0;
}

static long handle_seccomp(const tawcroot_syscall_args *args, ucontext_t *uc)
{
	(void)uc;
	if (args->a == SECCOMP_SET_MODE_FILTER) {
		/* NULL is a capability probe, not a filter installation. We do
		 * not provide BPF enforcement; do not advertise that capability. */
		if (!args->c) return TAWC_ENOSYS;
		/* NEW_LISTENER requires a real notification fd, which we cannot supply. */
		if (args->b & ~SECCOMP_FILTER_FLAG_TSYNC) return TAWC_EINVAL;
		return accept_guest_filter(args->c);
	}
	return TAWC_ENOSYS;
}

static long handle_prctl(const tawcroot_syscall_args *args, ucontext_t *uc)
{
	(void)uc;
	int op = (int)args->a;
	if (op == PR_SET_SECCOMP)
		return args->b == SECCOMP_MODE_FILTER && args->c ? accept_guest_filter(args->c) : TAWC_EINVAL;
	if (op == 36 /* PR_GET_KEEPCAPS */)
		return tawcroot_identity_keepcaps();
	long rv = TAWC_RAW(TAWC_SYS_prctl, args->a, args->b, args->c,
			   args->d, args->e, 0);
	/* Android refuses these to apps; fake root answers them like
	 * the kernel answers root (see identity.c). */
	if (op == 8 /* PR_SET_KEEPCAPS */ &&
	    (rv == 0 || (rv == TAWC_EPERM && args->b <= 1))) {
		tawcroot_identity_set_keepcaps((int)args->b);
		return 0;
	}
	if ((op == 24 /* PR_CAPBSET_DROP */ || op == 28 /* PR_SET_SECUREBITS */) &&
	    rv == TAWC_EPERM && tawcroot_identity_fake_caps())
		return 0;
	return rv;
}

/* Android omits robust-list syscalls from the app allowlist. Keep the guest's
 * actual registration for libraries which inspect their own pthread list.
 * This is registration/query compatibility, NOT kernel owner-death recovery.
 * Native kernels retain their complete implementation. No libc TLS layout or
 * fabricated list is involved; exit(2) stays untrapped for musl stack safety. */
#define ROBUST_SLOTS 4096
static struct { uint64_t owner; uintptr_t head; } robust_lists[ROBUST_SLOTS];

static uint64_t robust_owner(void)
{
	uint64_t pid = (uint32_t)TAWC_RAW(TAWC_SYS_getpid, 0, 0, 0, 0, 0, 0);
	uint64_t tid = (uint32_t)TAWC_RAW(TAWC_SYS_gettid, 0, 0, 0, 0, 0, 0);
	return (pid << 32) | tid;
}

static long handle_set_robust_list(const tawcroot_syscall_args *args, ucontext_t *uc)
{
	(void)uc;
	if (args->b != 3 * (long)sizeof(uintptr_t)) return TAWC_EINVAL;
	long result = TAWC_RAW(TAWC_SYS_set_robust_list, args->a, args->b, 0, 0, 0, 0);
	if (result != TAWC_ENOSYS) return result;
	uint64_t owner = robust_owner();
	/* A registration overwrites any previous registration for this thread,
	 * including a reused tid. Reap dead registrations only on exhaustion. */
	for (int pass = 0; pass < 2; pass++) {
		for (unsigned i = 0; i < ROBUST_SLOTS; i++) {
			unsigned slot = ((uint32_t)owner + i) % ROBUST_SLOTS;
			uint64_t found = __atomic_load_n(&robust_lists[slot].owner, __ATOMIC_ACQUIRE);
			if (found != owner) {
				if (pass && found && found != UINT64_MAX &&
				    TAWC_RAW(TAWC_SYS_tgkill, found >> 32, (uint32_t)found,
				             0, 0, 0, 0) == TAWC_ESRCH) {
					__atomic_compare_exchange_n(&robust_lists[slot].owner, &found,
					                            0, 0, __ATOMIC_ACQ_REL, __ATOMIC_RELAXED);
					found = __atomic_load_n(&robust_lists[slot].owner, __ATOMIC_ACQUIRE);
				}
				if (found || !__atomic_compare_exchange_n(&robust_lists[slot].owner,
				        &found, UINT64_MAX, 0, __ATOMIC_ACQ_REL, __ATOMIC_RELAXED)) continue;
			}
			__atomic_store_n(&robust_lists[slot].head, (uintptr_t)args->a, __ATOMIC_RELAXED);
			__atomic_store_n(&robust_lists[slot].owner, owner, __ATOMIC_RELEASE);
			return 0;
		}
	}
	return TAWC_ENOMEM;
}

static long handle_get_robust_list(const tawcroot_syscall_args *args, ucontext_t *uc)
{
	(void)uc;
	long result = TAWC_RAW(TAWC_SYS_get_robust_list, args->a, args->b, args->c, 0, 0, 0);
	if (result != TAWC_ENOSYS) return result;
	uint64_t owner = robust_owner();
	if ((int)args->a && (uint32_t)args->a != (uint32_t)owner) return TAWC_EPERM;
	uintptr_t head = 0;
	size_t size = 3 * sizeof(uintptr_t);
	for (unsigned i = 0; i < ROBUST_SLOTS; i++) {
		unsigned slot = ((uint32_t)owner + i) % ROBUST_SLOTS;
		if (__atomic_load_n(&robust_lists[slot].owner, __ATOMIC_ACQUIRE) == owner) {
			head = __atomic_load_n(&robust_lists[slot].head, __ATOMIC_RELAXED);
			break;
		}
	}
	if (tawc_copy_to_guest((void *)args->c, &size, sizeof size) < 0 ||
	    tawc_copy_to_guest((void *)args->b, &head, sizeof head) < 0) return TAWC_EFAULT;
	return 0;
}

static long handle_rt_sigaction(const tawcroot_syscall_args *args,
				ucontext_t *uc)
{
	(void)uc;
	int    sig         = (int)args->a;
	const void *act    = (const void *)(uintptr_t)args->b;
	void  *oldact      = (void *)(uintptr_t)args->c;
	size_t sigsetsize  = (size_t)args->d;

	if (sig != SIGSYS) {
		/* A handler may invoke translated syscalls, including async-signal-safe
		 * write(). Its sa_mask must not block our reserved trap signal. This
		 * is separate from rt_sigprocmask's thread-mask virtualization. */
		uint64_t action[TAWC_KERN_SIGACTION_SIZE / sizeof(uint64_t)];
		if (act && sigsetsize == 8) {
			long e = tawc_copy_from_guest(action, sizeof action, act);
			if (e < 0) return e;
			action[3] &= ~SIGSYS_BIT;
			act = action;
		}
		return TAWC_RAW(TAWC_SYS_rt_sigaction, args->a, (long)act,
				args->c, args->d, 0, 0);
	}

	if (sigsetsize != 8) return TAWC_EINVAL;

	/* Read the guest's new action into a stack-local buffer FIRST so
	 * we know whether the call would have succeeded before exposing
	 * stale shadow contents to a faulting oldact write. */
	unsigned char incoming[TAWC_KERN_SIGACTION_SIZE];
	int have_incoming = 0;
	if (act) {
		long e = tawc_copy_from_guest(incoming, sizeof incoming, act);
		if (e < 0) return TAWC_EFAULT;
		have_incoming = 1;
	}

	if (oldact) {
		unsigned char snap[TAWC_KERN_SIGACTION_SIZE];
		tawc_sigshadow_action_get(snap);
		long e = tawc_copy_to_guest(oldact, snap, sizeof snap);
		if (e < 0) return TAWC_EFAULT;
	}

	if (have_incoming)
		tawc_sigshadow_action_set(incoming);
	return 0;
}

/* Locate the 8-byte kernel sigset embedded in ucontext_t->uc_sigmask.
 * Bionic's <ucontext.h> exposes it as `sigset64_t` whose first member
 * is a `__bionic_sigset_t __bits` of 8 bytes; we treat it as a u64.
 * Modifying *here* is what makes the change persist across sigreturn —
 * a kernel-level rt_sigprocmask issued during a SIGSYS handler is
 * *undone* by sigreturn restoring task->blocked from this field. */
static uint64_t *uc_sigmask_word(ucontext_t *uc)
{
	return (uint64_t *)&uc->uc_sigmask;
}

/* Update the saved kernel mask so sigreturn preserves the change.
 * Copy input before output for aliasing, and roll back on output EFAULT. */
static long handle_rt_sigprocmask(const tawcroot_syscall_args *args,
				  ucontext_t *uc)
{
	int    how         = (int)args->a;
	const void *guest_set    = (const void *)(uintptr_t)args->b;
	void  *guest_oldset      = (void *)(uintptr_t)args->c;
	size_t sigsetsize  = (size_t)args->d;

	if (sigsetsize != 8) return TAWC_EINVAL;
	/* Kernel no-op semantics also ignore how when set is NULL. */
	if (!guest_set && !guest_oldset) return 0;

	uint64_t set_val = 0;
	int have_set = 0;
	if (guest_set) {
		long e = tawc_copy_from_guest(&set_val, 8, guest_set);
		if (e < 0) return TAWC_EFAULT;
		have_set = 1;
	}

	uint64_t *kmask = uc_sigmask_word(uc);
	uint64_t  cur_kmask = *kmask & ~SIGSYS_BIT;

	if (have_set) {
		uint64_t kernel_set = set_val & ~SIGSYS_BIT;

		switch (how) {
		case SIG_BLOCK:
			*kmask = cur_kmask | kernel_set;
			break;
		case SIG_UNBLOCK:
			*kmask = cur_kmask & ~kernel_set;
			break;
		case SIG_SETMASK:
			*kmask = kernel_set;
			break;
		default:
			return TAWC_EINVAL;
		}
	}

	if (guest_oldset) {
		uint64_t old = cur_kmask;
		long e = tawc_copy_to_guest(guest_oldset, &old, 8);
		if (e < 0) {
			/* A failed output copy must not change the mask. */
			if (have_set) *kmask = cur_kmask;
			return TAWC_EFAULT;
		}
	}
	return 0;
}

#if defined(__x86_64__)
/* x86_64 glibc's getpgrp(3) issues the legacy getpgrp syscall, which
 * Android's untrusted_app filter RET_TRAPs (bionic only ever calls
 * getpgid). Without this handler the -ENOSYS fallthrough reaches
 * bash's job-control init as a garbage process group and interactive
 * shells on a pty print "initialize_job_control: no job control in
 * background" and run with job control off. aarch64 never allocated a
 * getpgrp number — glibc wraps getpgid(0) there — so this is
 * emulator-only. */
static long handle_getpgrp(const tawcroot_syscall_args *args, ucontext_t *uc)
{
	(void)args;
	(void)uc;
	return TAWC_RAW(TAWC_SYS_getpgid, 0, 0, 0, 0, 0, 0);
}

/* time(tloc) → clock_gettime(CLOCK_REALTIME). Legacy time(2) is
 * RET_TRAPped by the real emulator filter (empirical audit:
 * notes/tawcroot/status.md); clock_gettime is allowlisted. Return
 * tv_sec, and mirror it into *tloc when non-NULL (via the guarded
 * copy — tloc is an untrusted guest pointer). */
static long handle_time(const tawcroot_syscall_args *args, ucontext_t *uc)
{
	(void)uc;
	struct { long tv_sec; long tv_nsec; } ts;
	long r = TAWC_RAW(TAWC_SYS_clock_gettime, 0 /*CLOCK_REALTIME*/,
	                  (long)&ts, 0, 0, 0, 0);
	if (r < 0) return r;
	if (args->a != 0) {
		long cr = tawc_copy_to_guest((void *)args->a, &ts.tv_sec,
		                             sizeof ts.tv_sec);
		if (cr < 0) return cr;
	}
	return ts.tv_sec;
}

/* alarm(seconds) → setitimer(ITIMER_REAL). glibc's alarm(3) already
 * routes through setitimer so this only fires for programs issuing the
 * raw legacy NR, but the real filter RET_TRAPs it, so without this it
 * -ENOSYSes. Contract: arm a one-shot ITIMER_REAL for `seconds` (0
 * disarms) and return the whole seconds left on the previous timer,
 * rounding a partial second up as the man page specifies. */
static long handle_alarm(const tawcroot_syscall_args *args, ucontext_t *uc)
{
	(void)uc;
	struct itv { long sec; long usec; };
	/* seconds is unsigned int in the kernel ABI — only the low 32
	 * register bits are meaningful. */
	struct { struct itv interval; struct itv value; } nv = {
		{ 0, 0 }, { (long)(unsigned int)args->a, 0 },
	}, ov;
	long r = TAWC_RAW(TAWC_SYS_setitimer, 0 /*ITIMER_REAL*/,
	                  (long)&nv, (long)&ov, 0, 0, 0);
	if (r < 0) return r;
	long rem = ov.value.sec;
	if (ov.value.usec != 0) rem++;
	return rem;
}
#endif

/* A fresh, empty directory standing in for a tmpfs: <parent of the
 * root>/.tawcroot-tmpfs/<pid>-<n>, outside the guest's view so no guest
 * path aliases it. Portal clears the directory at session start. */
static long make_tmpfs_dir(char *out, size_t cap, unsigned mode)
{
	static unsigned seq;
	size_t n = tawcroot_rootfs_host_path_len;
	while (n > 1 && tawcroot_rootfs_host_path[n - 1] != '/') n--;
	if (n <= 1 || n + 64 >= cap) return TAWC_ENAMETOOLONG;
	memcpy(out, tawcroot_rootfs_host_path, n);
	size_t pos = n;
	if (tawc_str_append(out, cap, &pos, ".tawcroot-tmpfs")) return TAWC_ENAMETOOLONG;
	long e = TAWC_RAW(TAWC_SYS_mkdirat, AT_FDCWD, (long)out, 0755, 0, 0, 0);
	if (e < 0 && e != TAWC_EEXIST) return e;
	size_t base = pos;
	for (int tries = 0; tries < 64; tries++) {
		pos = base;
		if (tawc_str_append(out, cap, &pos, "/") ||
		    tawc_str_append_dec(out, cap, &pos, tawc_getpid()) ||
		    tawc_str_append(out, cap, &pos, "-") ||
		    tawc_str_append_dec(out, cap, &pos, (long)__atomic_add_fetch(&seq, 1, __ATOMIC_RELAXED)))
			return TAWC_ENAMETOOLONG;
		e = TAWC_RAW(TAWC_SYS_mkdirat, AT_FDCWD, (long)out, 0700, 0, 0, 0);
		if (e == TAWC_EEXIST) continue;
		if (e < 0) return e;
		/* mkdir is filtered by the real umask; set the mode outright. */
		(void)TAWC_RAW(TAWC_SYS_fchmodat, AT_FDCWD, (long)out, mode & 07777, 0, 0, 0);
		return (long)pos;
	}
	return TAWC_EEXIST;
}

/* "mode=<octal>" from a tmpfs option string; tmpfs defaults to 1777. */
static unsigned tmpfs_mode(const char *data)
{
	char opts[256];
	if (!data || tawc_copy_string_from_guest(opts, sizeof opts, data) < 0)
		return 01777;
	for (const char *p = opts; *p; p++) {
		if ((p == opts || p[-1] == ',') && p[0] == 'm' && p[1] == 'o' &&
		    p[2] == 'd' && p[3] == 'e' && p[4] == '=') {
			unsigned m = 0;
			for (p += 5; *p >= '0' && *p <= '7'; p++) m = m * 8 + (unsigned)(*p - '0');
			return m;
		}
	}
	return 01777;
}

static long handle_mount(const tawcroot_syscall_args *args, ucontext_t *uc)
{
	(void)uc;
	/* A bounded path-view operation, not a kernel mount or security
	 * boundary (no CAP_SYS_ADMIN here). Binds enter this process's path
	 * view, as do the filesystems sandboxes build their root from:
	 * tmpfs is a fresh private directory, proc/sysfs/devpts the host's.
	 * A bind remount sets the bind's read-only bit. Propagation
	 * changes, moves and other filesystems succeed without effect, as
	 * under PRoot: sandboxes and `unshare --mount-proc` set those up and
	 * then use the tree that is already there; failing them aborted the
	 * whole tool. */
	unsigned long flags = (unsigned long)args->d;
	const char *target = (const char *)args->b;
	if (!target) return TAWC_EFAULT;
	if (flags & MS_REMOUNT) {
		if (flags & MS_BIND)
			return tawcroot_path_bind_set_ro(target, (flags & MS_RDONLY) != 0);
		return 0;
	}
	if (flags & MS_BIND)
		return tawcroot_path_bind_guest((const char *)args->a, target,
						(flags & MS_REC) != 0);
	if (flags & (MS_MOVE | MS_SHARED | MS_PRIVATE | MS_SLAVE | MS_UNBINDABLE))
		return 0;
	char fstype[16];
	if (!args->c || tawc_copy_string_from_guest(fstype, sizeof fstype,
						    (const char *)args->c) < 0)
		return 0;
	int ro = (flags & MS_RDONLY) != 0;
	if (tawc_streq(fstype, "proc")) return tawcroot_path_bind_host("/proc", target, ro);
	if (tawc_streq(fstype, "sysfs")) return tawcroot_path_bind_host("/sys", target, ro);
	if (tawc_streq(fstype, "devpts")) return tawcroot_path_bind_host("/dev/pts", target, ro);
	if (tawc_streq(fstype, "tmpfs")) {
		TAWCROOT_PATH_SCRATCH_AUTO(scratch);
		char *dir = scratch->buf[0];
		long e = make_tmpfs_dir(dir, TAWCROOT_PATH_SCRATCH_SIZE,
					tmpfs_mode((const char *)args->e));
		if (e < 0) return e;
		return tawcroot_path_bind_host(dir, target, ro);
	}
	return 0;
}

/* umount2: the bind mounted at `target` stops routing (MNT_DETACH: and
 * the ones below it). A target outside the view or without a bind (an
 * emulated propagation-only "mount") succeeds without effect. */
static long handle_umount2(const tawcroot_syscall_args *args, ucontext_t *uc)
{
	(void)uc;
	if (!args->a) return TAWC_EFAULT;
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *rel = scratch->buf[0], *abs = scratch->buf[1], *folded = scratch->buf[2];
	const size_t cap = TAWCROOT_PATH_SCRATCH_SIZE;
	long e = tawc_copy_string_from_guest(rel, cap, (const char *)args->a);
	if (e < 0) return e;
	size_t pos = 0;
	if (rel[0] != '/') {
		long cn = tawcroot_cwd_to_guest_abs(abs, cap);
		if (cn < 0) return 0;
		pos = (size_t)cn;
		if (tawc_str_append(abs, cap, &pos, "/")) return 0;
	}
	if (tawc_str_append(abs, cap, &pos, rel)) return 0;
	if (tawcroot_path_fold_absolute(abs, folded, cap) < 0) return 0;
	/* The fold drops the leading '/'; "" is the root. */
	if (folded[0]) tawcroot_path_unmount(folded, (args->b & MNT_DETACH) != 0);
	return 0;
}

void tawcroot_control_register(void)
{
	tawcroot_dispatch_install(TAWC_SYS_set_robust_list, handle_set_robust_list);
	tawcroot_dispatch_install(TAWC_SYS_get_robust_list, handle_get_robust_list);
	/* Some Android 4.19 kernels backport pidfd_open but not P_PIDFD
	 * waitid. GLib then selects pidfds and loses child exit status.
	 * Probe a closed, non-negative fd: supported kernels return EBADF,
	 * incomplete kernels EINVAL (a negative id is EINVAL everywhere, which
	 * once disabled pidfds on every kernel and broke Steam's lsof check).
	 * Do not emulate waits using a racy fd-to-PID lookup. On incomplete
	 * kernels expose pidfd_open as unavailable so callers use their
	 * ordinary SIGCHLD/waitpid path. New kernels stay native. */
	if (TAWC_RAW(TAWC_SYS_waitid, 3 /* P_PIDFD */, 0x3fffffff, 0,
	             5 /* WEXITED | WNOHANG */, 0, 0) == TAWC_EINVAL)
		tawcroot_dispatch_install(TAWC_SYS_pidfd_open, tawcroot_deny_enosys);
	tawcroot_dispatch_install(TAWC_SYS_seccomp,         handle_seccomp);
	tawcroot_dispatch_install(TAWC_SYS_prctl,           handle_prctl);
	tawcroot_dispatch_install(TAWC_SYS_rt_sigaction,    handle_rt_sigaction);
	tawcroot_dispatch_install(TAWC_SYS_rt_sigprocmask,  handle_rt_sigprocmask);
	/* Keep sigaltstack native: callers may unmap their stack before
	 * disabling it. Trapping that disable would use the unmapped stack. */
	/* io_uring_setup: deny with -ENOSYS so guest libraries fall back to
	 * syscall-based I/O which we can translate. The plan
	 * (notes/tawcroot/path-translation.md "Open questions" #1) classifies a passed-through
	 * io_uring as a *correctness* hazard, not just a missing feature: the
	 * kernel reads SQEs from app-shared memory, sees host-relative paths,
	 * and silently opens host files — bypassing every translation rule.
	 * Programs that probe with -ENOSYS fall back to non-uring paths
	 * cleanly. (Review finding D4.)
	 *
	 * io_uring_register and io_uring_enter trap with the same -ENOSYS for
	 * defense-in-depth: with io_uring_setup denied the guest can't create
	 * a ring fd, but a ring fd inherited from a non-tawcroot parent across
	 * exec would otherwise sail past us untranslated. Trapping the post-
	 * setup syscalls makes "no io_uring traffic ever escapes" enforceable
	 * independently of the stacked Android filter. See notes/tawcroot/path-translation.md
	 * "io_uring MVP behavior". */
	tawcroot_dispatch_install(TAWC_SYS_io_uring_setup,    tawcroot_deny_enosys);
	tawcroot_dispatch_install(TAWC_SYS_io_uring_enter,    tawcroot_deny_enosys);
	tawcroot_dispatch_install(TAWC_SYS_io_uring_register, tawcroot_deny_enosys);

	/* clone3: deny with -ENOSYS so glibc's __clone falls back to the
	 * older clone(2) syscall (NR 220 aarch64 / NR 56 x86_64). All stacked
	 * seccomp filters are evaluated at syscall entry and the most
	 * restrictive action wins, so this can't shield the guest from an
	 * Android RET_KILL on clone3 — it works only because Android's policy
	 * is empirically not KILL: on Android 14 our trap fires ([sigsys]
	 * nr=435), i.e. Android allows-or-traps clone3. Returning -ENOSYS
	 * causes glibc to set its "clone3 missing" flag and use clone()
	 * going forward, keeping the guest off the risky syscall entirely. */
	tawcroot_dispatch_install(TAWC_SYS_clone3,          tawcroot_deny_enosys);

	/* Never trap exit(2): musl unmaps a dying thread's stack before the
	 * syscall. SIGSYS delivery then has no stack and kills the process. */

#if defined(__x86_64__)
	tawcroot_dispatch_install(TAWC_SYS_getpgrp,         handle_getpgrp);
	tawcroot_dispatch_install(TAWC_SYS_time,            handle_time);
	tawcroot_dispatch_install(TAWC_SYS_alarm,           handle_alarm);
#endif

	/* Mount syscalls never reach the kernel: they would tear down our
	 * setup binds (/dev/shm, /proc, libhybris stage) or hand the guest a
	 * namespace where our fd-relative /proc walks no longer name what we
	 * think they name. mount, umount2, unshare and setns are emulated on
	 * the path view (handle_mount, namespace.c); pivot_root and chroot
	 * swap its root (chroot.c). */
	tawcroot_dispatch_install(TAWC_SYS_mount,           handle_mount);
	/* PRoot parity: namespace joins succeed without effect (there are
	 * no other namespaces to join). */
	tawcroot_dispatch_install(TAWC_SYS_umount2,         handle_umount2);
	tawcroot_dispatch_install(TAWC_SYS_setns,           tawcroot_answer_zero);
}
