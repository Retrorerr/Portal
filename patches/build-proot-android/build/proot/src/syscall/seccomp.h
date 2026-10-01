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

#ifndef SECCOMP_H
#define SECCOMP_H

#include "syscall/sysnum.h"
#include "tracee/tracee.h"
#include "attribute.h"
#include "arch.h"

typedef struct {
	Sysnum value;
	word_t flags;
} FilteredSysnum;

typedef struct {
	unsigned int value;
	size_t nb_abis;
	Abi abis[NB_MAX_ABIS];
} SeccompArch;

#define FILTERED_SYSNUM_END { PR_void, 0 }

#define FILTER_SYSEXIT  0x1

#ifdef __ANDROID__
/* Trace only when the first argument, a file descriptor, is
 * PROOT_TRACED_FD_BASE or higher.  */
#define FILTER_HIGH_FD  0x2
/* Trace clone(2) only when it asks for new namespaces.  */
#define FILTER_CLONE_NS 0x4
/* Trace rt_sigaction(2) only when it is about SIGSYS.  */
#define FILTER_SIGSYS   0x8
#else
#define FILTER_HIGH_FD  0
#define FILTER_CLONE_NS 0
#define FILTER_SIGSYS   0
#endif
#define FILTER_ARGS (FILTER_HIGH_FD | FILTER_CLONE_NS | FILTER_SIGSYS)

/* PRoot moves the descriptors it has to see every send, receive and
 * close of up here (see syscall/exit.c).  Guests rarely hold this many
 * files, and the default soft RLIMIT_NOFILE (1024) still allows it.  */
#define PROOT_TRACED_FD_BASE 900

extern int enable_syscall_filtering(const Tracee *tracee);
extern int filtered_sysnum_flags(const Tracee *tracee, Sysnum sysnum);

#endif /* SECCOMP_H */
