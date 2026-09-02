#!/bin/bash
# Run the KWin 6.7.4 missing-udev-monitor regression on native ARM64.
#
# This test deliberately injects a failed udev monitor into the package's
# kwin_wayland process.  It is diagnostic-only; it never installs a library
# and never changes the package-managed KWin files.
set -Eeuo pipefail

fail() {
    echo "kwin udev guard regression: $*" >&2
    exit 1
}

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd -- "$script_dir/.." && pwd)
build_root=${LOCALDESKTOP_KWIN_BUILD_ROOT:-/var/lib/localdesktop/build-kwin}
kwin_stage=${LOCALDESKTOP_KWIN_STAGE:-$build_root/stage-6.7.4}
kwin_bin=${LOCALDESKTOP_KWIN_BIN:-/usr/bin/kwin_wayland}
compiler=${CC:-cc}
shim=$build_root/libkwin-null-udev.so
runtime_dir=$build_root/runtime-udev-test
runtime_log=$build_root/kwin-udev-null-runtime-$$.log
socket_name=localdesktop-kwin-udev-null-$$

[ "$(uname -m)" = aarch64 ] || fail "must run on native aarch64 (not an emulator or translated host)"
[ -r "$repo_root/tests/kwin_udev_monitor_null.c" ] || fail "interposer source is missing"
[ -x "$kwin_bin" ] || fail "KWin binary is not executable: $kwin_bin"
[ -r "$kwin_stage/usr/lib/libkwin.so.6" ] || fail "staged libkwin.so.6 is missing"
command -v "${compiler%% *}" >/dev/null || fail "guest compiler is missing: $compiler"
command -v readelf >/dev/null || fail "readelf is required for the ARM64 check"
command -v timeout >/dev/null || fail "timeout is required for bounded execution"
command -v rg >/dev/null || fail "rg is required for log assertions"

mkdir -p "$build_root" "$runtime_dir"
chmod 700 "$runtime_dir"
"$compiler" -shared -fPIC -O2 -o "$shim" "$repo_root/tests/kwin_udev_monitor_null.c"
readelf -h "$shim" | rg -q 'AArch64' || fail "interposer is not an AArch64 shared object"

set +e
XDG_RUNTIME_DIR="$runtime_dir" \
LD_PRELOAD="$shim" \
LD_LIBRARY_PATH="$kwin_stage/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
KWIN_COMPOSE=Q \
timeout --foreground --kill-after=2s 8s \
  "$kwin_bin" --virtual --no-lockscreen --no-global-shortcuts \
  --socket "$socket_name" >"$runtime_log" 2>&1
status=$?
set -e

if [ "$status" -eq 139 ] || [ "$status" -eq 134 ]; then
    fail "KWin still terminated by signal (status $status); see $runtime_log"
fi
if [ "$status" -eq 125 ] || [ "$status" -eq 126 ] || [ "$status" -eq 127 ]; then
    fail "bounded KWin invocation failed before execution (status $status); see $runtime_log"
fi
if rg -i -q 'SIGSEGV|SIGABRT|segmentation fault' "$runtime_log"; then
    fail "KWin log contains a fatal signal; see $runtime_log"
fi
rg -Fq 'udev monitor unavailable; continuing without DRM hotplug events' "$runtime_log" \
    || fail "patched constructor warning was not observed; see $runtime_log"

echo "PASS: KWin survived the injected null udev monitor (status=$status)"
echo "log=$runtime_log"
