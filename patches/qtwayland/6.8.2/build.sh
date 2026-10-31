#!/bin/bash
# Run natively in Debian 13 ARM64 with qt6-base-private-dev,
# qt6-wayland-dev, qt6-wayland-private-dev, libwayland-dev,
# wayland-protocols, libegl-dev, libxkbcommon-dev, cmake, ninja-build, patch, g++.
set -euo pipefail
patches=$(cd "$(dirname "$0")" && pwd)
work=${1:?usage: build.sh WORK OUTPUT}
output=${2:?usage: build.sh WORK OUTPUT}
version=6.8.2
archive_sha=5e46157908295f2bf924462d8c0855b0508ba338ced9e810891fefa295dc9647
[ "$(dpkg-query -W -f='${Version}' libqt6waylandclient6)" = 6.8.2-4 ] || {
    echo 'The overlay requires Debian libqt6waylandclient6 6.8.2-4' >&2; exit 1;
}
mkdir -p "$work" "$output"
work=$(realpath "$work")
output=$(realpath "$output")
archive="$work/qtwayland-$version.tar.xz"
if [ ! -f "$archive" ]; then
    curl -fL --retry 3 -o "$archive.part" "https://download.qt.io/archive/qt/6.8/$version/submodules/qtwayland-everywhere-src-$version.tar.xz"
    mv "$archive.part" "$archive"
fi
printf '%s  %s\n' "$archive_sha" "$archive" | sha256sum -c -
source="$work/qtwayland-everywhere-src-$version"
# Always start from the verified archive; never stack a patch twice.
[ ! -e "$source" ] || { echo "Source already exists: $source; use a fresh work directory" >&2; exit 1; }
tar -xf "$archive" -C "$work"
for patch in "$patches"/[0-9][0-9][0-9][0-9]-*.patch; do
    patch -d "$source" -p1 < "$patch"
done
cmake -S "$source" -B "$work/build" -G Ninja \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr \
    '-DCMAKE_SHARED_LINKER_FLAGS=-Wl,-z,max-page-size=16384' \
    -DQT_BUILD_EXAMPLES=OFF -DQT_BUILD_TESTS=OFF \
    -DFEATURE_wayland_server=OFF
cmake --build "$work/build" --target WaylandClient --parallel 4
library=$(find "$work/build" -name 'libQt6WaylandClient.so.6.8.2' -type f -print -quit)
[ -n "$library" ] || { echo 'WaylandClient library missing' >&2; exit 1; }
cp "$library" "$output/libQt6WaylandClient.so.6.8.2"
strip "$output/libQt6WaylandClient.so.6.8.2"
ln -sfn libQt6WaylandClient.so.6.8.2 "$output/libQt6WaylandClient.so.6"
ln -sfn libQt6WaylandClient.so.6.8.2 "$output/libQt6WaylandClient.so"
sha256sum /usr/lib/aarch64-linux-gnu/libQt6WaylandClient.so.6.8.2 > "$output/original.sha256"
sha256sum "$output/libQt6WaylandClient.so.6.8.2"
