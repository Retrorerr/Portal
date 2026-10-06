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

#ifndef EVDEV_H
#define EVDEV_H

#include <stdbool.h>

#include "tracee/tracee.h"
#include "tracee/reg.h"

/* Type byte of the evdev ioctl(2) requests the seccomp filter traps.  */
#define FAKE_EVDEV_IOCTL_TYPE 'E'

/* Records the tracer writes on a fake pad's pty for the host to read,
 * shaped like struct input_event.  Real event types stop at EV_MAX.  */
#define FAKE_EVDEV_FF_UPLOAD 0xffff
#define FAKE_EVDEV_FF_ERASE  0xfffe

extern bool fake_evdev_ioctl(Tracee *tracee, word_t fd, word_t cmd, word_t arg, int *result);
extern void fake_evdev_open(Tracee *tracee, Reg path_reg, Reg flags_reg);

#endif /* EVDEV_H */
