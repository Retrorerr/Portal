# KWin 6.7.4 Android/PRoot source patches

This directory contains two independent source patches for the pinned KWin
6.7.4 build used by Local Desktop. Apply both to the same pristine source,
in numeric order. The first patch keeps the already-validated Android/PRoot
udev guard. The second hardens the QPA raster backing-store failure path
observed during the edge-maximize preview crash; it does not change output
scale policy, compositing backend, or maximize behavior.

This is a source patch for the confirmed Android/PRoot KWin crash.  The
captured stack is:

```text
KWin::UdevMonitor::fd() + 0
KWin::GpuManager::GpuManager() + 0x104
KWin::Application::createGpuManager()
kwin_wayland main
```

`GpuManager` is constructed before KWin selects its nested Wayland backend.
In KWin v6.7.4, `Udev::createMonitor()` returns `nullptr` when libudev cannot
create its netlink monitor, while the constructor unconditionally calls
`m_udevMonitor->fd()`.  The patch keeps `scanForRenderDevices()` and only
disables hotplug setup when the monitor is unavailable.  The extra guard in
`handleUdevEvent()` protects a future/manual call as well.

## Pinned source and patch

The patch targets KDE KWin tag `v6.7.4`, commit
`8438567a741826da8b7536a8b10eb3af8fc8820d`.  Apply it only to a pristine
checkout or extraction of that source:

```sh
kwin_source=/var/lib/localdesktop/build-kwin/kwin-9d1e43932d6799254350403279dc551298911b71
kwin_patch_dir=/path/to/Portal/patches/kwin

cd "$kwin_source"
for kwin_patch in "$kwin_patch_dir"/0001-*.patch "$kwin_patch_dir"/0002-*.patch; do
  patch -p1 --dry-run < "$kwin_patch"
  patch -p1 < "$kwin_patch"
done
rg -n "m_udevNotifier\(|udev monitor unavailable|scanForRenderDevices" \
  src/core/gpumanager.cpp
rg -n "m_usingFallback|SHM allocator (memfd_create|ftruncate|mmap) failed" \
  src/plugins/qpa/backingstore.* src/core/shmgraphicsbufferallocator.cpp
```

For a Git checkout, `git apply --check "$kwin_patch"` is equivalent to the
dry run.  Do not apply the patch twice.  The source archive used for the
OnePlus investigation was KWin v6.7.4 and had SHA-256
`00a199f8c78407a0630ec2c0873be90bbe4f9e9f31bd0f97c7057cdf224cb180`.

## Native ARM64 build

Run these commands inside the guest, not on the Windows host.  The installed
`kwin_wayland` must be from the same 6.7.4 package as the patched library.

```sh
set -eu
[ "$(uname -m)" = aarch64 ]
pacman -Q kwin

kwin_build=/var/lib/localdesktop/build-kwin/build-6.7.4
cmake -S "$kwin_source" -B "$kwin_build" -G Ninja \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DCMAKE_INSTALL_PREFIX=/usr \
  -DCMAKE_INSTALL_LIBDIR=lib \
  -DBUILD_TESTING=OFF
cmake --build "$kwin_build" --target kwin kwin_wayland --parallel 2
```

Verify that both results are guest ARM64 artifacts before staging them:

```sh
kwin_library=$(find "$kwin_build" -type f -name 'libkwin.so.*' -print | head -n 1)
[ -n "$kwin_library" ]
readelf -h "$kwin_library" | rg 'Class:|Machine:'
kwin_executable="$kwin_build/bin/kwin_wayland"
[ -x "$kwin_executable" ]
readelf -h "$kwin_executable" | rg 'Class:|Machine:'
```

Stage the library in an isolated directory first.  Do not overwrite the
package-managed copy until the null-monitor test has passed and the old copy
has been backed up by the release owner:

```sh
kwin_stage=/var/lib/localdesktop/build-kwin/stage-6.7.4
install -Dm755 "$kwin_library" "$kwin_stage/usr/lib/$(basename "$kwin_library")"
ln -s "$(basename "$kwin_library")" "$kwin_stage/usr/lib/libkwin.so.6"
ln -s "libkwin.so.6" "$kwin_stage/usr/lib/libkwin.so"
```

The QPA sources are object-linked into `kwin_wayland`; rebuilding only
`kwin`/`libkwin.so` cannot contain patch 0002. Stage the executable separately
for the release owner, without replacing the package-managed binary until the
isolated test and backup checks pass:

```sh
install -Dm755 "$kwin_executable" "$kwin_stage/usr/bin/kwin_wayland"
readelf -h "$kwin_stage/usr/bin/kwin_wayland" | rg 'Class:|Machine:'
```

## QPA failure-path regression

This is a failure-path hardening test, not a substitute for the Pad 3 visual
test. It verifies that the source/build really contains the fallback and
allocator checks, then confirms that a virtual KWin run does not terminate
with a fatal signal:

```sh
set -eu
rg -n "m_usingFallback|m_fallbackImage" \
  "$kwin_source/src/plugins/qpa/backingstore.cpp" \
  "$kwin_source/src/plugins/qpa/backingstore.h"
rg -n "SHM allocator (memfd_create|ftruncate|mmap) failed|rejected (empty|oversized)" \
  "$kwin_source/src/core/shmgraphicsbufferallocator.cpp"

runtime_dir="$kwin_stage/qpa-failure-runtime"
runtime_log="$kwin_stage/qpa-failure-runtime.log"
socket_name="localdesktop-kwin-qpa-failure-$$"
mkdir -p "$runtime_dir"
chmod 700 "$runtime_dir"
set +e
XDG_RUNTIME_DIR="$runtime_dir" \
LD_LIBRARY_PATH="$kwin_stage/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
KWIN_COMPOSE=Q \
QT_FORCE_STDERR_LOGGING=1 \
QT_LOGGING_RULES="kwin_core.warning=true;kwin_qpa.warning=true" \
timeout --foreground --kill-after=2s 8s \
  "$kwin_stage/usr/bin/kwin_wayland" --virtual --no-lockscreen \
  --no-global-shortcuts --socket "$socket_name" >"$runtime_log" 2>&1
status=$?
set -e
[ "$status" -ne 139 ]
[ "$status" -ne 134 ]
! rg -i -q 'SIGSEGV|SIGABRT|segmentation fault' "$runtime_log"
```

On the physical Pad 3, the release owner must then run the normal launch and
semantic maximize flow with the staged executable while keeping the existing
patched `libkwin.so.6.7.4` in place. The relevant attempt-correlated evidence
is:

```sh
rg -n "Failed to allocate backing-store buffer|SHM allocator (memfd_create|ftruncate|mmap) failed|Failed to (create a swapchain|acquire a graphics buffer|map a graphics buffer)|signal=11|crash-summary" \
  /var/lib/localdesktop/kwin.log \
  /var/lib/localdesktop/kwin-backtrace.log
```

Success requires no new SIGSEGV during Firefox edge maximize, normal Plasma
and outline rendering on the Pad 3, and no regression of the existing udev
guard. A fallback paint device only proves that Qt survived an allocation
failure and skipped presenting an invalid buffer; it is not visual success.

## Injected null-monitor regression

`tests/kwin_udev_monitor_null.c` is deliberately test-only.  It interposes
only `udev_monitor_new_from_netlink()` and leaves `udev_new()` plus render-node
enumeration untouched.  Build it with the guest compiler and run the virtual
backend, which still constructs `GpuManager` but does not require a physical
DRM output:

```sh
cc -shared -fPIC -O2 \
  -o /var/lib/localdesktop/build-kwin/libkwin-null-udev.so \
  /path/to/Portal/tests/kwin_udev_monitor_null.c

runtime_dir=/var/lib/localdesktop/build-kwin/runtime-udev-test
mkdir -p "$runtime_dir"
chmod 700 "$runtime_dir"
runtime_log=/var/lib/localdesktop/build-kwin/kwin-udev-null-runtime.log
set +e
XDG_RUNTIME_DIR="$runtime_dir" \
LD_PRELOAD=/var/lib/localdesktop/build-kwin/libkwin-null-udev.so \
LD_LIBRARY_PATH="$kwin_stage/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
KWIN_COMPOSE=Q \
timeout --foreground --kill-after=2s 8s \
  /usr/bin/kwin_wayland --virtual --no-lockscreen --no-global-shortcuts \
  --socket localdesktop-kwin-udev-null \
  >"$runtime_log" 2>&1
runtime_status=$?
set -e

rg -n "udev monitor unavailable; continuing without DRM hotplug events" \
  "$runtime_log"
[ "$runtime_status" -ne 139 ]
[ "$runtime_status" -ne 134 ]
```

Expected outcomes are a warning and either a clean stop or `timeout` status
124 after surviving the constructor.  A SIGSEGV (normally status 139) or
SIGABRT (normally status 134) is a failed regression.  This test proves only
that KWin gets past the previously crashing constructor; it does not prove
nested Wayland connection, configure/ack, buffer import, Android EGL
submission, or physical presentation.  Those remain separate ARM64 release
gates and must retain attempt-correlated logs and a native backtrace for any
new failure.
