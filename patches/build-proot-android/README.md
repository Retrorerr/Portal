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

Vendored `build/proot` is `termux/proot` `master` at
`d4d2a19081c3c07f75250e4ce2980b9fa2f5720f` (September 24, 2026, tag
v5.1.107.95), merged with the Portal patches below.

Portal compatibility patches (September 2026), needed by coding agents such as
Claude Code and by ordinary developer tools:

- `fstat()`/`fstatat(fd, "", AT_EMPTY_PATH)` no longer fail with `ENOENT` on
  descriptors without a file-system path (sockets, epoll, eventfd, inotify,
  pseudo files); `fstat()` of a closed descriptor reports `EBADF`.
- `statx()` applies fake-root meta files like `stat()` does, so coreutils,
  Node and Bun see the recorded mode and ownership.
- `fchmodat2()` returns `ENOSYS` instead of reaching the host
  with untranslated guest paths (glibc and callers fall back).
- `-H` (hidden files) removes orphaned meta files from a directory that holds
  nothing else before `rmdir()`, so hidden bookkeeping never blocks `rm -rf`.
- `-H` leaves failed `getdents()` results (a directory removed while open,
  `ENOTDIR`) untouched and filters in heap buffers sized by the kernel's
  result. It used to read an errno as a byte count into stack arrays sized by
  the caller's buffer, which killed PRoot and every process it traced.

Portal performance patches (September 2026):

- The seccomp filter traces only the `ioctl()` requests PRoot rewrites.
- `close()`, `send*()` and `recv*()` are traced only on descriptors of
  `PROOT_TRACED_FD_BASE` (900) or more. Upstream traces all of them for its
  netlink emulation, which put every Wayland, X11, D-Bus and PipeWire message
  through a ptrace stop (0.7 to 36 us per send/recv on a OnePlus Pad 3).
  Emulated netlink sockets are moved up there when they are created.
  Upstream's pipe shadowing, which also relied on tracing every `close()`,
  is therefore inactive; process substitution works without it on Portal.
- `clone()` is traced only when it asks for new namespaces.
- `PROOT_SPIN_US` makes the tracer poll briefly before blocking in
  `waitpid()`.
- Netlink uevent sockets become route sockets whose `bind()` succeeds, so
  udev monitors (SDL2) start instead of retrying every frame.

Portal launches PRoot with `-H` and `PROOT_L2S_DIR=<rootfs>/.proot.l2s`.

## Building Portal's binaries

`assets/libs/arm64-v8a/libproot.so` and `libproot_loader.so` are built from
this directory with Android NDK r27c and GNU awk (the loader step needs
`strtonum`):

    export ANDROID_NDK_HOME=/path/to/android-ndk-r27c
    ./make-talloc-static.sh
    ./make-proot.sh

Then copy `build/root-aarch64/root/bin/proot-userland` to `libproot.so` and
`build/root-aarch64/root/libexec/proot/loader` to `libproot_loader.so`.
The checkout path only sets PRoot's fallback loader path, which Portal never
uses (it always passes `PROOT_LOADER`); the shipped binaries were built from
`/opt/pbuild/build-proot-android`.

Always rebuild and commit the two together. On arm64 `libproot.so` embeds the
offset of the loader's `pokedata_workaround` stub, so a PRoot paired with a
loader from another build writes guest memory at the wrong address on kernels
whose `PTRACE_POKEDATA` is broken. Both must keep 16 KB aligned LOAD segments
(`scripts/check_elf_page_size.py assets`), and `scripts/proot-regression/run.sh`
runs Portal's regression cases against a host build of the same sources.

See https://github.com/green-green-avk/proot for more info.
