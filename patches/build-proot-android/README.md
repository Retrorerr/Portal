# build-proot-android

PRoot build scripts for Android. They produce PRoot binaries, statically linked with libtalloc, with unbundled loader and freely relocatable in a file tree.

Usage:
- Build or get prebuilt at https://github.com/green-green-avk/build-proot-android/tree/master/packages
- Unpack `<somewhere>`
- Run as `<somewhere>/root/bin/proot`\
for details, see https://github.com/green-green-avk/proot/blob/master/doc/usage/android/start-script-example
- ???
- Profit

How to build:
 - Dependencies: Android NDK / make / tar / gzip
 - Tune `config` file to match your environment
 - Run `./build.sh`

Vendored `build/proot` tracks the latest `termux/proot` `master` commit that
was current on March 31, 2026: `ab2e3464d04483b98a0614b470f3f8950d5a6468`
(committed on February 21, 2026). This repo still carries a small set of
downstream Android/APK build patches on top of that source snapshot.

Portal compatibility patches (September 2026), needed by coding agents such as
Claude Code and by ordinary developer tools:

- `fstat()`/`fstatat(fd, "", AT_EMPTY_PATH)` no longer fail with `ENOENT` on
  descriptors without a file-system path (sockets, epoll, eventfd, inotify,
  pseudo files); `fstat()` of a closed descriptor reports `EBADF`.
- `statx()` applies fake-root meta files like `stat()` does, so coreutils,
  Node and Bun see the recorded mode and ownership.
- `fchmodat2()` and `openat2()` return `ENOSYS` instead of reaching the host
  with untranslated guest paths (glibc and callers fall back).
- `-H` (hidden files) removes orphaned meta files from a directory that holds
  nothing else before `rmdir()`, so hidden bookkeeping never blocks `rm -rf`.

Portal launches PRoot with `-H` and `PROOT_L2S_DIR=<rootfs>/.proot.l2s`.

See https://github.com/green-green-avk/proot for more info.
