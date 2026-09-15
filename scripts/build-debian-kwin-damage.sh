#!/bin/sh
# RETIRED LEGACY SCRIPT — do not use for the active Forky graphics stack.
# The old Debian 6.3.6/QPainter experiment is kept only for historical source
# comparison. Active KWin builds must use patches/kwin/README.md and the exact
# tuple in assets/graphics-stack-lock.json.
set -eu
echo "build-debian-kwin-damage.sh is retired; use the Forky KWin pipeline." >&2
exit 2

# Historical implementation below is intentionally unreachable.
# Usage: script PRISTINE_KWIN_6_3_6_SOURCE PORTAL_PATCH_DIR TEST_SOURCE BUILD_DIR
source_dir=$1
patch_dir=$2
test_source=$3
build_dir=$4
for fix in "$patch_dir"/0001-*.patch "$patch_dir"/0002-*.patch; do
    if patch -d "$source_dir" -p1 --dry-run --forward < "$fix" >/dev/null 2>&1; then
        patch -d "$source_dir" -p1 --forward < "$fix"
    elif ! patch -d "$source_dir" -p1 --dry-run --reverse < "$fix" >/dev/null 2>&1; then
        echo "Patch neither applicable nor already applied: $fix" >&2
        exit 1
    fi
done
mkdir -p "$build_dir"
c++ -std=c++23 -O2 -fPIC "$test_source" \
    -I"$source_dir/src/backends/wayland" \
    $(pkg-config --cflags --libs Qt6Gui) -o "$build_dir/qpainter-damage-test"
"$build_dir/qpainter-damage-test"
cmake -S "$source_dir" -B "$build_dir" -G Ninja \
    -DCMAKE_BUILD_TYPE=RelWithDebInfo -DCMAKE_INSTALL_PREFIX=/usr \
    -DCMAKE_AUTOGEN_PARALLEL="${PORTAL_BUILD_JOBS:-4}" \
    -DCMAKE_INSTALL_LIBDIR=lib/aarch64-linux-gnu -DBUILD_TESTING=OFF
cmake --build "$build_dir" --target kwin --parallel "${PORTAL_BUILD_JOBS:-4}"
echo "If retained for audit, stage $build_dir/bin/libkwin.so.6.3.6 only under assets/legacy-graphics/; it is not an APK input."
