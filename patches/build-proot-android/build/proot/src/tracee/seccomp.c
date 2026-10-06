#include <errno.h>     /* E*, */
#include <signal.h>    /* SIGSYS, */
#include <unistd.h>    /* getpgid, */
#include <utime.h>     /* utimbuf, */
#include <sys/vfs.h>   /* statfs64 */
#include <string.h>    /* memset   */
#include <linux/net.h> /* SYS_SENDMMSG */
#include <assert.h>    /* assert(3), */
#include <time.h>      /* time(2), */
#include <inttypes.h>  /* PRIu64, */
#include <sched.h>     /* CLONE_SIGHAND, */
#include <talloc.h>    /* talloc_*, */
#include <stdint.h>    /* uint64_t, */
#include <sys/ptrace.h> /* ptrace(2), */

#include "extension/extension.h"
#include "cli/note.h"
#include "syscall/chain.h"
#include "syscall/syscall.h"
#include "tracee/seccomp.h"
#include "tracee/mem.h"
#include "tracee/abi.h"
#include "tracee/statx.h"
#include "path/path.h"

static int handle_seccomp_event_common(Tracee *tracee);

/* A signal the kernel forces on a thread which blocks it gets its
 * handler reset to SIG_DFL.  Android's filter traps set_robust_list(2)
 * right at the start of each thread, where glibc still blocks every
 * signal, so the SIGSYS handler of a sandboxed process (Chromium's,
 * Firefox's) is lost to the first thread it creates, and the next trap
 * of its own filter kills it.  PRoot thus keeps a copy of the guest's
 * SIGSYS action, the kernel's struct sigaction of the 64-bit ABIs
 * (handler, flags, restorer, mask), and reinstalls it after each trap
 * of Android's filter it absorbs.  */
struct sigsys_action {
	bool valid;
	uint8_t act[32];
};

/**
 * Make @child, just created by @parent with @clone_flags, share or
 * copy the SIGSYS action of @parent.
 */
void sigsys_action_new_child(Tracee *parent, Tracee *child, word_t clone_flags)
{
	if ((clone_flags & CLONE_SIGHAND) != 0) {
		if (parent->sigsys_action == NULL)
			parent->sigsys_action = talloc_zero(parent, struct sigsys_action);
		if (parent->sigsys_action != NULL)
			child->sigsys_action = talloc_reference(child, parent->sigsys_action);
	}
	else if (parent->sigsys_action != NULL && parent->sigsys_action->valid)
		child->sigsys_action = talloc_memdup(child, parent->sigsys_action,
						     sizeof(struct sigsys_action));
}

/**
 * Remember the SIGSYS action @tracee's rt_sigaction(2) sets, if any.
 */
void sigsys_action_enter(Tracee *tracee)
{
	word_t act = peek_reg(tracee, CURRENT, SYSARG_2);

	tracee->sigsys_action_new.pending = false;
	if (sizeof_word(tracee) != 8
	    || (int) peek_reg(tracee, CURRENT, SYSARG_1) != SIGSYS
	    || act == 0 || peek_reg(tracee, CURRENT, SYSARG_4) != 8)
		return;

	if (read_data(tracee, tracee->sigsys_action_new.act, act, sizeof(tracee->sigsys_action_new.act)) < 0)
		return;
	tracee->sigsys_action_new.pending = true;
}

/**
 * Commit the SIGSYS action remembered at the enter stage of @tracee's
 * rt_sigaction(2) if it succeeded, and report the result of the
 * trapped syscall if this rt_sigaction(2) was a reinstallation.
 */
void sigsys_action_exit(Tracee *tracee)
{
	const bool pending = tracee->sigsys_action_new.pending;

	tracee->sigsys_action_new.pending = false;
	if (pending && (long) peek_reg(tracee, CURRENT, SYSARG_RESULT) == 0) {
		if (tracee->sigsys_action == NULL)
			tracee->sigsys_action = talloc_zero(tracee, struct sigsys_action);
		if (tracee->sigsys_action != NULL) {
			memcpy(tracee->sigsys_action->act, tracee->sigsys_action_new.act,
			       sizeof(tracee->sigsys_action->act));
			tracee->sigsys_action->valid = true;
		}
	}

	if (tracee->sigsys_reinstall.pending) {
		tracee->sigsys_reinstall.pending = false;
		poke_reg(tracee, SYSARG_RESULT, tracee->sigsys_reinstall.result);
	}
}

/**
 * execve(2) resets the handlers of @tracee, and unshares them.
 */
void sigsys_action_execve(Tracee *tracee)
{
	struct sigsys_action *action = tracee->sigsys_action;
	word_t handler;

	if (action == NULL || (long) peek_reg(tracee, CURRENT, SYSARG_RESULT) < 0)
		return;

	memcpy(&handler, action->act, sizeof(handler));
	tracee->sigsys_action = NULL;
	if (action->valid && handler == (word_t) SIG_IGN)
		tracee->sigsys_action = talloc_memdup(tracee, action, sizeof(*action));
	talloc_unlink(tracee, action);
}

#ifndef PTRACE_GETSIGMASK
#define PTRACE_GETSIGMASK 0x420a
#define PTRACE_SETSIGMASK 0x420b
#endif

/**
 * Unblock SIGSYS in @tracee, a new thread or process which hasn't run
 * any code yet, if it has a handler for it.  glibc starts threads
 * with every signal blocked and calls set_robust_list(2) first, which
 * Android's filter traps: SIGSYS then gets through without resetting
 * the handler (see above), which the other threads of the process may
 * need before PRoot could reinstall it.  glibc sets the mask of the
 * thread right after.
 */
void sigsys_unblock_new_tracee(Tracee *tracee)
{
	const struct sigsys_action *action = tracee->sigsys_action;
	const uint64_t sigsys_bit = UINT64_C(1) << (SIGSYS - 1);
	uint64_t mask;
	word_t handler;

	if (action == NULL || !action->valid)
		return;

	memcpy(&handler, action->act, sizeof(handler));
	if (handler == (word_t) SIG_DFL)
		return;

	if (ptrace(PTRACE_GETSIGMASK, tracee->pid, sizeof(mask), &mask) < 0
	    || (mask & sigsys_bit) == 0)
		return;

	mask &= ~sigsys_bit;
	(void) ptrace(PTRACE_SETSIGMASK, tracee->pid, sizeof(mask), &mask);
}

/**
 * Reinstall the SIGSYS action of @tracee, which Android's filter just
 * trapped a syscall of: replace the latter, whose result is already
 * set, with rt_sigaction(2), and report that result at its exit.
 */
static void reinstall_sigsys_action(Tracee *tracee)
{
	const struct sigsys_action *action = tracee->sigsys_action;
	word_t handler;
	word_t address;

	if (action == NULL || !action->valid || sizeof_word(tracee) != 8
	    || tracee->restore_original_regs_after_seccomp_event)
		return;

	memcpy(&handler, action->act, sizeof(handler));
	if (handler == (word_t) SIG_DFL)
		return;

	address = alloc_mem(tracee, sizeof(action->act) + RED_ZONE_SIZE);
	if (address == 0 || write_data(tracee, address, action->act, sizeof(action->act)) < 0) {
		poke_reg(tracee, STACK_POINTER, peek_reg(tracee, ORIGINAL_SECCOMP_REWRITE, STACK_POINTER));
		return;
	}

	VERBOSE(tracee, 3, "vpid %" PRIu64 ": reinstalling the SIGSYS handler", tracee->vpid);

	tracee->sigsys_reinstall.pending = true;
	tracee->sigsys_reinstall.result = peek_reg(tracee, CURRENT, SYSARG_RESULT);
	set_sysnum(tracee, PR_rt_sigaction);
	poke_reg(tracee, SYSARG_1, SIGSYS);
	poke_reg(tracee, SYSARG_2, address);
	poke_reg(tracee, SYSARG_3, 0);
	poke_reg(tracee, SYSARG_4, 8);
	restart_syscall_after_seccomp(tracee);
}

/**
 * Restart syscall that caused seccomp event
 * after changing it in tracee registers
 *
 * Syscall that will be restarted will be translated by proot
 * so SIGSYS handler sees untranslated paths and should leave
 * them untranslated.
 */
void restart_syscall_after_seccomp(Tracee* tracee) {
	word_t instr_pointer;

	/* Enable restore regs at end of replaced call.
	 * This also defers delivering of signals until restarted syscall finishes.  */
	tracee->restore_original_regs_after_seccomp_event = true;
	tracee->restart_how = PTRACE_SYSCALL;

	/* Move the instruction pointer back to the original trap */
	instr_pointer = peek_reg(tracee, CURRENT, INSTR_POINTER);
	poke_reg(tracee, INSTR_POINTER, instr_pointer - get_systrap_size(tracee));

	/* X86 usually uses orig_rax when selecting syscall,
	 * but as this code is happening outside syscall handler
	 * we need to copy orig_eax back to eax.  */
#if defined(ARCH_X86_64)
	tracee->_regs[CURRENT].rax = tracee->_regs[CURRENT].orig_rax;
#elif defined(ARCH_X86)
	tracee->_regs[CURRENT].eax = tracee->_regs[CURRENT].orig_eax;
#endif

	/* Write registers. (Omiting special sysnum logic as we're not during syscall
	 * execution, but we're queueing new syscall to be called) */
	push_specific_regs(tracee, false);
}

/**
 * Set specified result (negative for errno) and do not restart syscall.
 */
void set_result_after_seccomp(Tracee *tracee, word_t result) {
	VERBOSE(tracee, 3, "Setting result after SIGSYS to 0x%lx", result);
	poke_reg(tracee, SYSARG_RESULT, result);
	push_specific_regs(tracee, false);
}

/**
 * Answer set_robust_list(2) or get_robust_list(2) for @tracee from the
 * head PRoot keeps per thread, and return the syscall result.
 *
 * Android's app filter traps set_robust_list(2), so glibc never really
 * registers its list; answering ENOSYS made glibc refuse robust mutexes,
 * and Chromium's zygote (Electron, CEF, Steam's webhelper) aborts with
 * "futex robust_list not initialized by pthreads" when
 * get_robust_list(2) then returns no head.  Only the kernel's cleanup of
 * locks a dying thread still holds is lost.
 */
int emulate_robust_list(Tracee *tracee, Sysnum sysnum)
{
	if (sysnum == PR_set_robust_list) {
		tracee->robust_list_head = peek_reg(tracee, CURRENT, SYSARG_1);
		tracee->robust_list_len  = peek_reg(tracee, CURRENT, SYSARG_2);
		return 0;
	}
	if (sysnum == PR_get_robust_list) {
		pid_t pid = (pid_t) peek_reg(tracee, CURRENT, SYSARG_1);
		word_t head_ptr = peek_reg(tracee, CURRENT, SYSARG_2);
		word_t len_ptr = peek_reg(tracee, CURRENT, SYSARG_3);
		const Tracee *target = tracee;

		if (pid != 0 && pid != tracee->pid) {
			target = get_tracee(tracee, pid, false);
			if (target == NULL)
				return -ESRCH;
		}
		if (write_data(tracee, head_ptr, &target->robust_list_head, sizeof(word_t)) < 0
		    || write_data(tracee, len_ptr, &target->robust_list_len, sizeof(word_t)) < 0)
			return -EFAULT;
		return 0;
	}
	return -ENOSYS;
}

/**
 * Handle SIGSYS signal that was caused by system seccomp policy.
 *
 * Return 0 to swallow signal or SIGSYS to deliver it to process.
 */
int handle_seccomp_event(Tracee* tracee)
{
	int ret;

	/* Reset status so next SIGTRAP | 0x80 is
	 * recognized as syscall entry.  */
	tracee->status = 0;

	/* Registers are never restored at this stage as they weren't saved.  */
	tracee->restore_original_regs = false;

	/* Fetch registers.  */
	ret = fetch_regs(tracee);
	if (ret != 0) {
		VERBOSE(tracee, 1, "Couldn't fetch regs on seccomp SIGSYS");
		tracee->restore_sysarg1_after_sigsys = false;
		return SIGSYS;
	}

#if defined(ARCH_ARM_EABI) || defined(ARCH_ARM64)
	/* A synthesized sysexit ran before this SIGSYS and poked
	 * SYSARG_RESULT, which on ARM/ARM64 aliases SYSARG_1.  The blocked
	 * syscall's first argument (e.g. a path pointer, or the rgid of
	 * setresgid) was thus overwritten with the faked result.  Restore it
	 * from the entry snapshot so both the SIGSYS emulation and any *at
	 * style restart below read the real argument.  The sysnum guard keeps
	 * a stale ORIGINAL (from an unrelated prior syscall) from leaking in.  */
	if (tracee->restore_sysarg1_after_sigsys
	    && get_sysnum(tracee, ORIGINAL) == get_sysnum(tracee, CURRENT))
		poke_reg(tracee, SYSARG_1, peek_reg(tracee, ORIGINAL, SYSARG_1));
#endif
	tracee->restore_sysarg1_after_sigsys = false;

	/* Save regs so they can be restored at end of replaced call.  */
	save_current_regs(tracee, ORIGINAL_SECCOMP_REWRITE);

	/* X86 uses orig_rax when selecting syscall,
	 * however at this point we are after syscall has been rejected
	 * and orig_rax was reset to -1.  */
#if defined(ARCH_X86_64)
	tracee->_regs[CURRENT].orig_rax = tracee->_regs[CURRENT].rax;
#elif defined(ARCH_X86)
	tracee->_regs[CURRENT].orig_eax = tracee->_regs[CURRENT].eax;
#endif

	print_current_regs(tracee, 3, "seccomp SIGSYS");

	ret = handle_seccomp_event_common(tracee);
	if (ret == 0)
		reinstall_sigsys_action(tracee);
	return ret;
}

/**
 * Reinstall the SIGSYS action of @tracee after Android's filter
 * trapped the syscall PRoot voided at the enter stage, whose result
 * is already faked.
 */
void reinstall_sigsys_action_after_void(Tracee *tracee)
{
	const struct sigsys_action *action = tracee->sigsys_action;

	if (action == NULL || !action->valid)
		return;

	tracee->status = 0;
	tracee->restore_original_regs = false;
	if (fetch_regs(tracee) != 0)
		return;
	save_current_regs(tracee, ORIGINAL_SECCOMP_REWRITE);
	reinstall_sigsys_action(tracee);
}

void fix_and_restart_enosys_syscall(Tracee* tracee)
{
	/* Reset tracee state so we're not handling syscall exit */
	tracee->status = 0;
	tracee->restore_original_regs = false;

	/* Restore and save original registers */
	memcpy(&tracee->_regs[CURRENT], &tracee->_regs[ORIGINAL], sizeof(tracee->_regs[CURRENT]));
	save_current_regs(tracee, ORIGINAL_SECCOMP_REWRITE);

	handle_seccomp_event_common(tracee);
}

static int handle_seccomp_event_common(Tracee *tracee)
{
	int ret;
	int status;
	Sysnum sysnum = get_sysnum(tracee, CURRENT);

	sysnum = get_sysnum(tracee, CURRENT);

	status = notify_extensions(tracee, SIGSYS_OCC, 0, 0);
	if (status < 0) {
		VERBOSE(tracee, 4, "SIGSYS errored out when being handled by an extension");
		set_result_after_seccomp(tracee, status);
		return 0;
	}
	if (status == 1) {
		VERBOSE(tracee, 4, "SIGSYS fully handled by an extension");
		set_result_after_seccomp(tracee, 0);
		return 0;
	}
	if (status == 2) {
		VERBOSE(tracee, 4, "SIGSYS fully handled by an extension with result set");
		return 0;
	}

	switch (sysnum) {
	case PR_open:
		set_sysnum(tracee, PR_openat);
		poke_reg(tracee, SYSARG_4, peek_reg(tracee, CURRENT, SYSARG_3));
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_openat2: {
		/* int openat2(int dirfd, const char *pathname,
		 *             struct open_how *how, size_t size);
		 *
		 * Convert to openat() so the call survives an outer seccomp
		 * policy that rejects the newer syscall (this is what raised
		 * the SIGSYS that brought us here), and so PRoot translates
		 * the path when the syscall is restarted.  The how.resolve
		 * flags (RESOLVE_BENEATH, ...) are dropped: they are not
		 * compatible with PRoot rewriting paths to absolute host
		 * paths, and PRoot already confines resolution to the rootfs.  */
		struct proot_open_how how = {};
		word_t how_size = peek_reg(tracee, CURRENT, SYSARG_4);
		if (how_size > sizeof(how))
			how_size = sizeof(how);
		ret = read_data(tracee, &how, peek_reg(tracee, CURRENT, SYSARG_3), how_size);
		if (ret < 0) {
			set_result_after_seccomp(tracee, ret);
			break;
		}
		set_sysnum(tracee, PR_openat);
		poke_reg(tracee, SYSARG_3, how.flags);
		poke_reg(tracee, SYSARG_4, how.mode);
		restart_syscall_after_seccomp(tracee);
		break;
	}

	case PR_accept:
		set_sysnum(tracee, PR_accept4);
		poke_reg(tracee, SYSARG_4, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_setgroups:
	case PR_setgroups32:
		set_result_after_seccomp(tracee, 0);
		break;

	/* The Android parent process commonly installs a seccomp
	 * filter that traps mount/umount/pivot_root/unshare/setns
	 * with SIGSYS.  Mirror what enter.c does for these: pretend
	 * they succeeded and apply the mount/pivot_root binding
	 * emulation so sandbox helpers like bubblewrap can proceed.  */
	case PR_mount:
		apply_emulated_mount(tracee);
		set_result_after_seccomp(tracee, 0);
		break;

	case PR_pivot_root:
		apply_emulated_pivot_root(tracee);
		set_result_after_seccomp(tracee, 0);
		break;

	case PR_umount:
	case PR_umount2:
		apply_emulated_umount(tracee);
		set_result_after_seccomp(tracee, 0);
		break;

	case PR_unshare:
	case PR_setns:
		set_result_after_seccomp(tracee, 0);
		break;

	case PR_getpgrp:
		/* Query value with getpgid and set it as result.  */
		set_result_after_seccomp(tracee, getpgid(tracee->pid));
		break;

	case PR_symlink:
		set_sysnum(tracee, PR_symlinkat);
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, AT_FDCWD);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_link:
		set_sysnum(tracee, PR_linkat);
		poke_reg(tracee, SYSARG_4, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		poke_reg(tracee, SYSARG_3, AT_FDCWD);
		poke_reg(tracee, SYSARG_5, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_chmod:
		set_sysnum(tracee, PR_fchmodat);
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		poke_reg(tracee, SYSARG_4, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_chown:
	case PR_lchown:
	case PR_chown32:
	case PR_lchown32:
		set_sysnum(tracee, PR_fchownat);
		poke_reg(tracee, SYSARG_4, peek_reg(tracee, CURRENT, SYSARG_3));
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		if (sysnum == PR_lchown || sysnum == PR_lchown32) {
			poke_reg(tracee, SYSARG_5, AT_SYMLINK_NOFOLLOW);
		} else {
			poke_reg(tracee, SYSARG_5, 0);
		}
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_unlink:
	case PR_rmdir:
		set_sysnum(tracee, PR_unlinkat);
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		poke_reg(tracee, SYSARG_3, sysnum==PR_rmdir ? AT_REMOVEDIR : 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_send:
		set_sysnum(tracee, PR_sendto);
		poke_reg(tracee, SYSARG_5, 0);
		poke_reg(tracee, SYSARG_6, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_recv:
		set_sysnum(tracee, PR_recvfrom);
		poke_reg(tracee, SYSARG_5, 0);
		poke_reg(tracee, SYSARG_6, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_waitpid:
		set_sysnum(tracee, PR_wait4);
		poke_reg(tracee, SYSARG_4, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_statfs:
	{
		int size;
		int status;
		char path[PATH_MAX];
		char original[PATH_MAX];
		char devshm_path[PATH_MAX];
		struct statfs64 my_statfs64;
		struct compat_statfs my_statfs;
		size = read_string(tracee, original, peek_reg(tracee, CURRENT, SYSARG_1), PATH_MAX);
		if (size < 0) {
			set_result_after_seccomp(tracee, size);
			break;
		}
		if (size >= PATH_MAX) { 
			set_result_after_seccomp(tracee, -ENAMETOOLONG);
			break;
		}
            	translate_path(tracee, path, AT_FDCWD, original, true);
		errno = 0;
		status = statfs64(path, &my_statfs64); 
		if (errno != 0) {
			set_result_after_seccomp(tracee, -errno);
			break;
		}

		/* Fake /dev/shm being tmpfs, see statfs handler in syscall/exit.c */
		if (translate_path(tracee, devshm_path, AT_FDCWD, "/dev/shm", true) >= 0) {
			Comparison comparison = compare_paths(devshm_path, path);
			if (comparison == PATHS_ARE_EQUAL || comparison == PATH1_IS_PREFIX) {
				my_statfs64.f_type = 0x01021994;
			}
		}

		if ((my_statfs64.f_blocks | my_statfs64.f_bfree | my_statfs64.f_bavail |
     		     my_statfs64.f_bsize | my_statfs64.f_frsize | my_statfs64.f_files | 
		     my_statfs64.f_ffree) & 0xffffffff00000000ULL) { 
			set_result_after_seccomp(tracee, -EOVERFLOW);
			break;
		}
		my_statfs.f_type = my_statfs64.f_type;
		my_statfs.f_bsize = my_statfs64.f_bsize;
		my_statfs.f_blocks = my_statfs64.f_blocks;
		my_statfs.f_bfree = my_statfs64.f_bfree;
		my_statfs.f_bavail = my_statfs64.f_bavail;
		my_statfs.f_files = my_statfs64.f_files;
		my_statfs.f_ffree = my_statfs64.f_ffree;
		my_statfs.f_fsid = my_statfs64.f_fsid;
		my_statfs.f_namelen = my_statfs64.f_namelen;
		my_statfs.f_frsize = my_statfs64.f_frsize;
		my_statfs.f_flags = my_statfs64.f_flags;
		memset(my_statfs.f_spare, 0, sizeof(my_statfs.f_spare));
                write_data(tracee, peek_reg(tracee, CURRENT, SYSARG_2), &my_statfs, sizeof(struct compat_statfs));

		set_result_after_seccomp(tracee, 0);
		break;
	}

	case PR_utimes:
	{
		/* int utimes(const char *filename, const struct timeval times[2]);
		 *
		 * convert to:
		 * int utimensat(int dirfd, const char *pathname, const struct timespec times[2], int flags);  */
		struct timeval times[2];
		struct timespec timens[2];

		set_sysnum(tracee, PR_utimensat);
		if (peek_reg(tracee, CURRENT, SYSARG_2) != 0) {
			ret = read_data(tracee, times, peek_reg(tracee, CURRENT, SYSARG_2), sizeof(times));
			if (ret < 0) {
				set_result_after_seccomp(tracee, ret);
				break;
			}
			timens[0].tv_sec = (time_t)times[0].tv_sec;
			timens[0].tv_nsec = (long)times[0].tv_usec * 1000;
			timens[1].tv_sec = (time_t)times[1].tv_sec;
			timens[1].tv_nsec = (long)times[1].tv_usec * 1000;
			ret = set_sysarg_data(tracee, timens, sizeof(timens), SYSARG_2);
			if (ret < 0) {
				set_result_after_seccomp(tracee, ret);
				break;
			}
		}
		poke_reg(tracee, SYSARG_4, 0);
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		restart_syscall_after_seccomp(tracee);
		break;
	}

	case PR_utime:
	{
		/* int utime(const char *filename, const struct utimbuf *times);
		 *
		 * convert to:
		 * int utimensat(int dirfd, const char *pathname, const struct timespec times[2], int flags);  */
		struct utimbuf times;
		struct timespec timens[2];

		set_sysnum(tracee, PR_utimensat);
		if (peek_reg(tracee, CURRENT, SYSARG_2) != 0) {
			ret = read_data(tracee, &times, peek_reg(tracee, CURRENT, SYSARG_2), sizeof(times));
			if (ret < 0) {
				set_result_after_seccomp(tracee, ret);
				break;
			}
			timens[0].tv_sec = (time_t)times.actime;
			timens[0].tv_nsec = 0;
			timens[1].tv_sec = (time_t)times.modtime;
			timens[1].tv_nsec = 0;
			ret = set_sysarg_data(tracee, timens, sizeof(timens), SYSARG_2);
			if (ret < 0) {
				set_result_after_seccomp(tracee, ret);
				break;
			}
		}
		poke_reg(tracee, SYSARG_4, 0);
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		restart_syscall_after_seccomp(tracee);
		break;
	}

#if defined(ARCH_X86) || defined(ARCH_X86_64)
	case PR_sendmmsg:
	{
		/* Convert direct sendmmsg syscall to socketcall.
		 * This affects only 32-bit x86, in other archs
		 * bionic doesn't use socketcall() for sendmmsg.  */
		size_t arg_size = sizeof_word(tracee);
		assert(arg_size <= sizeof(word_t));
		byte_t args[arg_size * 4];
		memset(args, 0, arg_size * 4);
		*(word_t*)(args) = peek_reg(tracee, CURRENT, SYSARG_1);
		*(word_t*)(args + arg_size) = peek_reg(tracee, CURRENT, SYSARG_2);
		*(word_t*)(args + 2 * arg_size) = peek_reg(tracee, CURRENT, SYSARG_3);
		*(word_t*)(args + 3 * arg_size) = peek_reg(tracee, CURRENT, SYSARG_4);
		word_t tracee_args = alloc_mem(tracee, arg_size * 4);
		write_data(tracee, tracee_args, args, arg_size * 4);
		set_sysnum(tracee, PR_socketcall);
		poke_reg(tracee, SYSARG_1, SYS_SENDMMSG);
		poke_reg(tracee, SYSARG_2, tracee_args);
		restart_syscall_after_seccomp(tracee);
		break;
	}
#endif

	case PR_stat:
	case PR_lstat:
		set_sysnum(tracee, PR_newfstatat);
		poke_reg(tracee, SYSARG_4, sysnum == PR_lstat ? AT_SYMLINK_NOFOLLOW : 0);
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_pipe:
		set_sysnum(tracee, PR_pipe2);
		poke_reg(tracee, SYSARG_2, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_dup2:
		set_sysnum(tracee, PR_dup3);
		poke_reg(tracee, SYSARG_3, 0);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_access:
		set_sysnum(tracee, PR_faccessat);
		poke_reg(tracee, SYSARG_4, 0);
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_mkdir:
		set_sysnum(tracee, PR_mkdirat);
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_rename:
		set_sysnum(tracee, PR_renameat);
		poke_reg(tracee, SYSARG_4, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_3, AT_FDCWD);
		poke_reg(tracee, SYSARG_2, peek_reg(tracee, CURRENT, SYSARG_1));
		poke_reg(tracee, SYSARG_1, AT_FDCWD);
		restart_syscall_after_seccomp(tracee);
		break;

	case PR_select:
	{
		// TODO: This doesn't update timeout with time spent inside select(2)
		//       after returning from syscall
		word_t timeval_arg = peek_reg(tracee, CURRENT, SYSARG_5);
		word_t timespec_arg = 0;
		if (timeval_arg != 0) {
			struct timeval tv = {};
			if (read_data(tracee, &tv, timeval_arg, sizeof(tv))) {
				set_result_after_seccomp(tracee, -EFAULT);
				break;
			}
			if (tv.tv_usec >= 1000000 || tv.tv_usec < 0) {
				set_result_after_seccomp(tracee, -EINVAL);
				break;
			}
			struct timespec ts = {
				.tv_sec = tv.tv_sec,
				.tv_nsec = tv.tv_usec * 1000
			};
			timespec_arg = alloc_mem(tracee, sizeof(ts));
			if(write_data(tracee, timespec_arg, &ts, sizeof(ts))) {
				set_result_after_seccomp(tracee, -EFAULT);
				break;
			}
		}
		set_sysnum(tracee, PR_pselect6);
		poke_reg(tracee, SYSARG_5, timespec_arg);
		poke_reg(tracee, SYSARG_6, 0);
		restart_syscall_after_seccomp(tracee);
		break;
	}

	case PR_poll:
	{
		int ms_arg = (int) peek_reg(tracee, CURRENT, SYSARG_3);
		word_t timespec_arg = 0;
		if (ms_arg >= 0) {
			struct timespec ts = {
				.tv_sec = ms_arg / 1000,
				.tv_nsec = (ms_arg % 1000) * 1000000
			};
			timespec_arg = alloc_mem(tracee, sizeof(ts));
			if(write_data(tracee, timespec_arg, &ts, sizeof(ts))) {
				set_result_after_seccomp(tracee, -EFAULT);
				break;
			}
		}
		set_sysnum(tracee, PR_ppoll);
		poke_reg(tracee, SYSARG_3, timespec_arg);
		poke_reg(tracee, SYSARG_4, 0);
		poke_reg(tracee, SYSARG_5, 0);
		restart_syscall_after_seccomp(tracee);
		break;
	}

	case PR_epoll_wait:
	{
		set_sysnum(tracee, PR_epoll_pwait);
		poke_reg(tracee, SYSARG_5, 0);
		poke_reg(tracee, SYSARG_6, 0);
		restart_syscall_after_seccomp(tracee);
		break;
	}

	case PR_time:
	{
		time_t t = time(NULL);
		word_t addr = peek_reg(tracee, CURRENT, SYSARG_1);
		errno = 0;
		if (addr != 0) {
			poke_word(tracee, addr, t);
		}
		set_result_after_seccomp(tracee, errno ? -EFAULT : t);
		break;
	}

	case PR_statx:
	{
		set_result_after_seccomp(tracee, handle_statx_syscall(tracee, true));
		break;
	}

	case PR_ftruncate:
	{
		if (detranslate_sysnum(get_abi(tracee), PR_ftruncate64) == SYSCALL_AVOIDER) {
			set_result_after_seccomp(tracee, -ENOSYS);
			break;
		}
		set_sysnum(tracee, PR_ftruncate64);
		poke_reg(tracee, SYSARG_3, peek_reg(tracee, CURRENT, SYSARG_2));
		poke_reg(tracee, SYSARG_2, 0);
		poke_reg(tracee, SYSARG_4, 0);
		restart_syscall_after_seccomp(tracee);
		break;
	}

	case PR_setresuid:
	case PR_setresgid:
	{
		gid_t rxid, exid, sxid, rxid_, exid_, sxid_;
		rxid = peek_reg(tracee, CURRENT, SYSARG_1);
		exid = peek_reg(tracee, CURRENT, SYSARG_2);
		sxid = peek_reg(tracee, CURRENT, SYSARG_3);
		if (sysnum == PR_setresuid)
			ret = getresuid(&rxid_, &exid_, &sxid_);
		else if (sysnum == PR_setresgid)
			ret = getresgid(&rxid_, &exid_, &sxid_);
		if (ret) {  // EFAULT = address outside address space
			set_result_after_seccomp(tracee, -EPERM);
			break;
		}
		ret = 0;
		if (rxid != rxid_ && rxid != -1)
			ret = -EPERM;
		if (exid != exid_ && exid != -1)
			ret = -EPERM;
		if (sxid != sxid_ && sxid != -1)
			ret = -EPERM;
		set_result_after_seccomp(tracee, ret);
		break;
	}

	case PR_set_robust_list:
	case PR_get_robust_list:
		set_result_after_seccomp(tracee, emulate_robust_list(tracee, sysnum));
		break;

	default:
		/* Set errno to -ENOSYS */
		set_result_after_seccomp(tracee, -ENOSYS);
	}

	return 0;
}
