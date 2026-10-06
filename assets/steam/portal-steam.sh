#!/bin/sh
# Managed by Portal: starts Valve's native ARM64 Steam client.
#
# The client lives in the user's Steam root and keeps itself updated from
# Valve's linuxarm64 channel. Portal only seeds it (the manifest's own
# bootstrapper is the i386 stub) and restarts it when an update asks to,
# which is what steam.sh does on x86.

STEAM_ROOT="${PORTAL_STEAM_ROOT:-$HOME/.local/share/Steam}"
CLIENT="$STEAM_ROOT/steamrtarm64/steam"
BOOTSTRAP=/usr/local/lib/portal/steam/portal-steam-bootstrap.py

notify() {
    if command -v zenity >/dev/null 2>&1; then
        zenity --error --title=Steam --width=420 --text="$1" 2>/dev/null
    else
        echo "Steam: $1" >&2
    fi
}

if [ ! -x "$CLIENT" ]; then
    mkdir -p "$STEAM_ROOT" || exit 1
    if command -v zenity >/dev/null 2>&1; then
        python3 "$BOOTSTRAP" --root "$STEAM_ROOT" 2>"$STEAM_ROOT/portal-bootstrap.log" |
            awk '/^PROGRESS/ { if ($3 > 0) printf "%d\n", $2 * 100 / $3; fflush() }' |
            zenity --progress --auto-close --no-cancel --title=Steam \
                --text="Downloading Steam for ARM64…" --width=420 2>/dev/null
    else
        python3 "$BOOTSTRAP" --root "$STEAM_ROOT" >/dev/null 2>"$STEAM_ROOT/portal-bootstrap.log"
    fi
    if [ ! -x "$CLIENT" ]; then
        notify "Steam could not be downloaded. Check the connection and try again.\n\n$(tail -n 3 "$STEAM_ROOT/portal-bootstrap.log" 2>/dev/null)"
        exit 1
    fi
fi

# The paths Steam, games and tools expect (steam.sh creates the same links).
mkdir -p "$HOME/.steam"
for link in steam root; do
    [ -L "$HOME/.steam/$link" ] || [ ! -e "$HOME/.steam/$link" ] &&
        ln -sfn "$STEAM_ROOT" "$HOME/.steam/$link"
done
# Steam starts every game through sdkarm64/steam-launch-wrapper, and games'
# Steamworks loads sdk64 (x86-64) or sdk32 steamclient.so; Box64 runs the
# x86 ones. Earlier Portal builds pointed sdk64 at linuxarm64: repair it.
for pair in sdkarm64:linuxarm64 sdk64:linux64 sdk32:linux32; do
    link="$HOME/.steam/${pair%%:*}"
    target="$STEAM_ROOT/${pair#*:}"
    if [ -L "$link" ] || [ ! -e "$link" ]; then
        [ "$(readlink "$link")" = "$target" ] || ln -sfn "$target" "$link"
    fi
done
echo $$ > "$HOME/.steam/steam.pid"

# First start: enable Steam Play for every Windows title with Portal's
# Proton tool. Steam owns config.vdf afterwards; a user's choice is kept.
if [ ! -e "$STEAM_ROOT/config/config.vdf" ]; then
    mkdir -p "$STEAM_ROOT/config"
    cat > "$STEAM_ROOT/config/config.vdf" <<'VDF'
"InstallConfigStore"
{
	"Software"
	{
		"Valve"
		{
			"Steam"
			{
				"CompatToolMapping"
				{
					"0"
					{
						"name"		"portal-proton-arm64"
						"config"		""
						"priority"		"75"
					}
				}
			}
		}
	}
}
VDF
fi

export STEAM_RUNTIME_ROOT="$STEAM_ROOT/steamrtarm64"
export LD_LIBRARY_PATH="$STEAM_ROOT/steamrtarm64:$STEAM_ROOT/steamrtarm64/libs${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
# Wine esync wants many descriptors; Android caps the hard limit at 32768.
ulimit -n "$(ulimit -Hn)" 2>/dev/null
# Steam's own scaling follows the KWin output scale it reads through Xwayland.
export STEAM_FORCE_DESKTOPUI_SCALING="${STEAM_FORCE_DESKTOPUI_SCALING:-${QT_SCALE_FACTOR:-1}}"

cd "$STEAM_ROOT" || exit 1

# One launcher owns the client. Steam's own single-instance check misses
# two clients starting at once (a double tap on a desktop icon), and the
# pair then fight over steamwebhelper. A later launch waits for the owner's
# client to come up, then hands its arguments over (steam://, -shutdown).
lock="$HOME/.steam/portal-launcher.lock"
exec 9>>"$lock"
if ! flock -n 9; then
    owner=0
    while :; do
        age=$(( $(date +%s) - $(stat -c %Y "$lock" 2>/dev/null || echo 0) ))
        [ "$age" -ge 20 ] && break
        sleep 1
        if flock -n 9; then owner=1; break; fi
    done
    if [ "$owner" -eq 0 ]; then
        exec 9>&-
        exec "$CLIENT" "$@"
    fi
fi
touch "$lock"

# x86 Linux games go through Portal's Box64 tool rather than Valve's FEX +
# Steam Linux Runtime chain, which cannot start under PRoot.
python3 /usr/local/lib/portal/steam/portal-steam-compat.py "$STEAM_ROOT" 2>/dev/null

while :; do
    "$CLIENT" "$@"
    status=$?
    # 42: an update was installed and the client asks to be restarted.
    [ "$status" -eq 42 ] || exit "$status"
done
