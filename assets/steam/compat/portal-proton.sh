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
#   PORTAL_PROTON=stable %command%            newest stable Proton (ARM64)
#                                             instead of Experimental
#   PORTAL_WOW64=box64 %command%              32-bit games through Box64's
#                                             WoW64 backend instead of FEX
#   PORTAL_VRAM_MB=6144 %command%             video memory games are told
#                                             they have (default 4096)

# Settings from Portal's game panel: this game's own file, else the shared
# one (KEY=value lines). A variable already set, by the game's launch
# options for one, wins.
portal_settings=/usr/local/lib/portal/settings
for portal_conf in "$portal_settings/app-${STEAM_COMPAT_APP_ID:-${SteamAppId:-0}}.conf" "$portal_settings/game.conf"; do
    [ -r "$portal_conf" ] || continue
    while IFS='=' read -r key value; do
        case "$key" in
            PORTAL_*|FEX_*) ;;
            *) continue ;;
        esac
        case "$key" in *[!A-Za-z0-9_]*) continue ;; esac
        eval "[ -n \"\${$key+set}\" ]" || export "$key=$value"
    done < "$portal_conf"
    break
done
unset portal_settings portal_conf key value

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

# Proton Experimental (ARM64) carries the newest FEX, DXVK and VKD3D-Proton
# (FEX's JIT disk cache needs FEX-2609 or later; Proton 11.0-2 ships older),
# so it wins when installed (Steam updates it like any other tool).
# Otherwise, or with PORTAL_PROTON=stable, the newest stable one.
find_proton() {
    library_folders | while IFS= read -r library; do
        for proton in "$library"/steamapps/common/Proton*ARM64*/proton \
                      "$library"/steamapps/common/Proton*arm64*/proton; do
            [ -f "$proton" ] || continue
            case "$proton" in
                *Experimental*) [ "$1" = stable ] || echo "1 $proton" ;;
                *) echo "0 $proton" ;;
            esac
        done
    done | sort -uV | tail -n 1 | cut -d' ' -f2-
}

case "$PORTAL_PROTON" in
    ""|experimental) proton=$(find_proton) ;;
    stable) proton=$(find_proton stable) ;;
    *) proton="$PORTAL_PROTON" ;;
esac
if [ -z "$proton" ] || [ ! -f "$proton" ]; then
    case "$PORTAL_PROTON" in
        ""|experimental)
            message "Proton for ARM64 is not installed yet.\n\nIn Steam, open Library, show Tools, and install \"Proton Experimental (ARM64)\" or \"Proton 11.0 (ARM64)\", then start the game again." ;;
        stable)
            message "No stable Proton for ARM64 is installed.\n\nIn Steam, open Library, show Tools, and install \"Proton 11.0 (ARM64)\". Or choose Newest for Proton in Portal's game panel (and drop PORTAL_PROTON=stable from the game's launch options, if it is there)." ;;
        *)
            message "PORTAL_PROTON is set to a Proton that does not exist:\n$PORTAL_PROTON\n\nCorrect or remove it in the game's launch options." ;;
    esac
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
    preload=$(printf '%s\n' "$LD_PRELOAD" | sed 's#\.so/#.so:/#g' | tr ' :' '\n\n' |
        while IFS= read -r lib; do [ -n "$lib" ] && [ -f "$lib" ] && echo "$lib"; done | paste -sd: -)
    [ -n "$preload" ] && export LD_PRELOAD="$preload" || unset LD_PRELOAD
}
repair_preload

# Wine relocates DLL pages in a private file mapping and then makes them
# executable, which needs SELinux execmod; Android denies it to apps. This
# preload moves such pages to anonymous memory first (portal-execmod.c).
execmod=/usr/local/lib/portal/steam/portal-execmod.so
[ -f "$execmod" ] && export LD_PRELOAD="$execmod${LD_PRELOAD:+:$LD_PRELOAD}"

# Wine's esync fallback needs many descriptors; fsync (futex_waitv) is
# available on Portal's Android kernels and is Proton's default.
ulimit -n "$(ulimit -Hn)" 2>/dev/null

# Turnip reports most of the shared system RAM as device-local memory, so
# DXVK tells games they have ~11 GB of VRAM and they fill it with texture
# caches (Trackmania: 6 GB), which swaps the whole tablet. Report a 4 GB
# GPU instead; a game's own DXVK_CONFIG launch option still wins.
export DXVK_CONFIG="dxgi.maxDeviceMemory = ${PORTAL_VRAM_MB:-4096}; dxgi.maxSharedMemory = ${PORTAL_VRAM_MB:-4096}${DXVK_CONFIG:+; $DXVK_CONFIG}"

# Keep compiled shaders between runs: Mesa's cache for Turnip pipelines and
# FEX's JIT cache for translated x86 code.
export MESA_SHADER_CACHE_MAX_SIZE="${MESA_SHADER_CACHE_MAX_SIZE:-4G}"
export FEX_DISKCACHE="${FEX_DISKCACHE:-1}"

# Frame-rate cap and FPS counter from Portal's game panel (or the
# PORTAL_FPS_LIMIT=N / PORTAL_HUD=1 launch options). A cap already in the
# game's own DXVK_CONFIG wins.
case "${PORTAL_FPS_LIMIT:-0}" in
    ''|0|*[!0-9]*) ;;
    *)
        case "$DXVK_CONFIG" in
            *maxFrameRate*) ;;
            *) export DXVK_CONFIG="$DXVK_CONFIG; dxgi.maxFrameRate = $PORTAL_FPS_LIMIT; d3d9.maxFrameRate = $PORTAL_FPS_LIMIT" ;;
        esac
        export VKD3D_FRAME_RATE="${VKD3D_FRAME_RATE:-$PORTAL_FPS_LIMIT}"
        ;;
esac
[ "$PORTAL_HUD" = 1 ] && export DXVK_HUD="${DXVK_HUD:-fps}"

if [ "$PORTAL_WOW64" = box64 ]; then
    if [ -f /opt/box64/wowbox64.dll ]; then
        export HODLL=wowbox64.dll
        export WINEDLLPATH="/opt/box64${WINEDLLPATH:+:$WINEDLLPATH}"
    else
        echo "portal-proton: Box64 is not installed; using FEX" >&2
    fi
fi

exec "$proton" "$verb" "$@"
