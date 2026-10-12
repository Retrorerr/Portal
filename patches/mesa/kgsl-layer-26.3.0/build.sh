#!/bin/bash
# Build Portal's Mesa KGSL layer (src/android/proot/mesa_layer.rs).
#
# Runs as root inside Debian 13 (trixie) on ARM64, natively; CI uses
# .github/workflows/mesa-kgsl-layer-arm64.yml. It reproduces lfdevs'
# mesa-for-android-container Debian trixie release (same tag, same meson
# options, same `meson install` tarball layout) with this directory's
# numbered Portal patches applied on top, and writes to <out-dir>:
#
#   mesa-for-android-container_<version>-<date>_debian_trixie_arm64.tar.gz
#   SHA256SUMS
#
#   build.sh <portal-checkout> <out-dir> [portal|baseline]
#
# "baseline" skips the Portal patches, reproducing lfdevs' release.
set -euo pipefail

PORTAL=$(realpath "$1")
OUT=$(realpath -m "$2")
VARIANT=${3:-portal}

LFDEVS_MESA_REPO=https://github.com/lfdevs/mesa-for-android-container.git
LFDEVS_MESA_TAG=mesa-26.3.0-devel-20260824
LFDEVS_MESA_SHA=98f3d6229d61452cef80f8563af7c56ae599dc14
# The layer version Portal pins (mesa_layer.rs LAYER_VERSION): lfdevs'
# version and build date, so the layer keeps its place in update ordering.
LAYER_SUFFIX=26.3.0-devel-20260824
PATCH_DIR="$PORTAL/patches/mesa/kgsl-layer-26.3.0"

case "$VARIANT" in
    portal | baseline) ;;
    *) echo "unknown variant: $VARIANT" >&2; exit 2 ;;
esac

export DEBIAN_FRONTEND=noninteractive
sed -i 's/^Types: deb$/Types: deb deb-src/' /etc/apt/sources.list.d/debian.sources
apt-get update
apt-get build-dep -y mesa
apt-get install -y --no-install-recommends ca-certificates git libxfixes-dev libarchive-dev

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

src="$work/mesa"
git clone --quiet --depth 1 --branch "$LFDEVS_MESA_TAG" "$LFDEVS_MESA_REPO" "$src"
test "$(git -C "$src" rev-parse HEAD)" = "$LFDEVS_MESA_SHA"
if [ "$VARIANT" = portal ]; then
    for patch in "$PATCH_DIR"/[0-9][0-9][0-9][0-9]-*.patch; do
        git -C "$src" -c user.name=portal -c user.email=portal@localhost am --quiet "$patch"
    done
fi

# lfdevs' Debian trixie options, verbatim.
meson setup "$src/build" "$src" \
    --prefix=/usr \
    -Dplatforms=x11,wayland \
    -Dgallium-drivers=freedreno,zink,virgl,llvmpipe \
    -Dgallium-va=disabled \
    -Dgallium-mediafoundation=disabled \
    -Dvulkan-drivers=freedreno \
    -Dvulkan-layers= \
    -Degl=enabled \
    -Dgles2=enabled \
    -Dglvnd=enabled \
    -Dglx=dri \
    -Dlibunwind=disabled \
    -Dintel-rt=disabled \
    -Dmicrosoft-clc=disabled \
    -Dvalgrind=disabled \
    -Dgles1=disabled \
    -Dfreedreno-kmds=kgsl \
    -Dbuildtype=release
ninja -C "$src/build"

install="$work/install"
DESTDIR="$install" meson install -C "$src/build" --quiet

mkdir -p "$OUT"
name="mesa-for-android-container_${LAYER_SUFFIX}_debian_trixie_arm64.tar.gz"
tar -zcf "$OUT/$name" -C "$install" .
(cd "$OUT" && sha256sum "$name" > SHA256SUMS && cat SHA256SUMS)
