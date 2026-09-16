#!/bin/sh
# guest_exec.sh — healthy proot guest exec for Debug automation (app.polarbear).
# Runs a guest command with stdio fully on files (setsid + /dev/null stdin)
# so ADB disconnect can never wedge the tracer and leave tracees in
# ptrace-stop. Mirrors the session's Mesa KGSL layer binds so clients use
# the same freedreno/KGSL stack as UI-launched apps.
#
# Usage (from host shell):
#   LIBDIR=$(adb shell pm path app.polarbear | tr -d '\r' | sed 's/package://;s|/base.apk|/lib/arm64|')
#   adb shell "run-as app.polarbear sh /data/data/app.polarbear/files/tmp/guest_exec.sh $LIBDIR /bin/sh /tmp/myscript.sh"
[ -z "$1" ] && { echo "usage: guest_exec.sh <libdir> <cmd...>" >&2; exit 2; }
libdir="$1"
shift
MESA=/data/data/app.polarbear/files/mesa-kgsl-layer
L=$MESA/usr/lib/aarch64-linux-gnu
S=$MESA/usr/share
export PROOT_LOADER="$libdir/libproot_loader.so"
export PROOT_TMP_DIR=/data/data/app.polarbear/files/tmp
setsid nohup "$libdir/libproot.so" -r /data/data/app.polarbear/files/runtime-B -L --link2symlink --sysvipc --root-id \
  -b /dev -b /proc -b /sys \
  -b "$L/dri:/usr/lib/aarch64-linux-gnu/dri" \
  -b "$L/libgallium-26.3.0-devel.so:/usr/lib/aarch64-linux-gnu/libgallium-26.3.0-devel.so" \
  -b "$L/libvulkan_freedreno.so:/usr/lib/aarch64-linux-gnu/libvulkan_freedreno.so" \
  -b "$L/libEGL_mesa.so.0.0.0:/usr/lib/aarch64-linux-gnu/libEGL_mesa.so.0.0.0" \
  -b "$L/libGLX_mesa.so.0.0.0:/usr/lib/aarch64-linux-gnu/libGLX_mesa.so.0.0.0" \
  -b "$L/libgbm.so.1.0.0:/usr/lib/aarch64-linux-gnu/libgbm.so.1.0.0" \
  -b "$L/libgbm.so.1:/usr/lib/aarch64-linux-gnu/libgbm.so.1" \
  -b "$L/libEGL_mesa.so.0:/usr/lib/aarch64-linux-gnu/libEGL_mesa.so.0" \
  -b "$L/libGLX_mesa.so.0:/usr/lib/aarch64-linux-gnu/libGLX_mesa.so.0" \
  -b "$L/gbm:/usr/lib/aarch64-linux-gnu/gbm" \
  -b "$S/vulkan/icd.d:/usr/share/vulkan/icd.d" \
  -b "$S/drirc.d:/usr/share/drirc.d" \
  -w /root "$@" </dev/null >>/data/data/app.polarbear/files/tmp/guest_exec.log 2>&1 &
echo LAUNCHED
