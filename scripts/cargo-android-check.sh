#!/usr/bin/env bash
# Fast Android compile check against NDK 27.2 (Linux).
#
# Usage: scripts/cargo-android-check.sh [cargo subcommand and args]
# Default: cargo check --target aarch64-linux-android --lib --features portal-debug
# Example: scripts/cargo-android-check.sh clippy --target aarch64-linux-android --lib --locked
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
ndk="$ANDROID_HOME/ndk/27.2.12479018"
bin="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
[ -x "$bin/aarch64-linux-android26-clang" ] || {
    echo "error: NDK 27.2.12479018 not found at $ndk (sdkmanager --install 'ndk;27.2.12479018')" >&2
    exit 1
}

export ANDROID_NDK_HOME="$ndk" ANDROID_NDK_ROOT="$ndk"
export CC_aarch64_linux_android="$bin/aarch64-linux-android26-clang"
# CXX is required: android-activity's build script fails without it.
export CXX_aarch64_linux_android="$bin/aarch64-linux-android26-clang++"
export AR_aarch64_linux_android="$bin/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$bin/aarch64-linux-android26-clang"

cd "$repo_root"
if [ $# -eq 0 ]; then
    set -- check --target aarch64-linux-android --lib --features portal-debug
fi
exec cargo "$@"
