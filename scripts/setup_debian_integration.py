#!/usr/bin/env python3
import os
import stat
from pathlib import Path

def setup_integration(rootfs: Path):
    bin_dir = rootfs / "usr" / "local" / "bin"
    bin_dir.mkdir(parents=True, exist_ok=True)
    lib_dir = rootfs / "usr" / "local" / "lib"
    lib_dir.mkdir(parents=True, exist_ok=True)
    etc_localdesktop = rootfs / "etc" / "localdesktop"
    etc_localdesktop.mkdir(parents=True, exist_ok=True)
    state_dir = rootfs / "var" / "lib" / "localdesktop"
    state_dir.mkdir(parents=True, exist_ok=True)

    # 1. startplasma-localdesktop
    startplasma_script = """#!/bin/bash
set -o pipefail

export LANG=C.UTF-8
export LC_ALL=C.UTF-8
export PIPEWIRE_RUNTIME_DIR=/tmp
export PULSE_SERVER=unix:/tmp/pulse/native
export XDG_RUNTIME_DIR=/tmp
export WAYLAND_DISPLAY=wayland-0
export XDG_SESSION_TYPE=wayland
export XDG_CURRENT_DESKTOP=KDE
export DESKTOP_SESSION=plasma
export KDE_FULL_SESSION=true
export KDE_SESSION_VERSION=6
export KDE_USE_SYSTEMD=0
export PLASMA_USE_SYSTEMD=0
export KDE_NO_PORTAL=1
export GTK_USE_PORTAL=0
export QT_NO_XDG_DESKTOP_PORTAL=1
export QT_WAYLAND_SHELL_INTEGRATION=xdg-shell
export QT_SCALE_FACTOR=1
export PLASMA_USE_QT_SCALING=1
export ELECTRON_DISABLE_SANDBOX=1
export LOCALDESKTOP_DIAGNOSTICS=1
export WAYLAND_DEBUG=${WAYLAND_DEBUG:-1}
export XDG_DATA_DIRS=/usr/local/share:/usr/share
export LD_LIBRARY_PATH="/usr/local/lib:/usr/lib/aarch64-linux-gnu:/lib/aarch64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export PATH="/usr/local/bin:/usr/lib/aarch64-linux-gnu/libexec:/usr/bin:/bin:/usr/sbin:/sbin:$PATH"

state_dir=/var/lib/localdesktop
mkdir -p "$state_dir"
ready_marker="$state_dir/plasma-ready"
failure_marker="$state_dir/plasma-failed"
crash_marker="$state_dir/kwin-crash"
session_log="$state_dir/plasma.log"
attempt_id="$(date +%s)-$$"
export LOCALDESKTOP_ATTEMPT_ID="$attempt_id"
rm -f "$ready_marker" "$failure_marker" "$crash_marker"

started=$(date +%s)
printf 'stage=launch timestamp=%s attempt=%s distro=debian13 mode=classic-dbus-run-session\\n' \\
    "$started" "$attempt_id" > "$session_log"

home_dir="${HOME:-/home/desktop}"
config_dir="$home_dir/.config"
mkdir -p "$config_dir"
startkderc="$config_dir/startkderc"
if command -v kwriteconfig6 >/dev/null 2>&1; then
    kwriteconfig6 --file "$startkderc" --group General --key systemdBoot false
else
    printf '[General]\\nsystemdBoot=false\\n' > "$startkderc"
fi

ksmserverrc="$config_dir/ksmserverrc"
if command -v kwriteconfig6 >/dev/null 2>&1; then
    kwriteconfig6 --file "$ksmserverrc" --group General --key loginMode emptySession
    kwriteconfig6 --file "$ksmserverrc" --group General --key confirmLogout false
else
    printf '[General]\\nloginMode=emptySession\\nconfirmLogout=false\\n' > "$ksmserverrc"
fi
rm -f "$config_dir/autostart/konsole.desktop" "$config_dir/autostart/org.kde.konsole.desktop"

ksplashrc="$config_dir/ksplashrc"
if command -v kwriteconfig6 >/dev/null 2>&1; then
    kwriteconfig6 --file "$ksplashrc" --group KSplash --key Theme None
else
    printf '[KSplash]\\nTheme=None\\n' > "$ksplashrc"
fi

# Disable screen locking completely: Android/OxygenOS owns device security
kdeglobals="$config_dir/kdeglobals"
if command -v kwriteconfig6 >/dev/null 2>&1; then
    kwriteconfig6 --file "$kdeglobals" --group 'KDE Action Restrictions][$i' --key 'action/lock_screen' false
else
    printf '[KDE Action Restrictions][$i]\\naction/lock_screen=false\\n' > "$kdeglobals"
fi

kscreenlockerrc="$config_dir/kscreenlockerrc"
if command -v kwriteconfig6 >/dev/null 2>&1; then
    kwriteconfig6 --file "$kscreenlockerrc" --group 'Daemon][$i' --key Autolock false
    kwriteconfig6 --file "$kscreenlockerrc" --group 'Daemon][$i' --key LockOnResume false
    kwriteconfig6 --file "$kscreenlockerrc" --group 'Daemon][$i' --key Timeout 0
else
    printf '[Daemon][$i]\\nAutolock=false\\nLockOnResume=false\\nTimeout=0\\n' > "$kscreenlockerrc"
fi

# Run ldconfig once if needed
if [ ! -f /etc/ld.so.cache ] && command -v ldconfig >/dev/null 2>&1; then
    ldconfig 2>/dev/null || true
fi

# Pre-launch activity manager daemon so plasmashell finds it immediately
/usr/lib/aarch64-linux-gnu/libexec/kactivitymanagerd &

dbus-run-session -- /usr/bin/startplasma-wayland >> "$session_log" 2>&1 &
session_pid=$!
printf 'stage=session-start pid=%s timestamp=%s\\n' "$session_pid" "$(date +%s)" >> "$session_log"

ready=0
for _ in $(seq 1 120); do
    if [ -s "$crash_marker" ] && grep -Fq "attempt=$attempt_id" "$crash_marker"; then
        break
    fi
    if [ -s "$ready_marker" ]; then
        ready=1
        printf 'stage=ready timestamp=%s evidence=%s\\n' "$(date +%s)" "$(cat "$ready_marker")" >> "$session_log"
        break
    fi
    if ! kill -0 "$session_pid" 2>/dev/null; then
        break
    fi
    sleep 1
done

if [ "$ready" -ne 1 ]; then
    runtime=$(( $(date +%s) - started ))
    printf 'reason=startup-timeout-or-exit runtime=%s pid=%s\\n' "$runtime" "$session_pid" > "$failure_marker"
    kill -TERM "$session_pid" 2>/dev/null || true
    sleep 1
    kill -KILL "$session_pid" 2>/dev/null || true
fi

wait "$session_pid"
"""
    p = bin_dir / "startplasma-localdesktop"
    p.write_text(startplasma_script, encoding="utf-8", newline="\n")
    p.chmod(p.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

    # 2. kwin_wayland wrapper
    kwin_wrapper = """#!/bin/bash
set -o pipefail

state_dir=/var/lib/localdesktop
mkdir -p "$state_dir"
log_file="$state_dir/kwin.log"
trace_file="$state_dir/kwin-backtrace.log"
crash_file="$state_dir/kwin-crash"
attempt_id="${LOCALDESKTOP_ATTEMPT_ID:-$(date +%s)-$$}"
export LOCALDESKTOP_ATTEMPT_ID="$attempt_id"

export LD_LIBRARY_PATH="/usr/local/lib:/usr/lib/aarch64-linux-gnu:/lib/aarch64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export QT_FORCE_STDERR_LOGGING=1
export QT_LOGGING_RULES="kwin_core.warning=true;kwin_qpa.warning=true"

if [ -r /usr/local/lib/localdesktop-crash-handler.so ]; then
    export LD_PRELOAD="/usr/local/lib/localdesktop-crash-handler.so${LD_PRELOAD:+:$LD_PRELOAD}"
    export SEGFAULT_SIGNALS=all
    export LOCALDESKTOP_CRASH_LOG="$trace_file"
fi

printf 'timestamp_ms=%s attempt=%s args=%q\\n' "$(date +%s000)" "$attempt_id" "$*" >> "$log_file"

/usr/bin/kwin_wayland --no-lockscreen "$@" 2>&1 | tee -a "$log_file"
status="${PIPESTATUS[0]}"

if [ "$status" -ge 128 ]; then
    printf 'timestamp_ms=%s attempt=%s status=%s pid=%s\\n' "$(date +%s000)" "$attempt_id" "$status" "$$" > "$crash_file"
    printf 'crash-summary timestamp_ms=%s attempt=%s status=%s signal=%s\\n' "$(date +%s000)" "$attempt_id" "$status" "$((status - 128))" >> "$trace_file"
fi

exit "$status"
"""
    p = bin_dir / "kwin_wayland"
    p.write_text(kwin_wrapper, encoding="utf-8", newline="\n")
    p.chmod(p.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

    # 3. ksplashqml stub
    p = bin_dir / "ksplashqml"
    p.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8", newline="\n")
    p.chmod(p.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

    # 4. plasma_waitforname stub
    p = bin_dir / "plasma_waitforname"
    p.write_text('#!/bin/sh\nif [ "$1" = "org.kde.KSplash" ]; then\n    exit 0\nfi\nexec /usr/bin/plasma_waitforname "$@"\n', encoding="utf-8", newline="\n")
    p.chmod(p.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

    print("Debian integration scripts successfully installed into /usr/local/bin!")

if __name__ == "__main__":
    root = Path("target/debian-13-rootfs")
    setup_integration(root)
