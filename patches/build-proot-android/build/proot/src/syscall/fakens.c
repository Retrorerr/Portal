/* -*- c-set-style: "K&R"; c-basic-offset: 8 -*-
 *
 * This file is part of PRoot.
 *
 * This program is free software; you can redistribute it and/or
 * modify it under the terms of the GNU General Public License as
 * published by the Free Software Foundation; either version 2 of the
 * License, or (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful, but
 * WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA
 * 02110-1301 USA.
 */

/* Emulated user and PID namespaces.
 *
 * PRoot strips CLONE_NEW* flags and pretends unshare(2) worked, since
 * kernels such as Android's have no user or PID namespaces at all.
 * Sandboxes (Chromium's, Firefox's, bubblewrap) then go on checking
 * what they believe they got: /proc/self/ns/{user,pid} exist, they
 * hold capabilities inside the new user namespace (for chroot(2),
 * among others), and the first process of a new PID namespace is
 * PID 1.  This file keeps those promises, so the sandboxes come up as
 * on a stock kernel and install their seccomp-bpf filters, the part
 * of them the kernel does enforce here.
 *
 * getpid(2) and friends are hot, so PRoot's own filter never traces
 * them.  A process creating a PID namespace instead gets a second
 * filter, chained right before its clone(2), which traces them for
 * it and its descendants only.  */

#include <errno.h>          /* E*, */
#include <string.h>         /* strcmp(3), */
#include <stdint.h>         /* uint*_t, */
#include <stddef.h>         /* offsetof, */
#include <unistd.h>         /* syscall(2), */
#include <sched.h>          /* CLONE_*, */
#include <sys/syscall.h>    /* SYS_*, */
#include <sys/resource.h>   /* PRIO_PROCESS, */
#include <sys/stat.h>       /* lstat(2), */
#include <linux/filter.h>   /* struct sock_filter, BPF_*, */
#include <linux/seccomp.h>  /* SECCOMP_*, */
#include <linux/audit.h>    /* AUDIT_ARCH_*, */
#include <linux/capability.h> /* CAP_*, _LINUX_CAPABILITY_*, */
#include <talloc.h>         /* talloc_*, */

#include "syscall/fakens.h"
#include "syscall/syscall.h"
#include "syscall/sysnum.h"
#include "syscall/chain.h"
#include "tracee/tracee.h"
#include "tracee/reg.h"
#include "tracee/mem.h"
#include "tracee/abi.h"
#include "extension/extension.h"
#include "extension/fake_id0/config.h"
#include "arch.h"

#ifndef CLONE_NEWUSER
#define CLONE_NEWUSER 0x10000000
#endif
#ifndef CLONE_NEWPID
#define CLONE_NEWPID 0x20000000
#endif

#define ALL_CAPS ((UINT64_C(1) << (CAP_LAST_CAP + 1)) - 1)

pid_t fakens_tgid(const Tracee *tracee)
{
	return tracee->tgid != 0 ? tracee->tgid : tracee->pid;
}

/**
 * Tell whether @tracee holds @cap in the user namespace PRoot
 * pretends it created.
 */
bool fakens_has_cap(const Tracee *tracee, int cap)
{
	return tracee->userns && (tracee->cap_effective & (UINT64_C(1) << cap)) != 0;
}

static void enter_userns(Tracee *tracee)
{
	/* A process gets every capability in the user namespace it
	 * creates.  */
	tracee->userns = true;
	tracee->cap_effective = ALL_CAPS;
	tracee->cap_permitted = ALL_CAPS;
	tracee->cap_inheritable = 0;
}

/**
 * Make @child, just created by @parent with @clone_flags (namespace
 * flags already stripped), inherit the emulated namespaces.
 */
void fakens_new_child(Tracee *parent, Tracee *child, word_t clone_flags)
{
	const bool thread = (clone_flags & CLONE_THREAD) != 0;

	child->tgid = thread ? fakens_tgid(parent) : child->pid;
	child->pidns_filter = parent->pidns_filter;

	child->userns = parent->userns;
	child->cap_effective = parent->cap_effective;
	child->cap_permitted = parent->cap_permitted;
	child->cap_inheritable = parent->cap_inheritable;
	if (parent->clone_stripped_newuser && !thread)
		enter_userns(child);

	TALLOC_FREE(child->pidns);
	TALLOC_FREE(child->pidns_for_children);
	if (thread) {
		if (parent->pidns != NULL)
			child->pidns = talloc_reference(child, parent->pidns);
		if (parent->pidns_for_children != NULL)
			child->pidns_for_children = talloc_reference(child, parent->pidns_for_children);
	}
	else if (parent->clone_stripped_newpid) {
		child->pidns = talloc_zero(child, struct fake_pidns);
		if (child->pidns != NULL)
			child->pidns->init = child->pid;
	}
	else if (parent->pidns_for_children != NULL) {
		/* The first child forked after unshare(CLONE_NEWPID)
		 * is the init of that namespace.  */
		if (parent->pidns_for_children->init == 0)
			parent->pidns_for_children->init = child->pid;
		child->pidns = talloc_reference(child, parent->pidns_for_children);
	}
	else if (parent->pidns != NULL)
		child->pidns = talloc_reference(child, parent->pidns);

	parent->clone_stripped_newuser = false;
	parent->clone_stripped_newpid = false;
}

/**
 * Replace @tracee's current syscall with the installation of the
 * seccomp filter that traces the syscalls whose answer depends on the
 * PID namespace (see fakens_enter).  If @chain, the current syscall
 * is restarted right after.  This function returns -errno if an error
 * occured, otherwise 1 if the syscall was replaced, 0 if not.
 */
static int install_pidns_filter(Tracee *tracee, bool chain)
{
#if defined(ARCH_ARM64)
	const uint32_t audit_arch = AUDIT_ARCH_AARCH64;
#elif defined(ARCH_X86_64)
	const uint32_t audit_arch = AUDIT_ARCH_X86_64;
#else
	const uint32_t audit_arch = 0;
#endif
	/* The PID is their only argument.  */
	static const Sysnum self[] = { PR_getpid, PR_gettid, PR_getppid };
	/* The PID is their first argument.  */
	static const Sysnum by_arg0[] = {
		PR_kill, PR_tkill, PR_tgkill, PR_getpgid, PR_getsid, PR_prlimit64,
		PR_sched_getaffinity, PR_sched_setaffinity,
		PR_sched_getparam, PR_sched_setparam,
		PR_sched_getscheduler, PR_sched_setscheduler,
	};
	/* The PID is their second argument.  */
	static const Sysnum by_arg1[] = { PR_getpriority, PR_setpriority };

	const size_t nb_self = sizeof(self) / sizeof(self[0]);
	const size_t nb_arg0 = sizeof(by_arg0) / sizeof(by_arg0[0]);
	const size_t nb_arg1 = sizeof(by_arg1) / sizeof(by_arg1[0]);
	const size_t allow = 3 + nb_self + nb_arg0 + nb_arg1;
	const size_t check0 = allow + 1;
	const size_t check1 = check0 + 3;
	const size_t trace = check1 + 3;

	struct sock_filter filter[trace + 1];
	struct sock_fprog program;
	word_t sysargs[6];
	word_t address;
	Sysnum sysnum;
	size_t i, k;
	int status;

	tracee->pidns_filter = true;

	/* Without seccomp acceleration every syscall stops anyway.  */
	if (audit_arch == 0 || tracee->seccomp != ENABLED)
		return 0;

#define JUMP(from, to) ((to) - (from) - 1)

	k = 0;
	filter[k++] = (struct sock_filter) BPF_STMT(BPF_LD + BPF_W + BPF_ABS, offsetof(struct seccomp_data, arch));
	filter[k] = (struct sock_filter) BPF_JUMP(BPF_JMP + BPF_JEQ + BPF_K, audit_arch, 0, JUMP(k, allow));
	k++;
	filter[k++] = (struct sock_filter) BPF_STMT(BPF_LD + BPF_W + BPF_ABS, offsetof(struct seccomp_data, nr));

	for (i = 0; i < nb_self; i++, k++)
		filter[k] = (struct sock_filter) BPF_JUMP(BPF_JMP + BPF_JEQ + BPF_K,
				detranslate_sysnum(get_abi(tracee), self[i]), JUMP(k, trace), 0);
	for (i = 0; i < nb_arg0; i++, k++)
		filter[k] = (struct sock_filter) BPF_JUMP(BPF_JMP + BPF_JEQ + BPF_K,
				detranslate_sysnum(get_abi(tracee), by_arg0[i]), JUMP(k, check0), 0);
	for (i = 0; i < nb_arg1; i++, k++)
		filter[k] = (struct sock_filter) BPF_JUMP(BPF_JMP + BPF_JEQ + BPF_K,
				detranslate_sysnum(get_abi(tracee), by_arg1[i]), JUMP(k, check1), 0);

	/* The low 32 bits of the argument (little-endian) are PID 1.  */
	filter[k++] = (struct sock_filter) BPF_STMT(BPF_RET + BPF_K, SECCOMP_RET_ALLOW);
	filter[k++] = (struct sock_filter) BPF_STMT(BPF_LD + BPF_W + BPF_ABS, offsetof(struct seccomp_data, args[0]));
	filter[k] = (struct sock_filter) BPF_JUMP(BPF_JMP + BPF_JEQ + BPF_K, 1, JUMP(k, trace), 0);
	k++;
	filter[k++] = (struct sock_filter) BPF_STMT(BPF_RET + BPF_K, SECCOMP_RET_ALLOW);
	filter[k++] = (struct sock_filter) BPF_STMT(BPF_LD + BPF_W + BPF_ABS, offsetof(struct seccomp_data, args[1]));
	filter[k] = (struct sock_filter) BPF_JUMP(BPF_JMP + BPF_JEQ + BPF_K, 1, JUMP(k, trace), 0);
	k++;
	filter[k++] = (struct sock_filter) BPF_STMT(BPF_RET + BPF_K, SECCOMP_RET_ALLOW);
	filter[k++] = (struct sock_filter) BPF_STMT(BPF_RET + BPF_K, SECCOMP_RET_TRACE);

#undef JUMP

	/* Copy the program below the stack pointer, which is put back
	 * right away: the kernel copies the program in, and nothing
	 * runs in the tracee before that.  */
	address = alloc_mem(tracee, sizeof(filter) + sizeof(program));
	if (address == 0)
		return -EFAULT;

	program.len = k;
	program.filter = (struct sock_filter *) address;
	status = write_data(tracee, address, filter, sizeof(filter));
	if (status < 0)
		return status;
	status = write_data(tracee, address + sizeof(filter), &program, sizeof(program));
	if (status < 0)
		return status;
	poke_reg(tracee, STACK_POINTER, peek_reg(tracee, ORIGINAL, STACK_POINTER));

	sysnum = get_sysnum(tracee, CURRENT);
	sysargs[0] = peek_reg(tracee, CURRENT, SYSARG_1);
	sysargs[1] = peek_reg(tracee, CURRENT, SYSARG_2);
	sysargs[2] = peek_reg(tracee, CURRENT, SYSARG_3);
	sysargs[3] = peek_reg(tracee, CURRENT, SYSARG_4);
	sysargs[4] = peek_reg(tracee, CURRENT, SYSARG_5);
	sysargs[5] = peek_reg(tracee, CURRENT, SYSARG_6);

	/* The syscall number can't be changed once a chain is
	 * registered, so the restart is chained at the exit stage of
	 * seccomp(2), see fakens_exit_start().  */
	if (chain) {
		tracee->fakens_restart.pending = true;
		tracee->fakens_restart.sysnum = sysnum;
		memcpy(tracee->fakens_restart.args, sysargs, sizeof(sysargs));
	}

	set_sysnum(tracee, PR_seccomp);
	poke_reg(tracee, SYSARG_1, SECCOMP_SET_MODE_FILTER);
	poke_reg(tracee, SYSARG_2, 0);
	poke_reg(tracee, SYSARG_3, address + sizeof(filter));

	tracee->sysexit_pending = true;
	tracee->restart_how = PTRACE_SYSCALL;
	return 1;
}

/**
 * Handle clone(2)/clone3(2) with the namespace @flags PRoot is about
 * to strip.  This function returns -errno if an error occured,
 * otherwise 0.
 */
int fakens_enter_clone(Tracee *tracee, word_t flags)
{
	int status;

	if ((flags & CLONE_NEWUSER) != 0)
		tracee->clone_stripped_newuser = true;

	if ((flags & CLONE_NEWPID) == 0)
		return 0;

	tracee->clone_stripped_newpid = true;
	if (tracee->pidns_filter)
		return 0;

	status = install_pidns_filter(tracee, true);
	return status < 0 ? status : 0;
}

/**
 * Handle unshare(2) with @flags.  This function returns -errno if an
 * error occured, 1 if @tracee's syscall was replaced (and must not be
 * voided), otherwise 0.
 */
int fakens_enter_unshare(Tracee *tracee, word_t flags)
{
	if ((flags & CLONE_NEWUSER) != 0)
		enter_userns(tracee);

	if ((flags & CLONE_NEWPID) == 0)
		return 0;

	if (tracee->pidns_for_children == NULL || tracee->pidns_for_children->init != 0) {
		TALLOC_FREE(tracee->pidns_for_children);
		tracee->pidns_for_children = talloc_zero(tracee, struct fake_pidns);
		if (tracee->pidns_for_children == NULL)
			return -ENOMEM;
	}

	if (tracee->pidns_filter)
		return 0;

	return install_pidns_filter(tracee, false);
}

static int answer(Tracee *tracee, word_t result, bool *handled)
{
	poke_reg(tracee, SYSARG_RESULT, result);
	set_sysnum(tracee, PR_void);
	*handled = true;
	return 0;
}

static word_t result_of(long result)
{
	return (word_t) (result < 0 ? -errno : result);
}

/* The real PID behind @pid for @tracee.  */
static pid_t real_pid(const Tracee *tracee, pid_t pid)
{
	return (pid == 1 && tracee->pidns != NULL && tracee->pidns->init != 0)
		? tracee->pidns->init : pid;
}

/**
 * Answer the capget(2)/capset(2) of a @tracee in an emulated user
 * namespace, from the capabilities PRoot keeps for it.
 */
static int handle_capabilities(Tracee *tracee, Sysnum sysnum, bool *handled)
{
	struct __user_cap_header_struct header;
	struct __user_cap_data_struct data[2];
	word_t header_address = peek_reg(tracee, CURRENT, SYSARG_1);
	word_t data_address = peek_reg(tracee, CURRENT, SYSARG_2);
	uint64_t effective, permitted, inheritable;
	size_t count;
	int status;

	status = read_data(tracee, &header, header_address, sizeof(header));
	if (status < 0)
		return 0;

	switch (header.version) {
	case _LINUX_CAPABILITY_VERSION_1:
		count = 1;
		break;
	case _LINUX_CAPABILITY_VERSION_2:
	case _LINUX_CAPABILITY_VERSION_3:
		count = 2;
		break;
	default:
		/* Let the kernel report its preferred version.  */
		return 0;
	}

	if (header.pid != 0 && real_pid(tracee, header.pid) != tracee->pid)
		return 0;

	if (sysnum == PR_capget) {
		memset(data, 0, sizeof(data));
		data[0].effective   = (uint32_t) tracee->cap_effective;
		data[0].permitted   = (uint32_t) tracee->cap_permitted;
		data[0].inheritable = (uint32_t) tracee->cap_inheritable;
		data[1].effective   = (uint32_t) (tracee->cap_effective >> 32);
		data[1].permitted   = (uint32_t) (tracee->cap_permitted >> 32);
		data[1].inheritable = (uint32_t) (tracee->cap_inheritable >> 32);
		if (data_address != 0) {
			status = write_data(tracee, data_address, data, count * sizeof(data[0]));
			if (status < 0)
				return answer(tracee, (word_t) status, handled);
		}
		return answer(tracee, 0, handled);
	}

	memset(data, 0, sizeof(data));
	status = read_data(tracee, data, data_address, count * sizeof(data[0]));
	if (status < 0)
		return answer(tracee, (word_t) status, handled);

	effective   = data[0].effective   | ((uint64_t) data[1].effective << 32);
	permitted   = data[0].permitted   | ((uint64_t) data[1].permitted << 32);
	inheritable = data[0].inheritable | ((uint64_t) data[1].inheritable << 32);

	/* The rules of the kernel's cap_capset().  */
	if ((permitted & ~tracee->cap_permitted) != 0
	    || (effective & ~permitted) != 0
	    || (inheritable & ~(tracee->cap_inheritable | tracee->cap_permitted)) != 0)
		return answer(tracee, (word_t) -EPERM, handled);

	tracee->cap_effective = effective;
	tracee->cap_permitted = permitted;
	tracee->cap_inheritable = inheritable;
	return answer(tracee, 0, handled);
}

/**
 * Emulate the current syscall of @tracee if it depends on the
 * emulated namespaces; *@handled then tells it was answered.  This
 * function returns -errno if an error occured, otherwise 0.
 */
int fakens_enter(Tracee *tracee, Sysnum sysnum, bool *handled)
{
	const struct fake_pidns *ns = tracee->pidns;
	const pid_t tgid = fakens_tgid(tracee);
	word_t arg1 = peek_reg(tracee, CURRENT, SYSARG_1);
	word_t arg2 = peek_reg(tracee, CURRENT, SYSARG_2);
	word_t arg3 = peek_reg(tracee, CURRENT, SYSARG_3);
	word_t arg4 = peek_reg(tracee, CURRENT, SYSARG_4);
	uint8_t buffer[1024];
	pid_t pid;
	long result;
	int status;

	*handled = false;

	switch (sysnum) {
	case PR_capget:
	case PR_capset:
		return tracee->userns ? handle_capabilities(tracee, sysnum, handled) : 0;

	case PR_getpid:
		return (ns != NULL && ns->init == tgid) ? answer(tracee, 1, handled) : 0;

	case PR_gettid:
		return (ns != NULL && ns->init == tracee->pid) ? answer(tracee, 1, handled) : 0;

	case PR_getppid:
		if (ns == NULL || ns->init == 0)
			return 0;
		if (ns->init == tgid)
			return answer(tracee, 0, handled);
		/* The init of the namespace is PID 1 to its children.  */
		tracee->sysexit_pending = true;
		tracee->restart_how = PTRACE_SYSCALL;
		return 0;

	default:
		break;
	}

	/* The syscalls below target PID 1.  PRoot makes them itself
	 * rather than rewriting their argument, which a seccomp filter
	 * of the tracee checking that argument would then refuse.  */
	if (ns == NULL || ns->init == 0)
		return 0;

	pid = (sysnum == PR_getpriority || sysnum == PR_setpriority) ? (pid_t) arg2 : (pid_t) arg1;
	if (pid != 1)
		return 0;
	pid = ns->init;

	switch (sysnum) {
	case PR_kill:
		result = syscall(SYS_kill, pid, (int) arg2);
		break;

	case PR_tkill:
		result = syscall(SYS_tkill, pid, (int) arg2);
		break;

	case PR_tgkill:
		result = syscall(SYS_tgkill, pid, real_pid(tracee, (pid_t) arg2), (int) arg3);
		break;

	case PR_getpgid:
		result = syscall(SYS_getpgid, pid);
		break;

	case PR_getsid:
		result = syscall(SYS_getsid, pid);
		break;

	case PR_getpriority:
		if ((int) arg1 != PRIO_PROCESS)
			return 0;
		result = syscall(SYS_getpriority, (int) arg1, pid);
		break;

	case PR_setpriority:
		if ((int) arg1 != PRIO_PROCESS)
			return 0;
		result = syscall(SYS_setpriority, (int) arg1, pid, (int) arg3);
		break;

	case PR_sched_getscheduler:
		result = syscall(SYS_sched_getscheduler, pid);
		break;

	case PR_sched_getparam:
		result = syscall(SYS_sched_getparam, pid, buffer);
		if (result == 0 && arg2 != 0) {
			status = write_data(tracee, arg2, buffer, sizeof(int));
			if (status < 0)
				return answer(tracee, (word_t) status, handled);
		}
		break;

	case PR_sched_setparam:
	case PR_sched_setscheduler: {
		word_t param = (sysnum == PR_sched_setparam) ? arg2 : arg3;

		if (param != 0) {
			status = read_data(tracee, buffer, param, sizeof(int));
			if (status < 0)
				return answer(tracee, (word_t) status, handled);
		}
		result = (sysnum == PR_sched_setparam)
			? syscall(SYS_sched_setparam, pid, param != 0 ? buffer : NULL)
			: syscall(SYS_sched_setscheduler, pid, (int) arg2, param != 0 ? buffer : NULL);
		break;
	}

	case PR_sched_getaffinity:
		result = syscall(SYS_sched_getaffinity, pid,
				arg2 < sizeof(buffer) ? arg2 : sizeof(buffer), buffer);
		if (result > 0) {
			status = write_data(tracee, arg3, buffer, result);
			if (status < 0)
				return answer(tracee, (word_t) status, handled);
		}
		break;

	case PR_sched_setaffinity: {
		word_t size = arg2 < sizeof(buffer) ? arg2 : sizeof(buffer);

		status = read_data(tracee, buffer, arg3, size);
		if (status < 0)
			return answer(tracee, (word_t) status, handled);
		result = syscall(SYS_sched_setaffinity, pid, size, buffer);
		break;
	}

	case PR_prlimit64: {
		uint64_t new_limit[2];
		uint64_t old_limit[2];

		if (arg3 != 0) {
			status = read_data(tracee, new_limit, arg3, sizeof(new_limit));
			if (status < 0)
				return answer(tracee, (word_t) status, handled);
		}
		result = syscall(SYS_prlimit64, pid, (int) arg2,
				arg3 != 0 ? new_limit : NULL, arg4 != 0 ? old_limit : NULL);
		if (result == 0 && arg4 != 0) {
			status = write_data(tracee, arg4, old_limit, sizeof(old_limit));
			if (status < 0)
				return answer(tracee, (word_t) status, handled);
		}
		break;
	}

	default:
		return 0;
	}

	return answer(tracee, result_of(result), handled);
}

/**
 * Restart the fork-like syscall seccomp(2) replaced at the enter
 * stage, if any, now that @tracee is at the exit stage of the latter.
 * This function returns true if it did.
 */
bool fakens_exit_start(Tracee *tracee)
{
	const word_t *args = tracee->fakens_restart.args;

	if (!tracee->fakens_restart.pending)
		return false;
	tracee->fakens_restart.pending = false;

	(void) register_chained_syscall(tracee, tracee->fakens_restart.sysnum,
					args[0], args[1], args[2], args[3], args[4], args[5]);
	return true;
}

/**
 * Fix the result of @tracee's syscall @sysnum at its exit stage.
 */
void fakens_exit(Tracee *tracee, Sysnum sysnum)
{
	word_t result = peek_reg(tracee, CURRENT, SYSARG_RESULT);

	switch (sysnum) {
	case PR_getppid:
		if (tracee->pidns != NULL && tracee->pidns->init != 0
		    && (pid_t) result == tracee->pidns->init)
			poke_reg(tracee, SYSARG_RESULT, 1);
		break;

	case PR_execve:
	case PR_execveat: {
		Extension *extension;
		const Config *config;

		if (!tracee->userns || (int) result < 0)
			break;

		/* execve(2) drops the capabilities of anyone but root.  */
		extension = get_extension(tracee, fake_id0_callback);
		config = extension != NULL ? talloc_get_type_abort(extension->config, Config) : NULL;
		if (config != NULL && config->euid == 0) {
			tracee->cap_effective = ALL_CAPS;
			tracee->cap_permitted = ALL_CAPS;
		}
		else {
			tracee->cap_effective = 0;
			tracee->cap_permitted = 0;
		}
		break;
	}

	default:
		break;
	}
}

/**
 * Point @path, a host path translated for @tracee, to another
 * namespace of the same process if it names one the kernel doesn't
 * have: /proc/self/ns/user exists for sandboxes to find (see the top
 * of this file).
 */
void fakens_fix_ns_path(const Tracee *tracee, char path[PATH_MAX])
{
	static const char *const emulated[] = { "user", "pid", "pid_for_children", "ipc", NULL };
	struct stat statl;
	char *name;
	size_t i;

	/* The emulation relies on the PID namespace filter, see
	 * install_pidns_filter(): without it, keep reporting that
	 * these namespaces are not supported at all.  */
	if (tracee == NULL || tracee->seccomp != ENABLED)
		return;

	if (strncmp(path, "/proc/", 6) != 0)
		return;

	name = strrchr(path, '/');
	if (name == NULL || name - path < 4 || strncmp(name - 3, "/ns/", 4) != 0)
		return;
	name++;

	for (i = 0; emulated[i] != NULL; i++) {
		if (strcmp(name, emulated[i]) == 0)
			break;
	}
	if (emulated[i] == NULL)
		return;

	if (lstat(path, &statl) == 0 || errno != ENOENT)
		return;

	strcpy(name, "uts");
}
