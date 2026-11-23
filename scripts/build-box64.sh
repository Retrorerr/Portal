#!/usr/bin/env bash
set -euo pipefail

# Build the Debian ARM64 Box64 version used by Portal with the D-Bus ABI fix.
# Requires CMake, Ninja, Python 3, and aarch64-linux-gnu-gcc/binutils.
repo=$(cd "$(dirname "$0")/.." && pwd)
source_name=box64_0.3.4+dfsg.orig.tar.xz
archive=${1:-"$repo/target/$source_name"}
output=${2:-"$repo/assets/guest-arm64/portal-box64"}
if [ ! -f "$archive" ]; then
    mkdir -p "$(dirname "$archive")"
    curl --fail --location --output "$archive" \
        "https://deb.debian.org/debian/pool/main/b/box64/$source_name"
fi
printf '%s  %s\n' 17ab815fba7f0a8df7fcd892f9ffd7c64282c32d24ebf974a0c4d931965b4e14 "$archive" | sha256sum --check --status

scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
tar -xf "$archive" -C "$scratch"
source_dir="$scratch/box64-0.3.4"
patch --directory="$source_dir" -p1 < "$repo/patches/box64/dbus-x86-va-list.patch"
cmake -S "$source_dir" -B "$scratch/build" -G Ninja \
    -DCMAKE_SYSTEM_NAME=Linux -DCMAKE_SYSTEM_PROCESSOR=aarch64 \
    -DCMAKE_C_COMPILER=aarch64-linux-gnu-gcc -DCMAKE_ASM_COMPILER=aarch64-linux-gnu-gcc \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_EXE_LINKER_FLAGS=-Wl,-z,max-page-size=16384 \
    -DARM64=ON -DARM_DYNAREC=ON -DNOGIT=ON
cmake --build "$scratch/build" --target box64 --parallel "${BOX64_BUILD_JOBS:-$(nproc)}"
aarch64-linux-gnu-strip --strip-unneeded "$scratch/build/box64"
install -m 755 "$scratch/build/box64" "$output"
