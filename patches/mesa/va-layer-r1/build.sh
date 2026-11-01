#!/usr/bin/env bash
# build.sh: build Portal's VA layer, the termuxva VA-API driver.
#
# Run in an arm64 Debian trixie environment (the Portal guest itself works:
# `apt install build-essential meson ninja-build pkg-config python3-mako
# python3-yaml python3-packaging bison flex glslang-tools libdrm-dev libva-dev
# libexpat1-dev zlib1g-dev libzstd-dev libwayland-dev wayland-protocols
# libx11-xcb-dev libxcb-dri3-dev libxcb-present-dev libxcb-sync-dev
# libxshmfence-dev libxcb-xfixes0-dev libxcb-randr0-dev git`).
#
# CI: .github/workflows/va-layer-arm64.yml.
#
# Output: $OUT/termuxva_drv_video.so (+ .gz, the release asset, and
# SHA256SUMS), one self-contained library (freedreno
# KGSL screen + VA frontend + termux-va bridge). It shares no soname with the
# KGSL Mesa layer, so both load in one process. Install it as
# files/va-layer/termuxva_drv_video.so (see src/android/backend/video_decode.rs).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="${SRC:-$PWD/mesa-va-r1}"
OUT="${OUT:-$PWD/va-layer}"
TAG=mesa-27.0.0-devel-20261025-r1   # lfdevs/mesa-for-android-container, dev/adreno-main
SHA=eb3e7fd4f009cab04b1482aad4843c0bde28a786

if [[ ! -f "$SRC/meson.build" ]]; then
  git clone --quiet --depth 1 --branch "$TAG" https://github.com/lfdevs/mesa-for-android-container.git "$SRC"
  test "$(git -C "$SRC" rev-parse HEAD)" = "$SHA"
  git -C "$SRC" apply "$HERE/portal-va-r1.patch"
fi

meson setup "$SRC/build" "$SRC" --wipe -Dbuildtype=release -Db_ndebug=true \
  -Dgallium-drivers=freedreno -Dvulkan-drivers= -Dfreedreno-kmds=kgsl \
  -Dgallium-va=enabled -Dtermux-va-bridge=enabled -Dvideo-codecs=all \
  -Dllvm=disabled -Dplatforms=x11,wayland -Dglx=disabled -Degl=disabled \
  -Dgbm=disabled -Dopengl=false -Dgles1=disabled -Dgles2=disabled \
  -Dvalgrind=disabled -Dlibunwind=disabled -Dlmsensors=disabled -Dzstd=enabled
ninja -C "$SRC/build" src/gallium/targets/va/libgallium_drv_video.so
mkdir -p "$OUT"
install -m 644 "$SRC/build/src/gallium/targets/va/libgallium_drv_video.so" \
  "$OUT/termuxva_drv_video.so"
strip --strip-unneeded "$OUT/termuxva_drv_video.so"
# The release asset video_decode.rs pins (LAYER_URL/SHA256/size).
gzip -9 -n -c "$OUT/termuxva_drv_video.so" > "$OUT/termuxva_drv_video.so.gz"
(cd "$OUT" && sha256sum termuxva_drv_video.so termuxva_drv_video.so.gz | tee SHA256SUMS \
  && stat -c '%s %n' termuxva_drv_video.so.gz)
