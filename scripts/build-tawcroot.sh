#!/bin/bash
# Build the vendored tawcroot guest engine (third_party/tawcroot) and stage
# libtawcroot.so into assets/libs/arm64-v8a/, which xbuild packages into the
# APK through `runtime_libs` like libproot.so.
#
# Usage: scripts/build-tawcroot.sh
# Env:   ANDROID_NDK_HOME (defaults to NDK 27.2.12479018 under ~/Android/Sdk)
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-${ANDROID_HOME:-$HOME/Android/Sdk}/ndk/27.2.12479018}"
export TAWCROOT_STAGE_DIR="$root/assets/libs"

bash "$root/third_party/tawcroot/build.sh" --abi=aarch64
python3 "$root/scripts/check_elf_page_size.py" "$root/assets/libs/arm64-v8a/libtawcroot.so"
sha256sum "$root/assets/libs/arm64-v8a/libtawcroot.so"
