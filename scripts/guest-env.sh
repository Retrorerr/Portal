#!/bin/sh
# guest-env.sh — canonical Debug guest-client environment (sourced, not run).
#
# Ownership contract (see scripts/guest-exec.sh):
#   * THIS FILE owns the fixed Portal client policy every UI-launched app
#     inherits from the session: KGSL/Freedreno Mesa selection, Wayland
#     display/runtime defaults. Values are `${VAR:-default}` so a caller can
#     still override for a targeted experiment.
#   * The CALLER owns DBUS_SESSION_BUS_ADDRESS (session-specific) and the
#     command itself.
# Explicitly NOT set here (must stay that way):
#   * KWin-only Anland variables (ANLAND*, ANLAND_NO_DRM_DEVICE,
#     ANLAND_SKIP_IMPLICIT_SYNC_WAIT, XWAYLAND_FORCE_KGSL_SURFACELESS,
#     KWIN_GL_DEBUG) — compositor wrapper only.
#   * EGL_PLATFORM — forces QtQuick software fallback.
export MESA_LOADER_DRIVER_OVERRIDE="${MESA_LOADER_DRIVER_OVERRIDE:-kgsl}"
export GALLIUM_DRIVER="${GALLIUM_DRIVER:-freedreno}"
export FD_FORCE_KGSL="${FD_FORCE_KGSL:-1}"
export FD_KGSL_ENABLE_DMABUF="${FD_KGSL_ENABLE_DMABUF:-1}"
export TURNIP_KMD="${TURNIP_KMD:-kgsl}"
export WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-wayland-1}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp}"
export XDG_CURRENT_DESKTOP="${XDG_CURRENT_DESKTOP:-KDE}"
export DESKTOP_SESSION="${DESKTOP_SESSION:-plasma}"
export QT_QPA_PLATFORM="${QT_QPA_PLATFORM:-wayland}"
export QT_WAYLAND_SHELL_INTEGRATION="${QT_WAYLAND_SHELL_INTEGRATION:-xdg-shell}"
export HOME="${HOME:-/root}"
export TMPDIR="${TMPDIR:-/tmp}"
export USER="${USER:-root}"
export LOGNAME="${LOGNAME:-root}"
export LANG="${LANG:-C.UTF-8}"
export LC_ALL="${LC_ALL:-C.UTF-8}"
export SHELL="${SHELL:-/bin/bash}"
export PATH="/usr/local/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export LD_LIBRARY_PATH="/usr/local/lib:/usr/lib/aarch64-linux-gnu:/lib/aarch64-linux-gnu"
