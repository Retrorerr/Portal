/* Emulated evdev game pads (Portal).
 *
 * Android gives apps no /dev/input or /dev/uinput, so Portal creates one
 * pseudo-terminal per controller, puts it in raw mode and links
 * /dev/input/eventN (a host directory bound onto the guest's /dev/input)
 * to its slave. Reads, writes and poll(2) then already behave like an
 * evdev node: the host writes struct input_event records on the master,
 * and force-feedback records the guest side produces come back on it.
 * Only the evdev ioctl(2) requests are missing; this file answers them for
 * every pty the host registered in its pad registry (one file per pty
 * number, holding the pad name).
 *
 * This is the same contract PRoot's syscall/evdev.c serves through
 * $PROOT_FAKE_EVDEV_DIR (patches/build-proot-android). The registry is the
 * `desc` sibling of the directory bound onto /dev/input
 * (`<files>/fake-input/{input,desc}`, src/android/gamepad.rs), so it needs
 * no state of its own across execs: the bind table already carries it.
 *
 * Every pad reports the layout of the kernel's xpad driver for an Xbox 360
 * controller, which SDL, Wine and Steam map without configuration. */

#include <stddef.h>
#include <stdint.h>
#include <linux/input.h>
#include "gamepad.h"
#include "errno_neg.h"
#include "io.h"
#include "path.h"
#include "raw_sys.h"
#include "sysnr.h"
#include "tawc_string.h"
#include "tawc_uapi.h"
#include "usercopy.h"

#define PAD_NAME_DEFAULT "Microsoft X-Box 360 pad"
#define PAD_EFFECTS 16
#define PAD_SLOTS 32
#define REGISTRY_INPUT_SUFFIX "/fake-input/input"
#define REGISTRY_LEAF "desc"
#define FAKE_EVDEV_FF_UPLOAD 0xffff
#define FAKE_EVDEV_FF_ERASE  0xfffe
#define TAWC_O_WRONLY   01
#define TAWC_O_NOCTTY   0400
#define TAWC_O_NONBLOCK 04000
#define TAWC_O_CLOEXEC  02000000

/* Rumble effect slots per pty, so ids stay stable across uploads. */
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
#define NO_SIZE(request) ((request) & ~(_IOC_SIZEMASK << _IOC_SIZESHIFT))

static void set_bit_in(unsigned long *bitmap, unsigned int bit)
{
	bitmap[bit / LONG_BITS] |= 1UL << (bit % LONG_BITS);
}

/* Host registry directory into `out`, or 0 when no pad input bind exists. */
static int registry(char *out, size_t cap)
{
	const size_t suffix = sizeof(REGISTRY_INPUT_SUFFIX) - 1;
	for (size_t i = 0; i < tawcroot_n_binds; i++) {
		const struct tawcroot_bind *bind = &tawcroot_binds[i];
		if (!bind->active || !tawc_streq(bind->dst, "dev/input"))
			continue;
		if (bind->src_len < suffix ||
		    !tawc_streq(bind->src + bind->src_len - suffix, REGISTRY_INPUT_SUFFIX))
			return 0;
		size_t keep = bind->src_len - (sizeof("input") - 1);
		if (keep + sizeof(REGISTRY_LEAF) > cap)
			return 0;
		memcpy(out, bind->src, keep);
		memcpy(out + keep, REGISTRY_LEAF, sizeof(REGISTRY_LEAF));
		return 1;
	}
	return 0;
}

/* Registry entry path for pty `pts` into `out`; 0 when unavailable. */
static int entry_path(int pts, char *out, size_t cap)
{
	if (!registry(out, cap))
		return 0;
	size_t pos = strlen(out);
	return tawc_str_append(out, cap, &pos, "/") == 0 &&
	       tawc_str_append_dec(out, cap, &pos, pts) == 0;
}

static void fd_link(int fd, char *out, size_t cap)
{
	size_t pos = 0;
	(void)tawc_str_append(out, cap, &pos, "/proc/self/fd/");
	(void)tawc_str_append_dec(out, cap, &pos, fd);
}

/* Number of the pty `fd` names when the host registered it as a pad,
 * otherwise -1. */
static int fd_pts(int fd)
{
	char link[48], target[64], entry[300];
	if (fd < 0)
		return -1;
	fd_link(fd, link, sizeof link);
	long length = TAWC_RAW(TAWC_SYS_readlinkat, AT_FDCWD, (long)link,
			       (long)target, sizeof(target) - 1, 0, 0);
	if (length <= 9)
		return -1;
	target[length] = '\0';
	if (!tawc_starts_with(target, "/dev/pts/"))
		return -1;
	long number = 0;
	for (const char *p = target + 9; *p; p++) {
		if (*p < '0' || *p > '9' || number > (1 << 20))
			return -1;
		number = number * 10 + (*p - '0');
	}
	if (!entry_path((int)number, entry, sizeof entry))
		return -1;
	if (TAWC_RAW(TAWC_SYS_faccessat, AT_FDCWD, (long)entry, 0 /* F_OK */, 0, 0, 0) != 0)
		return -1;
	return (int)number;
}

static void pad_name(int pts, char *name, size_t size)
{
	char entry[300];
	long length = -1;
	if (entry_path(pts, entry, sizeof entry)) {
		long fd = tawc_openat(AT_FDCWD, entry, O_RDONLY | TAWC_O_CLOEXEC, 0);
		if (fd >= 0) {
			length = TAWC_RAW(TAWC_SYS_read, fd, (long)name, (long)(size - 1), 0, 0, 0);
			tawc_close((int)fd);
		}
	}
	if (length > 0) {
		name[length] = '\0';
		for (long i = 0; i < length; i++)
			if (name[i] == '\r' || name[i] == '\n')
				name[i] = '\0';
	}
	if (length <= 0 || name[0] == '\0')
		(void)tawc_str_copy(name, size, PAD_NAME_DEFAULT);
}

static unsigned int *slots_for(int pts)
{
	size_t free_slot = PAD_SLOTS;
	for (size_t i = 0; i < PAD_SLOTS; i++) {
		if (effect_slots[i].used != 0 && effect_slots[i].pts == pts)
			return &effect_slots[i].used;
		if (effect_slots[i].used == 0 && free_slot == PAD_SLOTS)
			free_slot = i;
	}
	if (free_slot == PAD_SLOTS)
		free_slot = (size_t)pts % PAD_SLOTS;
	effect_slots[free_slot].pts = pts;
	effect_slots[free_slot].used = 0;
	return &effect_slots[free_slot].used;
}

/* Tell the host about a rumble effect by writing a record on the pty,
 * which it reads from the master side like the guest's play events. */
static void notify_host(int fd, unsigned short type, unsigned short code,
			int value, unsigned long magnitudes)
{
	struct input_event record;
	char link[48];
	memset(&record, 0, sizeof record);
	record.input_event_usec = magnitudes;
	record.type = type;
	record.code = code;
	record.value = value;
	fd_link(fd, link, sizeof link);
	long out = tawc_openat(AT_FDCWD, link,
			       TAWC_O_WRONLY | TAWC_O_NOCTTY | TAWC_O_NONBLOCK | TAWC_O_CLOEXEC, 0);
	if (out < 0)
		return;
	/* A closed pad has nobody to tell. */
	(void)TAWC_RAW(TAWC_SYS_write, out, (long)&record, sizeof record, 0, 0, 0);
	tawc_close((int)out);
}

static long copy_out(long arg, const void *data, size_t length, size_t room)
{
	if (length > room)
		length = room;
	if (length > 0 && tawc_copy_to_guest((void *)(uintptr_t)arg, data, length) < 0)
		return TAWC_EFAULT;
	return (long)length;
}

static long ok_or_fault(long copied)
{
	return copied < 0 ? copied : 0;
}

static long get_bits(unsigned int ev, long arg, size_t room)
{
	unsigned long bits[BITMAP_LONGS(KEY_MAX)];
	size_t longs;
	memset(bits, 0, sizeof bits);
	switch (ev) {
	case 0:
		set_bit_in(bits, EV_SYN);
		set_bit_in(bits, EV_KEY);
		set_bit_in(bits, EV_ABS);
		set_bit_in(bits, EV_FF);
		longs = BITMAP_LONGS(EV_MAX);
		break;
	case EV_KEY:
		for (size_t i = 0; i < ARRAY_LENGTH(pad_buttons); i++)
			set_bit_in(bits, pad_buttons[i]);
		longs = BITMAP_LONGS(KEY_MAX);
		break;
	case EV_ABS:
		for (size_t i = 0; i < ARRAY_LENGTH(pad_axes); i++)
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
		return TAWC_EINVAL;
	}
	return copy_out(arg, bits, longs * sizeof(unsigned long), room);
}

static long upload_effect(int pts, int fd, long arg)
{
	struct ff_effect effect;
	if (tawc_copy_from_guest(&effect, sizeof effect, (const void *)(uintptr_t)arg) < 0)
		return TAWC_EFAULT;
	if (effect.type != FF_RUMBLE)
		return TAWC_EINVAL;
	unsigned int *used = slots_for(pts);
	if (effect.id == -1) {
		int id;
		for (id = 0; id < PAD_EFFECTS; id++)
			if ((*used & (1U << id)) == 0)
				break;
		if (id == PAD_EFFECTS)
			return TAWC_ENOSPC;
		effect.id = (short)id;
		if (tawc_copy_to_guest((void *)(uintptr_t)arg, &effect, sizeof effect) < 0)
			return TAWC_EFAULT;
	} else if (effect.id < 0 || effect.id >= PAD_EFFECTS) {
		return TAWC_EINVAL;
	}
	*used |= 1U << effect.id;
	notify_host(fd, FAKE_EVDEV_FF_UPLOAD, (unsigned short)effect.id,
		    effect.replay.length,
		    ((unsigned long)effect.u.rumble.strong_magnitude << 16)
		    | effect.u.rumble.weak_magnitude);
	return 0;
}

static long erase_effect(int pts, int fd, long arg)
{
	unsigned int *used = slots_for(pts);
	int id = (int)arg;
	if (id < 0 || id >= PAD_EFFECTS || (*used & (1U << id)) == 0)
		return TAWC_EINVAL;
	*used &= ~(1U << id);
	notify_host(fd, FAKE_EVDEV_FF_ERASE, (unsigned short)id, 0, 0);
	return 0;
}

int tawcroot_gamepad_open_flags(const char *guest_path, int flags)
{
	/* A session leader with no terminal would otherwise adopt a pad's
	 * pty as its controlling terminal and get SIGHUP when the host
	 * unplugs the pad. Real evdev nodes ignore O_NOCTTY. */
	if (tawc_starts_with(guest_path, "/dev/input/"))
		flags |= TAWC_O_NOCTTY;
	return flags;
}

static long pad_ioctl(int fd, int pts, unsigned int request, long arg)
{
	const size_t size = _IOC_SIZE(request);
	const unsigned int nr = _IOC_NR(request);
	char name[128];

	if (_IOC_DIR(request) == _IOC_READ && nr >= 0x20 && nr < 0x20 + EV_CNT)
		return get_bits(nr - 0x20, arg, size);
	if (_IOC_DIR(request) == _IOC_READ && nr >= 0x40 && nr < 0x40 + ABS_CNT) {
		for (size_t i = 0; i < ARRAY_LENGTH(pad_axes); i++)
			if (pad_axes[i].code == nr - 0x40)
				return ok_or_fault(copy_out(arg, &pad_axes[i].info,
							    sizeof(pad_axes[i].info), size));
		return TAWC_EINVAL;
	}
	if (_IOC_DIR(request) == _IOC_WRITE && nr >= 0xc0 && nr < 0xc0 + ABS_CNT)
		return 0; /* EVIOCSABS: the layout is fixed. */

	switch (NO_SIZE(request)) {
	case NO_SIZE(EVIOCGVERSION): {
		int version = EV_VERSION;
		return ok_or_fault(copy_out(arg, &version, sizeof version, size));
	}
	case NO_SIZE(EVIOCGID): {
		struct input_id id = { BUS_USB, 0x045e, 0x028e, 0x0114 };
		return ok_or_fault(copy_out(arg, &id, sizeof id, size));
	}
	case EVIOCGNAME(0):
		pad_name(pts, name, sizeof name);
		return copy_out(arg, name, strlen(name) + 1, size);
	case EVIOCGPHYS(0): {
		size_t pos = 0;
		(void)tawc_str_append(name, sizeof name, &pos, "portal-gamepad-");
		(void)tawc_str_append_dec(name, sizeof name, &pos, pts);
		(void)tawc_str_append(name, sizeof name, &pos, "/input0");
		return copy_out(arg, name, strlen(name) + 1, size);
	}
	case EVIOCGUNIQ(0):
		return TAWC_ENOENT;
	case EVIOCGPROP(0): {
		unsigned long props[BITMAP_LONGS(INPUT_PROP_MAX)] = { 0 };
		return copy_out(arg, props, sizeof props, size);
	}
	case EVIOCGKEY(0):
	case EVIOCGLED(0):
	case EVIOCGSND(0):
	case EVIOCGSW(0): {
		/* Nothing held: the host sends the current state as events. */
		unsigned long state[BITMAP_LONGS(KEY_MAX)] = { 0 };
		size_t length = NO_SIZE(request) == EVIOCGKEY(0)
			? sizeof state : sizeof(unsigned long);
		return copy_out(arg, state, length, size);
	}
	case NO_SIZE(EVIOCGEFFECTS): {
		int effects = PAD_EFFECTS;
		return ok_or_fault(copy_out(arg, &effects, sizeof effects, size));
	}
	case NO_SIZE(EVIOCSFF):
		return size < sizeof(struct ff_effect) ? TAWC_EINVAL : upload_effect(pts, fd, arg);
	case NO_SIZE(EVIOCRMFF):
		return erase_effect(pts, fd, arg);
	case NO_SIZE(EVIOCGRAB):
	case NO_SIZE(EVIOCREVOKE):
	case NO_SIZE(EVIOCSCLOCKID):
	case NO_SIZE(EVIOCSMASK):
		return 0;
	case NO_SIZE(EVIOCGREP):
	case NO_SIZE(EVIOCSREP):
		return TAWC_ENOSYS; /* No EV_REP, as on the kernel's xpad. */
	default:
		return TAWC_EINVAL;
	}
}

int tawcroot_gamepad_ioctl(int fd, unsigned int request, long arg, long *result)
{
	if (_IOC_TYPE(request) != 'E')
		return 0;
	int pts = fd_pts(fd);
	if (pts < 0)
		return 0;
	*result = pad_ioctl(fd, pts, request, arg);
	return 1;
}
