#!/bin/bash
# Build Portal's Anland libkwin (assets/kwin-anland-arm64/libkwin.so.6.3.6).
#
# Runs as root inside Debian 13 (trixie) on ARM64, natively; CI uses
# .github/workflows/kwin-anland-arm64.yml. Every input is pinned and checked:
#
#   Debian's KWin 6.3.6 orig tarball
#   + lfdevs/kwin anland-5.13-debian-4_6.3.6-95 (the debian/ packaging and its
#     quilt series, which add the Anland backend; the same source as the
#     kwin 4:6.3.6-95 packages installed in Portal's guest)
#   + this directory's numbered Portal patches, appended to that series
#
# It configures with lfdevs' own debian/rules and Debian's build flags, builds
# only the libkwin target, and writes to <out-dir>:
#
#   libkwin.so.6.3.6                       stripped, as shipped in the APK
#   kwin-anland-6.3.6-<variant>-source.tar.xz  the exact patched source tree
#   SHA256SUMS
#
#   build.sh <portal-checkout> <out-dir> [portal|baseline]
#
# "baseline" skips the Portal patches, reproducing lfdevs' libkwin for
# comparison.
set -euo pipefail

PORTAL=$(realpath "$1")
OUT=$(realpath -m "$2")
VARIANT=${3:-portal}

ORIG_URL=https://deb.debian.org/debian/pool/main/k/kwin/kwin_6.3.6.orig.tar.xz
ORIG_SHA256=27f2205f06d58f1d1f480d2a94ae24022c2f95b9c1fdc5a549f8e143713fce12
LFDEVS_KWIN_REPO=https://github.com/lfdevs/kwin.git
LFDEVS_KWIN_TAG=anland-5.13-debian-4_6.3.6-95
LFDEVS_KWIN_SHA=beea4c3d22f08100b1b3acda1bd502e87bb1a347
PATCH_DIR="$PORTAL/patches/kwin/anland-6.3.6"

case "$VARIANT" in
    portal | baseline) ;;
    *) echo "unknown variant: $VARIANT" >&2; exit 2 ;;
esac

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl devscripts equivs git quilt xz-utils

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

curl -fsSL --retry 3 -o "$work/kwin_6.3.6.orig.tar.xz" "$ORIG_URL"
echo "$ORIG_SHA256  $work/kwin_6.3.6.orig.tar.xz" | sha256sum -c -

git clone --quiet --depth 1 --branch "$LFDEVS_KWIN_TAG" "$LFDEVS_KWIN_REPO" "$work/lfdevs-kwin"
test "$(git -C "$work/lfdevs-kwin" rev-parse HEAD)" = "$LFDEVS_KWIN_SHA"

tree="$work/kwin-6.3.6"
tar -xJf "$work/kwin_6.3.6.orig.tar.xz" -C "$work"
cp -a "$work/lfdevs-kwin/debian" "$tree/"
if [ "$VARIANT" = portal ]; then
    for patch in "$PATCH_DIR"/[0-9][0-9][0-9][0-9]-*.patch; do
        cp "$patch" "$tree/debian/patches/"
        basename "$patch" >> "$tree/debian/patches/series"
    done
fi
cat "$tree/debian/patches/series"

cd "$tree"
QUILT_PATCHES=debian/patches quilt --quiltrc=/dev/null push -a
tar -C "$work" --exclude=kwin-6.3.6/.pc -cJf "$work/source.tar.xz" kwin-6.3.6

mk-build-deps --install --remove \
    --tool 'apt-get -y --no-install-recommends' debian/control

# dh would export these; lfdevs' debian/rules sets hardening=+all.
export DEB_BUILD_MAINT_OPTIONS=hardening=+all
eval "$(dpkg-buildflags --export=sh)"
debian/rules override_dh_auto_configure
builddir=$(find . -maxdepth 1 -type d -name 'obj-*' | head -n 1)
cmake --build "$builddir" --target kwin --parallel "$(nproc)"

library=$(find "$builddir" -name libkwin.so.6.3.6 -type f | head -n 1)
mkdir -p "$OUT"
strip --strip-unneeded -o "$OUT/libkwin.so.6.3.6" "$library"
mv "$work/source.tar.xz" "$OUT/kwin-anland-6.3.6-$VARIANT-source.tar.xz"
(cd "$OUT" && sha256sum libkwin.so.6.3.6 "kwin-anland-6.3.6-$VARIANT-source.tar.xz" > SHA256SUMS)
cat "$OUT/SHA256SUMS"
