/*
 * Test-only libudev interposer for KWin's missing-monitor regression.
 *
 * It deliberately makes udev_monitor_new_from_netlink() fail while leaving
 * udev_new() and render-node enumeration untouched.  Build this file as a
 * native guest shared object and load it with LD_PRELOAD.  Never install it
 * into the APK or guest image.
 */
#define _GNU_SOURCE

#include <errno.h>

struct udev;
struct udev_monitor;

struct udev_monitor *udev_monitor_new_from_netlink(struct udev *udev, const char *name)
{
    (void)udev;
    (void)name;
    errno = ENOSYS;
    return 0;
}
