#!/bin/sh
# Managed by Portal: runs Windows games with Valve's Proton for ARM64.
#
# Proton 11 ARM64 is Wine built as ARM64EC with FEX inside it, so Wine,
# DXVK and VKD3D-Proton run natively on Turnip and only the game's own x86
# code is translated. Valve's stock tool wraps Proton in pressure-vessel (a
# bwrap container of Steam Linux Runtime 4); under PRoot every container
# mount and path lookup costs a ptrace stop, so this tool runs the same
# Proton build directly on Portal's Debian guest instead. The stock
# "Proton (ARM64)" stays selectable per game.
#
# Per-game launch options (Properties > General > Launch Options):
#   PORTAL_PROTON=/path/to/proton %command%   use a specific Proton build
#   PORTAL_WOW64=box64 %command%              32-bit games through Box64's
#                                             WoW64 backend instead of FEX

verb="$1"
[ "$#" -gt 0 ] && shift

STEAM_ROOT="${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.local/share/Steam}"

message() {
    if command -v zenity >/dev/null 2>&1; then
        zenity --error --title="Portal Proton" --width=460 --text="$1" 2>/dev/null
    fi
    echo "portal-proton: $1" >&2
}

# Every Steam library folder, the main one first.
library_folders() {
    echo "$STEAM_ROOT"
    vdf="$STEAM_ROOT/steamapps/libraryfolders.vdf"
    [ -r "$vdf" ] && sed -n 's/^[[:space:]]*"path"[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p' "$vdf"
}

find_proton() {
    library_folders | while IFS= read -r library; do
        for proton in "$library"/steamapps/common/Proton*ARM64*/proton \
                      "$library"/steamapps/common/Proton*arm64*/proton; do
            [ -f "$proton" ] && echo "$proton"
        done
    done | sort -uV | tail -n 1
}

proton="${PORTAL_PROTON:-$(find_proton)}"
if [ -z "$proton" ] || [ ! -f "$proton" ]; then
    message "Proton for ARM64 is not installed yet.\n\nIn Steam, open Library, show Tools, and install \"Proton 11.0 (ARM64)\" (or newer), then start the game again."
    exit 1
fi

# Steam's own runtime libraries are for the client, not for Wine.
if [ -n "$LD_LIBRARY_PATH" ]; then
    LD_LIBRARY_PATH=$(printf '%s' "$LD_LIBRARY_PATH" | tr ':' '\n' | grep -v '/steamrtarm64' | paste -sd: -)
    [ -n "$LD_LIBRARY_PATH" ] && export LD_LIBRARY_PATH || unset LD_LIBRARY_PATH
fi

# Steam prepends its overlay to LD_PRELOAD without a separator, which
# swallows Portal's drmshim.so (the GPU render-node shim). Split the glued
# entries and keep only libraries that exist.
repair_preload() {
    preload=$(printf '%s' "$LD_PRELOAD" | sed 's#\.so/#.so:/#g' | tr ' :' '\n\n' |
        while IFS= read -r lib; do [ -n "$lib" ] && [ -f "$lib" ] && echo "$lib"; done | paste -sd: -)
    [ -n "$preload" ] && export LD_PRELOAD="$preload" || unset LD_PRELOAD
}
repair_preload

# Wine's esync fallback needs many descriptors; fsync (futex_waitv) is
# available on Portal's Android kernels and is Proton's default.
ulimit -n "$(ulimit -Hn)" 2>/dev/null

# Keep compiled shaders between runs: Mesa's cache for Turnip pipelines and
# FEX's JIT cache for translated x86 code.
export MESA_SHADER_CACHE_MAX_SIZE="${MESA_SHADER_CACHE_MAX_SIZE:-4G}"
export FEX_DISKCACHE="${FEX_DISKCACHE:-1}"

if [ "$PORTAL_WOW64" = box64 ]; then
    if [ -f /opt/box64/wowbox64.dll ]; then
        export HODLL=wowbox64.dll
        export WINEDLLPATH="/opt/box64${WINEDLLPATH:+:$WINEDLLPATH}"
    else
        echo "portal-proton: Box64 is not installed; using FEX" >&2
    fi
fi

exec "$proton" "$verb" "$@"
