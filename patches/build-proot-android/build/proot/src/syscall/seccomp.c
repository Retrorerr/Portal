/* -*- c-set-style: "K&R"; c-basic-offset: 8 -*-
 *
 * This file is part of PRoot.
 *
 * Copyright (C) 2015 STMicroelectronics
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

#include "build.h"
#include "arch.h"

#if defined(HAVE_SECCOMP_FILTER)

#include <sys/prctl.h>     /* prctl(2), PR_* */
#include <linux/filter.h>  /* struct sock_*, */
#include <linux/seccomp.h> /* SECCOMP_MODE_FILTER, */
#include <linux/filter.h>  /* struct sock_*, */
#include <linux/audit.h>   /* AUDIT_, */
#include <sys/queue.h>     /* LIST_FOREACH, */
#include <sys/types.h>     /* size_t, */
#include <talloc.h>        /* talloc_*, */
#include <errno.h>         /* E*, */
#include <string.h>        /* memcpy(3), */
#include <stddef.h>        /* offsetof(3), */
#include <stdint.h>        /* uint*_t, UINT*_MAX, */
#include <assert.h>        /* assert(3), */
#include <termios.h>       /* TCSETS, TCGETS2, */
#include <sys/ioctl.h>     /* _IOW, */
#include <linux/sockios.h>  /* SIOCGIFINDEX, */
#include <sched.h>         /* CLONE_NEW*, */
#include <signal.h>        /* SIGSYS, */

#include "syscall/seccomp.h"
#include "tracee/tracee.h"
#include "syscall/syscall.h"
#include "syscall/sysnum.h"
#include "extension/extension.h"
#include "cli/note.h"

#include "compat.h"
#include "attribute.h"

#define DEBUG_FILTER(...) /* fprintf(stderr, __VA_ARGS__) */

/**
 * Allocate an empty @program->filter.  This function returns -errno
 * if an error occurred, otherwise 0.
 */
static int new_program_filter(struct sock_fprog *program)
{
	program->filter = talloc_array(NULL, struct sock_filter, 0);
	if (program->filter == NULL)
		return -ENOMEM;

	program->len = 0;
	return 0;
}

/**
 * Append to @program->filter the given @statements (@nb_statements
 * items).  This function returns -errno if an error occurred,
 * otherwise 0.
 */
static int add_statements(struct sock_fprog *program, size_t nb_statements,
			struct sock_filter *statements)
{
	size_t length;
	void *tmp;
	size_t i;

	length = talloc_array_length(program->filter);
	tmp  = talloc_realloc(NULL, program->filter, struct sock_filter, length + nb_statements);
	if (tmp == NULL)
		return -ENOMEM;
	program->filter = tmp;

	for (i = 0; i < nb_statements; i++, length++)
		memcpy(&program->filter[length], &statements[i], sizeof(struct sock_filter));

	return 0;
}

/**
 * Append to @program->filter the statements required to notify PRoot
 * about the given @syscall made by a tracee, with the given @flag.
 * This function returns -errno if an error occurred, otherwise 0.
 */
static int add_trace_syscall(struct sock_fprog *program, word_t syscall, int flag)
{
	int status;

	/* Sanity check.  */
	if (syscall > UINT32_MAX)
		return -ERANGE;

	#define LENGTH_TRACE_SYSCALL 2
	struct sock_filter statements[LENGTH_TRACE_SYSCALL] = {
		/* Compare the accumulator with the expected syscall:
		 * skip the next statement if not equal.  */
		BPF_JUMP(BPF_JMP + BPF_JEQ + BPF_K, syscall, 0, 1),

		/* Notify the tracer.  */
		BPF_STMT(BPF_RET + BPF_K, SECCOMP_RET_TRACE + flag)
	};

	DEBUG_FILTER("FILTER:     trace if syscall == %ld\n", syscall);

	status = add_statements(program, LENGTH_TRACE_SYSCALL, statements);
	if (status < 0)
		return status;

	return 0;
}

#ifdef __ANDROID__
/* ioctl(2) requests PRoot rewrites on Android (syscall/enter.c and
 * syscall/exit.c).  Every other request, GPU submissions and fence waits
 * included, runs without stopping the tracee: each ptrace stop costs tens
 * of microseconds on phones, so trapping all ioctls capped GPU clients at
 * a few hundred frames per second whatever the GPU could do.  */
static const uint32_t traced_ioctl_requests[] = {
	TCSETS + 2 /* + TCSAFLUSH */,
	TCGETS2,
	TCSETS2,
	TCSETSW2,
	TCSETSF2,
	_IOW(0x94, 9, int) /* FICLONE */,
	SIOCGIFINDEX,
};
#define NB_TRACED_IOCTLS (sizeof(traced_ioctl_requests) / sizeof(traced_ioctl_requests[0]))
#define LENGTH_TRACE_IOCTL (NB_TRACED_IOCTLS + 4)

/**
 * Like add_trace_syscall(), but only for the ioctl(2) requests listed in
 * traced_ioctl_requests; other requests are allowed straight away.
 */
static int add_trace_ioctl(struct sock_fprog *program, word_t syscall, int flag)
{
	/* The request is an unsigned int in the kernel: compare the low
	 * 32 bits of the second argument (little-endian).  */
	const size_t request_offset = offsetof(struct seccomp_data, args[1]);
	struct sock_filter statements[LENGTH_TRACE_IOCTL];
	size_t i;

	if (syscall > UINT32_MAX || request_offset > UINT32_MAX)
		return -ERANGE;

	/* Not ioctl(2): skip this block.  */
	statements[0] = (struct sock_filter) BPF_JUMP(BPF_JMP + BPF_JEQ + BPF_K, syscall, 0, LENGTH_TRACE_IOCTL - 1);
	statements[1] = (struct sock_filter) BPF_STMT(BPF_LD + BPF_W + BPF_ABS, request_offset);
	for (i = 0; i < NB_TRACED_IOCTLS; i++)
		statements[2 + i] = (struct sock_filter) BPF_JUMP(BPF_JMP + BPF_JEQ + BPF_K,
					traced_ioctl_requests[i], NB_TRACED_IOCTLS - i, 0);
	statements[2 + NB_TRACED_IOCTLS] = (struct sock_filter) BPF_STMT(BPF_RET + BPF_K, SECCOMP_RET_ALLOW);
	statements[3 + NB_TRACED_IOCTLS] = (struct sock_filter) BPF_STMT(BPF_RET + BPF_K, SECCOMP_RET_TRACE + flag);

	DEBUG_FILTER("FILTER:     trace if syscall == %ld and request is rewritten\n", syscall);

	return add_statements(program, LENGTH_TRACE_IOCTL, statements);
}

/* The namespace flags syscall/enter.c strips from clone(2).  */
#define CLONE_NS_FLAGS (CLONE_NEWNS | CLONE_NEWUTS | CLONE_NEWIPC | \
			CLONE_NEWUSER | CLONE_NEWPID | CLONE_NEWNET | \
			0x02000000 /* CLONE_NEWCGROUP */ | \
			0x00000080 /* CLONE_NEWTIME */)

#define LENGTH_TRACE_ARG 5

/**
 * Like add_trace_syscall(), but only when the first argument of
 * @syscall matches @flag: FILTER_HIGH_FD, a descriptor PRoot moved to
 * PROOT_TRACED_FD_BASE or higher (negative ones match too), or
 * FILTER_CLONE_NS, clone(2) flags asking for a namespace, or
 * FILTER_SIGSYS, the signal number SIGSYS.  close(2),
 * send(2) and recv(2) calls otherwise stop every Wayland, X11, D-Bus
 * and PipeWire message, and clone(2) every thread.
 */
static int add_trace_arg(struct sock_fprog *program, word_t syscall, int flag)
{
	/* Compare the low 32 bits of the first argument (little-endian).  */
	const size_t arg_offset = offsetof(struct seccomp_data, args[0]);
	struct sock_filter statements[LENGTH_TRACE_ARG];

	if (syscall > UINT32_MAX || arg_offset > UINT32_MAX)
		return -ERANGE;

	/* Not @syscall: skip this block.  */
	statements[0] = (struct sock_filter) BPF_JUMP(BPF_JMP + BPF_JEQ + BPF_K, syscall, 0, LENGTH_TRACE_ARG - 1);
	statements[1] = (struct sock_filter) BPF_STMT(BPF_LD + BPF_W + BPF_ABS, arg_offset);
	if ((flag & FILTER_HIGH_FD) != 0)
		statements[2] = (struct sock_filter) BPF_JUMP(BPF_JMP + BPF_JGE + BPF_K, PROOT_TRACED_FD_BASE, 1, 0);
	else if ((flag & FILTER_SIGSYS) != 0)
		statements[2] = (struct sock_filter) BPF_JUMP(BPF_JMP + BPF_JEQ + BPF_K, SIGSYS, 1, 0);
	else
		statements[2] = (struct sock_filter) BPF_JUMP(BPF_JMP + BPF_JSET + BPF_K, CLONE_NS_FLAGS, 1, 0);
	statements[3] = (struct sock_filter) BPF_STMT(BPF_RET + BPF_K, SECCOMP_RET_ALLOW);
	statements[4] = (struct sock_filter) BPF_STMT(BPF_RET + BPF_K, SECCOMP_RET_TRACE + (flag & FILTER_SYSEXIT));

	DEBUG_FILTER("FILTER:     trace if syscall == %ld and its first argument matches\n", syscall);

	return add_statements(program, LENGTH_TRACE_ARG, statements);
}
#endif

/**
 * Append to @program->filter the statements that allow anything (if
 * unfiltered).  Note that @section_length is used to make a
 * sanity check.  This function returns -errno if an error occurred,
 * otherwise 0.
 */
static int end_arch_section(struct sock_fprog *program, size_t section_length)
{
	int status;

	#define LENGTH_END_SECTION 1
	struct sock_filter statements[LENGTH_END_SECTION] = {
		BPF_STMT(BPF_RET + BPF_K, SECCOMP_RET_ALLOW)
	};

	DEBUG_FILTER("FILTER:     allow\n");

	status = add_statements(program, LENGTH_END_SECTION, statements);
	if (status < 0)
		return status;

	/* Sanity check, see start_arch_section().  */
	if (talloc_array_length(program->filter) - program->len != section_length)
		return -ERANGE;

	return 0;
}

/**
 * Append to @program->filter the statements that check the current
 * @architecture.  Note that @section_length is used to make a
 * sanity check.  This function returns -errno if an error occurred,
 * otherwise 0.
 */
static int start_arch_section(struct sock_fprog *program, uint32_t arch, size_t section_length)
{
	const size_t arch_offset    = offsetof(struct seccomp_data, arch);
	const size_t syscall_offset = offsetof(struct seccomp_data, nr);
	int status;

	/* Sanity checks.  */
	if (   arch_offset    > UINT32_MAX
	    || syscall_offset > UINT32_MAX
	    || section_length > UINT32_MAX - 1)
		return -ERANGE;

	#define LENGTH_START_SECTION 4
	struct sock_filter statements[LENGTH_START_SECTION] = {
		/* Load the current architecture into the
		 * accumulator.  */
		BPF_STMT(BPF_LD + BPF_W + BPF_ABS, arch_offset),

		/* Compare the accumulator with the expected
		 * architecture: skip the following statement if
		 * equal.  */
		BPF_JUMP(BPF_JMP + BPF_JEQ + BPF_K, arch, 1, 0),

		/* This is not the expected architecture, so jump
		 * unconditionally to the end of this section.  */
		BPF_STMT(BPF_JMP + BPF_JA + BPF_K, section_length + 1),

		/* This is the expected architecture, so load the
		 * current syscall into the accumulator.  */
		BPF_STMT(BPF_LD + BPF_W + BPF_ABS, syscall_offset)
	};

	DEBUG_FILTER("FILTER: if arch == %ld, up to %zdth statement\n",
		arch, section_length);

	status = add_statements(program, LENGTH_START_SECTION, statements);
	if (status < 0)
		return status;

	/* See the sanity check in end_arch_section().  */
	program->len = talloc_array_length(program->filter);

	return 0;
}

/**
 * Append to @program->filter the statements that forbid anything (if
 * unfiltered) and update @program->len.  This function returns -errno
 * if an error occurred, otherwise 0.
 */
static int finalize_program_filter(struct sock_fprog *program)
{
	int status;

	#define LENGTH_FINALIZE 1
	struct sock_filter statements[LENGTH_FINALIZE] = {
		BPF_STMT(BPF_RET + BPF_K, SECCOMP_RET_KILL)
	};

	DEBUG_FILTER("FILTER: kill\n");

	status = add_statements(program, LENGTH_FINALIZE, statements);
	if (status < 0)
		return status;

	program->len = talloc_array_length(program->filter);

	return 0;
}

/**
 * Free @program->filter and set @program->len to 0.
 */
static void free_program_filter(struct sock_fprog *program)
{
	TALLOC_FREE(program->filter);
	program->len = 0;
}

/**
 * Convert the given @sysnums into BPF filters according to the
 * following pseudo-code, then enabled them for the given @tracee and
 * all of its future children:
 *
 *     for each handled architectures
 *         for each filtered syscall
 *             trace
 *         allow
 *     kill
 *
 * This function returns -errno if an error occurred, otherwise 0.
 */
static int set_seccomp_filters(const FilteredSysnum *sysnums)
{
	SeccompArch seccomp_archs[] = SECCOMP_ARCHS;
	size_t nb_archs = sizeof(seccomp_archs) / sizeof(SeccompArch);

	struct sock_fprog program = { .len = 0, .filter = NULL };
	size_t section_length;
	size_t i, j, k;
	int status;

	status = new_program_filter(&program);
	if (status < 0)
		goto end;

	/* For each handled architectures */
	for (i = 0; i < nb_archs; i++) {
		word_t syscall;

		section_length = LENGTH_END_SECTION;

		/* Pre-compute the length of the filter for this architecture.  */
		for (j = 0; j < seccomp_archs[i].nb_abis; j++) {
			for (k = 0; sysnums[k].value != PR_void; k++) {
				syscall = detranslate_sysnum(seccomp_archs[i].abis[j], sysnums[k].value);
				if (syscall == SYSCALL_AVOIDER)
					continue;
#ifdef __ANDROID__
				if (sysnums[k].value == PR_ioctl) {
					section_length += LENGTH_TRACE_IOCTL;
					continue;
				}
				if ((sysnums[k].flags & FILTER_ARGS) != 0) {
					section_length += LENGTH_TRACE_ARG;
					continue;
				}
#endif
				section_length += LENGTH_TRACE_SYSCALL;
			}
		}

		/* Filter: if handled architecture */
		status = start_arch_section(&program, seccomp_archs[i].value, section_length);
		if (status < 0)
			goto end;

		for (j = 0; j < seccomp_archs[i].nb_abis; j++) {
			for (k = 0; sysnums[k].value != PR_void; k++) {
				/* Get the architecture specific syscall number.  */
				syscall = detranslate_sysnum(seccomp_archs[i].abis[j], sysnums[k].value);
				if (syscall == SYSCALL_AVOIDER)
					continue;

				/* Filter: trace if handled syscall */
#ifdef __ANDROID__
				if (sysnums[k].value == PR_ioctl)
					status = add_trace_ioctl(&program, syscall, sysnums[k].flags);
				else if ((sysnums[k].flags & FILTER_ARGS) != 0)
					status = add_trace_arg(&program, syscall, sysnums[k].flags);
				else
#endif
				status = add_trace_syscall(&program, syscall, sysnums[k].flags);
				if (status < 0)
					goto end;
			}
		}

		/* Filter: allow untraced syscalls for this architecture */
		status = end_arch_section(&program, section_length);
		if (status < 0)
			goto end;
	}

	status = finalize_program_filter(&program);
	if (status < 0)
		goto end;

	status = prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
	if (status < 0)
		goto end;

	/* To output this BPF program for debug purpose:
	 *
	 *     write(2, program.filter, program.len * sizeof(struct sock_filter));
	 */

	status = prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program);
	if (status < 0)
		goto end;

	status = 0;
end:
	free_program_filter(&program);
	return status;
}

/* List of sysnums handled by PRoot.  */
static FilteredSysnum proot_sysnums[] = {
	{ PR_accept,		FILTER_SYSEXIT },
	{ PR_accept4,		FILTER_SYSEXIT },
	{ PR_access,		0 },
	{ PR_acct,		0 },
	{ PR_bind,		FILTER_SYSEXIT },
	{ PR_brk,		FILTER_SYSEXIT },
	{ PR_chdir,		FILTER_SYSEXIT },
	{ PR_chmod,		0 },
	{ PR_chown,		0 },
	{ PR_chown32,		0 },
	{ PR_capget,		0 },
	{ PR_capset,		0 },
	{ PR_chroot,		0 },
	{ PR_clone,		FILTER_CLONE_NS },
	{ PR_clone3,		0 },
	{ PR_close,		FILTER_HIGH_FD },
	{ PR_connect,		0 },
	{ PR_creat,		0 },
	{ PR_recvfrom,		FILTER_HIGH_FD },
	{ PR_rt_sigaction,	FILTER_SIGSYS | FILTER_SYSEXIT },
	{ PR_recvmsg,		FILTER_HIGH_FD },
	{ PR_sendmsg,		FILTER_HIGH_FD },
	{ PR_sendto,		FILTER_HIGH_FD },
	{ PR_socket,		FILTER_SYSEXIT },
	{ PR_execve,		FILTER_SYSEXIT },
	{ PR_execveat,		FILTER_SYSEXIT },
	{ PR_faccessat,		0 },
	{ PR_faccessat2,	FILTER_SYSEXIT },
	{ PR_fchdir,		FILTER_SYSEXIT },
	{ PR_fchmodat,		0 },
	{ PR_fchmodat2,	FILTER_SYSEXIT },
	{ PR_fchownat,		0 },
	{ PR_fstatat64,		0 },
	{ PR_futimesat,		0 },
	{ PR_getcwd,		FILTER_SYSEXIT },
	{ PR_getpeername,	FILTER_SYSEXIT },
	{ PR_getsockname,	FILTER_SYSEXIT },
	{ PR_getxattr,		0 },
	{ PR_inotify_add_watch,	0 },
#ifdef __ANDROID__
	{ PR_ioctl,		FILTER_SYSEXIT },
#endif
	{ PR_lchown,		0 },
	{ PR_lchown32,		0 },
	{ PR_lgetxattr,		0 },
	{ PR_link,		0 },
	{ PR_linkat,		0 },
	{ PR_listxattr,		0 },
	{ PR_llistxattr,	0 },
	{ PR_lremovexattr,	0 },
	{ PR_lsetxattr,		0 },
	{ PR_lstat,		0 },
	{ PR_lstat64,		0 },
#ifdef __ANDROID__
	{ PR_memfd_create,	0 },
#endif
	{ PR_mkdir,		0 },
	{ PR_mkdirat,		0 },
	{ PR_mknod,		0 },
	{ PR_mknodat,		0 },
	{ PR_mount,		FILTER_SYSEXIT },
	{ PR_name_to_handle_at,	0 },
	{ PR_newfstatat,	0 },
	{ PR_oldlstat,		0 },
	{ PR_oldstat,		0 },
	{ PR_open,		0 },
	{ PR_openat,		0 },
	{ PR_openat2,		0 },
	{ PR_pivot_root,	FILTER_SYSEXIT },
	{ PR_prctl, 		0 },
	{ PR_prlimit64,		FILTER_SYSEXIT },
	{ PR_ptrace,		FILTER_SYSEXIT },
	{ PR_readlink,		FILTER_SYSEXIT },
	{ PR_readlinkat,	FILTER_SYSEXIT },
	{ PR_removexattr,	0 },
	{ PR_rename,		FILTER_SYSEXIT },
	{ PR_renameat,		FILTER_SYSEXIT },
	{ PR_renameat2,		FILTER_SYSEXIT },
	{ PR_rmdir,		0 },
	{ PR_setrlimit,		FILTER_SYSEXIT },
	{ PR_setxattr,		0 },
	{ PR_socketcall,	FILTER_SYSEXIT },
	{ PR_stat,		0 },
	{ PR_stat64,		0 },
	{ PR_statfs,		FILTER_SYSEXIT },
	{ PR_statfs64,		FILTER_SYSEXIT },
	{ PR_statx,		FILTER_SYSEXIT },
	{ PR_swapoff,		0 },
	{ PR_swapon,		0 },
	{ PR_symlink,		0 },
	{ PR_symlinkat,		0 },
	{ PR_truncate,		0 },
	{ PR_truncate64,	0 },
	{ PR_umount,		FILTER_SYSEXIT },
	{ PR_umount2,		FILTER_SYSEXIT },
	{ PR_uname,		FILTER_SYSEXIT },
	{ PR_unshare,		FILTER_SYSEXIT },
	{ PR_setns,		FILTER_SYSEXIT },
	{ PR_unlink,		0 },
	{ PR_unlinkat,		0 },
	{ PR_uselib,		0 },
	{ PR_utime,		FILTER_SYSEXIT },
	{ PR_utimensat,		0 },
	{ PR_utimes,		0 },
	{ PR_wait4,		FILTER_SYSEXIT },
	{ PR_waitpid,		FILTER_SYSEXIT },
	FILTERED_SYSNUM_END,
};

/**
 * Add the @new_sysnums to the list of filtered @sysnums, using the
 * given Talloc @context.  This function returns -errno if an error
 * occurred, otherwise 0.
 */
static int merge_filtered_sysnums(TALLOC_CTX *context, FilteredSysnum **sysnums,
				const FilteredSysnum *new_sysnums)
{
	size_t i, j;

	assert(sysnums != NULL);

	if (*sysnums == NULL) {
		/* Start with no sysnums but the terminator.  */
		*sysnums = talloc_array(context, FilteredSysnum, 1);
		if (*sysnums == NULL)
			return -ENOMEM;

		(*sysnums)[0].value = PR_void;
	}

	for (i = 0; new_sysnums[i].value != PR_void; i++) {
		/* Search for the given sysnum.  */
		for (j = 0; (*sysnums)[j].value != PR_void
			 && (*sysnums)[j].value != new_sysnums[i].value; j++)
			;

		if ((*sysnums)[j].value == PR_void) {
			/* No such sysnum, allocate a new entry.  */
			(*sysnums) = talloc_realloc(context, (*sysnums), FilteredSysnum, j + 2);
			if ((*sysnums) == NULL)
				return -ENOMEM;

			(*sysnums)[j] = new_sysnums[i];

			/* The last item is the terminator.  */
			(*sysnums)[j + 1].value = PR_void;
		}
		else
			/* The sysnum is already filtered, merge the
			 * flags.  An argument restriction stays only if
			 * every list asks for it.  */
			(*sysnums)[j].flags =
				(((*sysnums)[j].flags | new_sysnums[i].flags) & ~FILTER_ARGS)
				| ((*sysnums)[j].flags & new_sysnums[i].flags & FILTER_ARGS);
	}

	return 0;
}

/**
 * Tell the kernel to trace only syscalls handled by PRoot and its
 * extensions.  This filter will be enabled for the given @tracee and
 * all of its future children.  This function returns -errno if an
 * error occurred, otherwise 0.
 */
int enable_syscall_filtering(const Tracee *tracee)
{
	FilteredSysnum *filtered_sysnums = NULL;
	Extension *extension;
	int status;

	assert(tracee != NULL && tracee->ctx != NULL);

	/* Add the sysnums required by PRoot to the list of filtered
	 * sysnums.  TODO: only if path translation is required.  */
	status = merge_filtered_sysnums(tracee->ctx, &filtered_sysnums, proot_sysnums);
	if (status < 0)
		return status;

	/* Merge the sysnums required by the extensions to the list
	 * of filtered sysnums.  */
	if (tracee->extensions != NULL) {
		LIST_FOREACH(extension, tracee->extensions, link) {
			if (extension->filtered_sysnums == NULL)
				continue;

			status = merge_filtered_sysnums(tracee->ctx, &filtered_sysnums,
							extension->filtered_sysnums);
			if (status < 0)
				return status;
		}
	}

	status = set_seccomp_filters(filtered_sysnums);
	if (status < 0)
		return status;

	return 0;
}

/**
 * Return the flags the filter of enable_syscall_filtering() associates
 * with @sysnum for @tracee, ie. the data PTRACE_EVENT_SECCOMP stops of
 * that syscall report: the filter was built from the very same lists.
 */
int filtered_sysnum_flags(const Tracee *tracee, Sysnum sysnum)
{
	Extension *extension;
	int flags = 0;
	size_t i;

	for (i = 0; proot_sysnums[i].value != PR_void; i++) {
		if (proot_sysnums[i].value == sysnum)
			flags |= proot_sysnums[i].flags;
	}

	if (tracee->extensions == NULL)
		return flags;

	LIST_FOREACH(extension, tracee->extensions, link) {
		if (extension->filtered_sysnums == NULL)
			continue;

		for (i = 0; extension->filtered_sysnums[i].value != PR_void; i++) {
			if (extension->filtered_sysnums[i].value == sysnum)
				flags |= extension->filtered_sysnums[i].flags;
		}
	}

	return flags;
}

#else

#include "syscall/seccomp.h"
#include "tracee/tracee.h"
#include "attribute.h"

int enable_syscall_filtering(const Tracee *tracee UNUSED)
{
	return 0;
}

int filtered_sysnum_flags(const Tracee *tracee UNUSED, Sysnum sysnum UNUSED)
{
	return 0;
}

#endif /* defined(HAVE_SECCOMP_FILTER) */
