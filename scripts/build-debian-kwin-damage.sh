#!/bin/sh
# Run in a Debian 13 ARM64 build guest with the KWin build dependencies.
# Usage: script PRISTINE_KWIN_6_3_6_SOURCE PORTAL_PATCH_DIR TEST_SOURCE BUILD_DIR
set -eu
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
echo "Stage $build_dir/bin/libkwin.so.6.3.6 in assets/kwin-debian-arm64 before rebuilding the APK."
