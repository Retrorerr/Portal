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

#ifndef FAKENS_H
#define FAKENS_H

#include <stdbool.h>
#include <limits.h>   /* PATH_MAX, */

#include "tracee/tracee.h"
#include "syscall/sysnum.h"

/* An emulated PID namespace: its init process sees itself as PID 1.  */
struct fake_pidns {
	/* Real PID of the init process, 0 until it is forked.  */
	pid_t init;
};

extern pid_t fakens_tgid(const Tracee *tracee);
extern bool fakens_has_cap(const Tracee *tracee, int cap);
extern void fakens_new_child(Tracee *parent, Tracee *child, word_t clone_flags);
extern int fakens_enter_clone(Tracee *tracee, word_t flags);
extern int fakens_enter_unshare(Tracee *tracee, word_t flags);
extern int fakens_enter(Tracee *tracee, Sysnum sysnum, bool *handled);
extern bool fakens_exit_start(Tracee *tracee);
extern void fakens_exit(Tracee *tracee, Sysnum sysnum);
extern void fakens_fix_ns_path(const Tracee *tracee, char path[PATH_MAX]);

#endif /* FAKENS_H */
