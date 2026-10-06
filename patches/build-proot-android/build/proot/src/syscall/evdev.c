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

/* Emulated evdev game pads.
 *
 * Android gives apps no /dev/input or /dev/uinput, so the host app
 * creates one pseudo-terminal per controller, puts it in raw mode and
 * links /dev/input/eventN to its slave.  Reads, writes and poll(2) then
 * already behave like an evdev node: the host writes struct input_event
 * records on the master, and force-feedback play events the guest writes
 * come back on it.  Only the evdev ioctl(2) requests are missing, and the
 * tracer answers those here for every pty the host registered in
 * $PROOT_FAKE_EVDEV_DIR (one file per pty number, holding the pad name).
 *
 * Every pad reports the layout of the kernel's xpad driver for an Xbox 360
 * controller, which SDL, Wine and Steam map without configuration.  The
 * ioctl(2) path only runs while a program enumerates or uploads rumble
 * effects; events never stop the tracee.  */

#include <errno.h>       /* E*, */
#include <fcntl.h>       /* open(2), O_*, */
#include <limits.h>      /* PATH_MAX, */
#include <stdio.h>       /* snprintf(3), */
#include <stdlib.h>      /* getenv(3), strtol(3), */
#include <string.h>      /* memset(3), strlen(3), */
#include <unistd.h>      /* readlink(2), access(2), */
#include <linux/input.h> /* EVIOC*, struct input_*, struct ff_effect, */

#include "syscall/evdev.h"
#include "syscall/syscall.h"
#include "tracee/mem.h"
#include "tracee/reg.h"

#define PAD_NAME_DEFAULT "Microsoft X-Box 360 pad"
#define PAD_EFFECTS 16
#define PAD_SLOTS 32

/* Rumble effect slots per pty, so ids stay stable across uploads.  */
static struct {
	int pts;
	unsigned int used;
} effect_slots[PAD_SLOTS];

static const unsigned short pad_buttons[] = {
	BTN_A, BTN_B, BTN_X, BTN_Y, BTN_TL, BTN_TR, BTN_SELECT, BTN_START,
	BTN_MODE, BTN_THUMBL, BTN_THUMBR,
};

static const struct {
	unsigned short code;
	struct input_absinfo info;
} pad_axes[] = {
	{ ABS_X,     { 0, -32768, 32767, 16, 128, 0 } },
	{ ABS_Y,     { 0, -32768, 32767, 16, 128, 0 } },
	{ ABS_Z,     { 0, 0, 255, 0, 0, 0 } },
	{ ABS_RX,    { 0, -32768, 32767, 16, 128, 0 } },
	{ ABS_RY,    { 0, -32768, 32767, 16, 128, 0 } },
	{ ABS_RZ,    { 0, 0, 255, 0, 0, 0 } },
	{ ABS_HAT0X, { 0, -1, 1, 0, 0, 0 } },
	{ ABS_HAT0Y, { 0, -1, 1, 0, 0, 0 } },
};

#define ARRAY_LENGTH(a) (sizeof(a) / sizeof((a)[0]))
#define LONG_BITS (8 * sizeof(unsigned long))
#define BITMAP_LONGS(max) (((max) + LONG_BITS) / LONG_BITS)

static void set_bit_in(unsigned long *bitmap, unsigned int bit)
{
	bitmap[bit / LONG_BITS] |= 1UL << (bit % LONG_BITS);
}

static const char *registry(void)
{
	static const char *dir = NULL;
	static bool looked = false;

	if (!looked) {
		dir = getenv("PROOT_FAKE_EVDEV_DIR");
		if (dir != NULL && dir[0] == '\0')
			dir = NULL;
		looked = true;
	}
	return dir;
}

/**
 * Return the number of the pty slave @host_path names when the host
 * registered it as a pad, otherwise -1.
 */
static int registered_pts(const char *host_path)
{
	char entry[PATH_MAX];
	const char *dir = registry();
	char *end;
	long number;

	if (dir == NULL || strncmp(host_path, "/dev/pts/", 9) != 0)
		return -1;
	number = strtol(host_path + 9, &end, 10);
	if (end == host_path + 9 || *end != '\0' || number < 0 || number > 1 << 20)
		return -1;
	if (snprintf(entry, sizeof(entry), "%s/%ld", dir, number) >= (int) sizeof(entry))
		return -1;
	if (access(entry, F_OK) != 0)
		return -1;
	return (int) number;
}

static int fd_pts(const Tracee *tracee, word_t fd)
{
	char link[64];
	char target[PATH_MAX];
	ssize_t length;

	if (registry() == NULL || (int) fd < 0)
		return -1;
	snprintf(link, sizeof(link), "/proc/%d/fd/%d", tracee->pid, (int) fd);
	length = readlink(link, target, sizeof(target) - 1);
	if (length <= 0)
		return -1;
	target[length] = '\0';
	return registered_pts(target);
}

static void pad_name(int pts, char *name, size_t size)
{
	char entry[PATH_MAX];
	ssize_t length = -1;
	int fd;

	snprintf(entry, sizeof(entry), "%s/%d", registry(), pts);
	fd = open(entry, O_RDONLY | O_CLOEXEC);
	if (fd >= 0) {
		length = read(fd, name, size - 1);
		close(fd);
	}
	if (length <= 0) {
		snprintf(name, size, "%s", PAD_NAME_DEFAULT);
		return;
	}
	name[length] = '\0';
	name[strcspn(name, "\r\n")] = '\0';
	if (name[0] == '\0')
		snprintf(name, size, "%s", PAD_NAME_DEFAULT);
}

static unsigned int *slots_for(int pts)
{
	size_t i;
	size_t free_slot = PAD_SLOTS;

	for (i = 0; i < PAD_SLOTS; i++) {
		if (effect_slots[i].used != 0 && effect_slots[i].pts == pts)
			return &effect_slots[i].used;
		if (effect_slots[i].used == 0 && free_slot == PAD_SLOTS)
			free_slot = i;
	}
	if (free_slot == PAD_SLOTS)
		free_slot = (size_t) pts % PAD_SLOTS;
	effect_slots[free_slot].pts = pts;
	effect_slots[free_slot].used = 0;
	return &effect_slots[free_slot].used;
}

/**
 * Tell the host about a rumble effect by writing a record on the pty,
 * which it reads from the master side like the guest's play events.
 */
static void notify_host(const Tracee *tracee, word_t fd, unsigned short type,
			unsigned short code, int value, unsigned long magnitudes)
{
	struct input_event record;
	char link[64];
	int out;

	memset(&record, 0, sizeof(record));
	record.input_event_usec = magnitudes;
	record.type = type;
	record.code = code;
	record.value = value;

	snprintf(link, sizeof(link), "/proc/%d/fd/%d", tracee->pid, (int) fd);
	out = open(link, O_WRONLY | O_NOCTTY | O_NONBLOCK | O_CLOEXEC);
	if (out < 0)
		return;
	if (write(out, &record, sizeof(record)) < 0) {
		/* The host closed the pad: nothing to tell.  */
	}
	close(out);
}

static int copy_out(Tracee *tracee, word_t arg, const void *data, size_t length, size_t room)
{
	if (length > room)
		length = room;
	if (length > 0 && write_data(tracee, arg, data, length) < 0)
		return -EFAULT;
	return (int) length;
}

static int get_bits(Tracee *tracee, unsigned int ev, word_t arg, size_t room)
{
	unsigned long bits[BITMAP_LONGS(KEY_MAX)];
	size_t longs;
	size_t i;

	memset(bits, 0, sizeof(bits));
	switch (ev) {
	case 0:
		set_bit_in(bits, EV_SYN);
		set_bit_in(bits, EV_KEY);
		set_bit_in(bits, EV_ABS);
		set_bit_in(bits, EV_FF);
		longs = BITMAP_LONGS(EV_MAX);
		break;
	case EV_KEY:
		for (i = 0; i < ARRAY_LENGTH(pad_buttons); i++)
			set_bit_in(bits, pad_buttons[i]);
		longs = BITMAP_LONGS(KEY_MAX);
		break;
	case EV_ABS:
		for (i = 0; i < ARRAY_LENGTH(pad_axes); i++)
			set_bit_in(bits, pad_axes[i].code);
		longs = BITMAP_LONGS(ABS_MAX);
		break;
	case EV_FF:
		set_bit_in(bits, FF_RUMBLE);
		set_bit_in(bits, FF_GAIN);
		longs = BITMAP_LONGS(FF_MAX);
		break;
	case EV_REL: longs = BITMAP_LONGS(REL_MAX); break;
	case EV_MSC: longs = BITMAP_LONGS(MSC_MAX); break;
	case EV_LED: longs = BITMAP_LONGS(LED_MAX); break;
	case EV_SND: longs = BITMAP_LONGS(SND_MAX); break;
	case EV_SW:  longs = BITMAP_LONGS(SW_MAX); break;
	default:
		return -EINVAL;
	}
	return copy_out(tracee, arg, bits, longs * sizeof(unsigned long), room);
}

static int upload_effect(Tracee *tracee, int pts, word_t fd, word_t arg)
{
	struct ff_effect effect;
	unsigned int *used;
	int id;

	if (read_data(tracee, &effect, arg, sizeof(effect)) < 0)
		return -EFAULT;
	if (effect.type != FF_RUMBLE)
		return -EINVAL;

	used = slots_for(pts);
	if (effect.id == -1) {
		for (id = 0; id < PAD_EFFECTS; id++)
			if ((*used & (1U << id)) == 0)
				break;
		if (id == PAD_EFFECTS)
			return -ENOSPC;
		effect.id = (short) id;
		if (write_data(tracee, arg, &effect, sizeof(effect)) < 0)
			return -EFAULT;
	} else if (effect.id < 0 || effect.id >= PAD_EFFECTS) {
		return -EINVAL;
	}
	*used |= 1U << effect.id;

	notify_host(tracee, fd, FAKE_EVDEV_FF_UPLOAD, (unsigned short) effect.id,
		    effect.replay.length,
		    ((unsigned long) effect.u.rumble.strong_magnitude << 16)
		    | effect.u.rumble.weak_magnitude);
	return 0;
}

static int erase_effect(Tracee *tracee, int pts, word_t fd, word_t arg)
{
	unsigned int *used = slots_for(pts);
	int id = (int) arg;

	if (id < 0 || id >= PAD_EFFECTS || (*used & (1U << id)) == 0)
		return -EINVAL;
	*used &= ~(1U << id);
	notify_host(tracee, fd, FAKE_EVDEV_FF_ERASE, (unsigned short) id, 0, 0);
	return 0;
}

/**
 * Answer the evdev ioctl(2) @cmd on @fd when it is a registered pad:
 * store the syscall result in @result and return true.  Return false to
 * let the kernel handle any other descriptor.
 */
bool fake_evdev_ioctl(Tracee *tracee, word_t fd, word_t cmd, word_t arg, int *result)
{
	const unsigned int request = (unsigned int) cmd;
	const size_t size = _IOC_SIZE(request);
	const unsigned int nr = _IOC_NR(request);
	char name[128];
	int pts;

	if (_IOC_TYPE(request) != FAKE_EVDEV_IOCTL_TYPE)
		return false;
	pts = fd_pts(tracee, fd);
	if (pts < 0)
		return false;

	if (_IOC_DIR(request) == _IOC_READ && nr >= 0x20 && nr < 0x20 + EV_CNT) {
		*result = get_bits(tracee, nr - 0x20, arg, size);
		return true;
	}
	if (_IOC_DIR(request) == _IOC_READ && nr >= 0x40 && nr < 0x40 + ABS_CNT) {
		size_t i;
		*result = -EINVAL;
		for (i = 0; i < ARRAY_LENGTH(pad_axes); i++) {
			if (pad_axes[i].code == nr - 0x40) {
				*result = copy_out(tracee, arg, &pad_axes[i].info,
						   sizeof(pad_axes[i].info), size) < 0 ? -EFAULT : 0;
				break;
			}
		}
		return true;
	}
	if (_IOC_DIR(request) == _IOC_WRITE && nr >= 0xc0 && nr < 0xc0 + ABS_CNT) {
		*result = 0; /* EVIOCSABS: the layout is fixed.  */
		return true;
	}

	switch (request & ~(_IOC_SIZEMASK << _IOC_SIZESHIFT)) {
	case EVIOCGVERSION & ~(_IOC_SIZEMASK << _IOC_SIZESHIFT): {
		int version = EV_VERSION;
		*result = copy_out(tracee, arg, &version, sizeof(version), size) < 0 ? -EFAULT : 0;
		return true;
	}
	case EVIOCGID & ~(_IOC_SIZEMASK << _IOC_SIZESHIFT): {
		struct input_id id = { BUS_USB, 0x045e, 0x028e, 0x0114 };
		*result = copy_out(tracee, arg, &id, sizeof(id), size) < 0 ? -EFAULT : 0;
		return true;
	}
	case EVIOCGNAME(0):
		pad_name(pts, name, sizeof(name));
		*result = copy_out(tracee, arg, name, strlen(name) + 1, size);
		return true;
	case EVIOCGPHYS(0):
		snprintf(name, sizeof(name), "portal-gamepad-%d/input0", pts);
		*result = copy_out(tracee, arg, name, strlen(name) + 1, size);
		return true;
	case EVIOCGUNIQ(0):
		*result = -ENOENT;
		return true;
	case EVIOCGPROP(0): {
		unsigned long props[BITMAP_LONGS(INPUT_PROP_MAX)] = { 0 };
		*result = copy_out(tracee, arg, props, sizeof(props), size);
		return true;
	}
	case EVIOCGKEY(0):
	case EVIOCGLED(0):
	case EVIOCGSND(0):
	case EVIOCGSW(0): {
		/* Nothing held: the host sends the current state as events.  */
		unsigned long state[BITMAP_LONGS(KEY_MAX)] = { 0 };
		size_t length = (request & ~(_IOC_SIZEMASK << _IOC_SIZESHIFT)) == EVIOCGKEY(0)
			? sizeof(state) : sizeof(unsigned long);
		*result = copy_out(tracee, arg, state, length, size);
		return true;
	}
	case EVIOCGEFFECTS & ~(_IOC_SIZEMASK << _IOC_SIZESHIFT): {
		int effects = PAD_EFFECTS;
		*result = copy_out(tracee, arg, &effects, sizeof(effects), size) < 0 ? -EFAULT : 0;
		return true;
	}
	case EVIOCSFF & ~(_IOC_SIZEMASK << _IOC_SIZESHIFT):
		*result = size < sizeof(struct ff_effect) ? -EINVAL : upload_effect(tracee, pts, fd, arg);
		return true;
	case EVIOCRMFF & ~(_IOC_SIZEMASK << _IOC_SIZESHIFT):
		*result = erase_effect(tracee, pts, fd, arg);
		return true;
	case EVIOCGRAB & ~(_IOC_SIZEMASK << _IOC_SIZESHIFT):
	case EVIOCREVOKE & ~(_IOC_SIZEMASK << _IOC_SIZESHIFT):
	case EVIOCSCLOCKID & ~(_IOC_SIZEMASK << _IOC_SIZESHIFT):
	case EVIOCSMASK & ~(_IOC_SIZEMASK << _IOC_SIZESHIFT):
		*result = 0;
		return true;
	case EVIOCGREP & ~(_IOC_SIZEMASK << _IOC_SIZESHIFT):
	case EVIOCSREP & ~(_IOC_SIZEMASK << _IOC_SIZESHIFT):
		*result = -ENOSYS; /* No EV_REP, as on the kernel's xpad.  */
		return true;
	default:
		*result = -EINVAL;
		return true;
	}
}

/**
 * Open registered pads with O_NOCTTY: a session leader with no terminal
 * would otherwise adopt the pad's pty as its controlling
 * terminal and get SIGHUP when the host unplugs the pad.
 */
void fake_evdev_open(Tracee *tracee, Reg path_reg, Reg flags_reg)
{
	char host_path[PATH_MAX];
	word_t flags;

	if (registry() == NULL)
		return;
	if (get_sysarg_path(tracee, host_path, path_reg) < 0)
		return;
	if (registered_pts(host_path) < 0)
		return;
	flags = peek_reg(tracee, CURRENT, flags_reg);
	if ((flags & O_NOCTTY) == 0)
		poke_reg(tracee, flags_reg, flags | O_NOCTTY);
}
