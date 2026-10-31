#!/usr/bin/env bash
# build-portal-va-daemon.sh — cross-compile the Android-side MediaCodec video
# decode daemon (third_party/termux-va) that serves the guest's VA-API bridge.
#
# Usage:
#   ANDROID_NDK_HOME=~/Android/Sdk/ndk/27.2.12479018 scripts/build-portal-va-daemon.sh
#
# Writes assets/libs/arm64-v8a/libportal_va_daemon.so and
# libportal_va_probe.so. Like the PipeWire helpers they are executables named
# `lib*.so` so Android extracts them from the APK into nativeLibraryDir, the
# only place an app may exec from.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$ROOT/third_party/termux-va"
# AMediaFormat_getRect needs API 28; termux-va pins 29.
API="${API:-29}"
OUT_DIR="${OUT_DIR:-$ROOT/assets/libs/arm64-v8a}"

ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-$HOME/Android/Sdk/ndk/27.2.12479018}}"
case "$(uname -s)" in
  Linux) HOST_TAG=linux-x86_64 ;;
  Darwin) HOST_TAG=darwin-x86_64 ;;
  *) echo "unsupported host" >&2; exit 2 ;;
esac
BIN="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$HOST_TAG/bin"
CC="$BIN/aarch64-linux-android$API-clang"
[[ -x "$CC" ]] || { echo "NDK clang not found: $CC" >&2; exit 2; }

CFLAGS=(-O2 -Wall -Wextra -std=c11 -fPIE -pie -I "$SRC/common"
        -ffile-prefix-map="$ROOT"=. -Wl,--build-id=none)
mkdir -p "$OUT_DIR"
"$CC" "${CFLAGS[@]}" -o "$OUT_DIR/libportal_va_daemon.so" "$SRC/daemon/termux-va.c" \
  -lmediandk -llog -landroid
"$CC" "${CFLAGS[@]}" -o "$OUT_DIR/libportal_va_probe.so" "$SRC/tools/tva-probe.c"
"$BIN/llvm-strip" --strip-unneeded "$OUT_DIR/libportal_va_daemon.so" "$OUT_DIR/libportal_va_probe.so"
sha256sum "$OUT_DIR/libportal_va_daemon.so" "$OUT_DIR/libportal_va_probe.so"
