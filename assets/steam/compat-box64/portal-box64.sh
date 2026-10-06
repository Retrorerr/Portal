#!/bin/sh
# Managed by Portal: runs x86-64 Linux games with Box64.
#
# On ARM64 Steam maps native Linux titles to Steam Linux Runtime and slots
# Valve's FEX tool under it. That chain needs SteamOS's x86 rootfs and a
# bwrap container, neither of which exists under PRoot, so the game exits
# at once. This tool drops those layers and runs the game's own binary
# through Box64, which calls the guest's native GL, Vulkan, SDL and ALSA.
#
# Per-game launch options (Properties > General > Launch Options):
#   PORTAL_BOX64=/path/to/box64 %command%   use a specific Box64 build

# Steam drops a game's output; keep the last run's for diagnosis.
log="${TMPDIR:-/tmp}/portal-box64-${SteamAppId:-game}.log"
exec >"$log" 2>&1 </dev/null
echo "portal-box64: $*"

verb="$1"
[ "$#" -gt 0 ] && shift
[ "$1" = "--" ] && shift

message() {
    if command -v zenity >/dev/null 2>&1; then
        zenity --error --title="Portal Box64" --width=460 --text="$1" 2>/dev/null
    fi
    echo "portal-box64: $1" >&2
}

# Drop Steam's runtime and emulator layers: each is "<entry point> ... --".
while [ "$#" -gt 0 ]; do
    case "$1" in
        */_v2-entry-point|*/scout-on-soldier-entry-point-v2|*/fex-compat-tool|*/SteamLinuxRuntime*/run|*/run-in-*)
            while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do shift; done
            [ "$#" -gt 0 ] && shift
            ;;
        *) break ;;
    esac
done

if [ "$#" -eq 0 ]; then
    message "Steam gave no game command to run."
    exit 1
fi

# Steam's ARM64 client libraries are not the game's.
if [ -n "$LD_LIBRARY_PATH" ]; then
    LD_LIBRARY_PATH=$(printf '%s' "$LD_LIBRARY_PATH" | tr ':' '\n' | grep -v -e '/steamrtarm64' -e '/ubuntu12_32' -e '/linuxarm64' | paste -sd: -)
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

# Games bundle SDL builds that read SDL_VIDEODRIVER as one driver name and
# fail on Portal's "wayland,x11" list; Xwayland suits every one of them.
export SDL_VIDEODRIVER="${PORTAL_SDL_VIDEODRIVER:-x11}"
ulimit -n "$(ulimit -Hn)" 2>/dev/null
export MESA_SHADER_CACHE_MAX_SIZE="${MESA_SHADER_CACHE_MAX_SIZE:-4G}"

box64="${PORTAL_BOX64:-$(command -v box64)}"

# The ELF class and machine of a file: x86_64, i386, aarch64 or empty.
elf_kind() {
    [ -f "$1" ] || return
    head=$(od -A n -t x1 -N 20 "$1" 2>/dev/null | tr -d ' \n')
    case "$head" in
        7f454c46*) ;;
        *) return ;;
    esac
    case "$(printf '%s' "$head" | cut -c37-40)" in
        3e00) echo x86_64 ;;
        0300) echo i386 ;;
        b700) echo aarch64 ;;
    esac
}

# The game binary is the first argument that names an ELF file; anything
# before it (a launch-option wrapper such as gamemoderun) runs as is.
index=0
target=
for arg in "$@"; do
    index=$((index + 1))
    kind=$(elf_kind "$arg")
    if [ -n "$kind" ]; then
        target="$arg"
        break
    fi
done

case "$kind" in
    aarch64|"")
        # Native ARM64 build, or a script Box64 cannot start: run unchanged.
        exec "$@"
        ;;
    i386)
        message "This game is a 32-bit x86 Linux build, which Portal cannot run yet.\n\nIn Properties > Compatibility, force \"Portal Proton (ARM64 + FEX)\" to run its Windows version instead, if it has one."
        exit 1
        ;;
esac

if [ -z "$box64" ] || [ ! -x "$box64" ]; then
    message "Box64 is not installed. Remove and reinstall Steam from Portal's Apps panel to add it."
    exit 1
fi

# Rebuild the argument list with box64 in front of the game binary.
count=0
for arg in "$@"; do
    count=$((count + 1))
    if [ "$count" -eq "$index" ]; then
        set -- "$@" "$box64" "$arg"
    else
        set -- "$@" "$arg"
    fi
done
shift "$count"

exec "$@"
