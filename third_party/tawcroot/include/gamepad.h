#pragma once
/* Emulated evdev game pads (see src/gamepad.c). */

/* `flags` for a guest open of `guest_path`, with O_NOCTTY added for pads. */
int tawcroot_gamepad_open_flags(const char *guest_path, int flags);

/* A path which could refer to a pad, including eventN relative opens. */
int tawcroot_gamepad_path(const char *guest_path);

/* Replace a pad's discovery fd with this open's own event queue. */
int tawcroot_gamepad_reader(int fd, int flags);

/* Answer an evdev ioctl on a registered pad: store the syscall result in
 * *result and return 1. Return 0 for any other descriptor or request. */
int tawcroot_gamepad_ioctl(int fd, unsigned int request, long arg, long *result);
