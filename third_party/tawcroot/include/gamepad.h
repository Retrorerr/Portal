#pragma once
/* Emulated evdev game pads (see src/gamepad.c). */

/* `flags` for a guest open of `guest_path`, with O_NOCTTY added for pads. */
int tawcroot_gamepad_open_flags(const char *guest_path, int flags);

/* Answer an evdev ioctl on a registered pad: store the syscall result in
 * *result and return 1. Return 0 for any other descriptor or request. */
int tawcroot_gamepad_ioctl(int fd, unsigned int request, long arg, long *result);
