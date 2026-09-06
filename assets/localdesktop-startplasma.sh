#!/bin/bash
# Classic/non-systemd Plasma startup used inside the Android PRoot guest.
# `plasma-dbus-run-session-if-needed` is intentionally not used here: it may
# delegate to a per-user systemd instance, which cannot own a stable user bus
# in this environment.
set -o pipefail

export PIPEWIRE_RUNTIME_DIR=/tmp
export PULSE_SERVER=unix:/tmp/pulse/native
export XDG_RUNTIME_DIR=/tmp
export WAYLAND_DISPLAY=wayland-0
export XDG_SESSION_TYPE=wayland
export XDG_CURRENT_DESKTOP=KDE
export XDG_MENU_PREFIX=plasma-
export DESKTOP_SESSION=plasma
export KDE_FULL_SESSION=true
export KDE_SESSION_VERSION=6
export KDE_USE_SYSTEMD=0
export PLASMA_USE_SYSTEMD=0
export KDE_NO_PORTAL=1
export GTK_USE_PORTAL=0
# Qt can probe the desktop portal while QGuiApplication is constructed.
# The session has no portal-ready desktop yet, so that probe can block
# kcminit_startup before it publishes its readiness pipe byte.
export QT_NO_XDG_DESKTOP_PORTAL=1
export QT_WAYLAND_SHELL_INTEGRATION=xdg-shell
export ELECTRON_DISABLE_SANDBOX=1
export LOCALDESKTOP_DIAGNOSTICS=1
export SHELL=/bin/bash
# Debugger capture is opt-in.  Running every KWin instance under gdb changes
# startup timing and ptrace is commonly denied by Android's sandbox.
export LOCALDESKTOP_GDB_BACKTRACE=${LOCALDESKTOP_GDB_BACKTRACE:-@GDB_BACKTRACE@}
# Keep a protocol trace in the bounded guest session log when startup needs
# diagnosis. Set WAYLAND_DEBUG=1 explicitly when tracing protocols.
export WAYLAND_DEBUG=${WAYLAND_DEBUG:-0}

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
printf 'stage=launch timestamp=%s attempt=%s mode=classic-dbus-run-session\n' \
    "$started" "$attempt_id" > "$session_log"
printf 'stage=environment timestamp=%s wayland_debug=%s gdb_backtrace=%s\n' \
    "$(date +%s)" "$WAYLAND_DEBUG" "$LOCALDESKTOP_GDB_BACKTRACE" >> "$session_log"
printf 'stage=backend compositor=kwin_wayland session=plasma-wayland launcher=%s\n' \
    "/usr/local/bin/startplasma-localdesktop" >> "$session_log"

# Package versions make a crash archive actionable without dumping the full
# guest package database. Keep this allowlist limited to the components that
# own the startup path and record a clear unavailable value on partial images.
for package in kwin-wayland plasma-workspace plasma-desktop qt6-wayland; do
    if command -v dpkg-query >/dev/null 2>&1; then
        version=$(dpkg-query -W "$package" 2>/dev/null || true)
    else
        version="unavailable"
    fi
    printf 'package %s=%q\n' "$package" "${version:-unavailable}" >> "$session_log"
done

# Keep the diagnostic archive useful even if WAYLAND_DEBUG produces a very
# verbose session. Only the launch-relevant values are persisted.
for name in HOME USER LOGNAME WAYLAND_DISPLAY XDG_RUNTIME_DIR XDG_SESSION_TYPE \
    XDG_CURRENT_DESKTOP DESKTOP_SESSION KDE_FULL_SESSION KDE_SESSION_VERSION \
    KDE_USE_SYSTEMD PLASMA_USE_SYSTEMD QT_NO_XDG_DESKTOP_PORTAL \
    WAYLAND_DEBUG LOCALDESKTOP_CLIPBOARD_HOST LOCALDESKTOP_CLIPBOARD_PORT; do
    eval "value=\${$name-}"
    printf 'env %s=%q\n' "$name" "$value" >> "$session_log"
done

trim_log() {
    local path="$1"
    local max_bytes=8388608
    [ -f "$path" ] || return 0
    local size
    size=$(wc -c < "$path" 2>/dev/null || echo 0)
    if [ "$size" -gt "$max_bytes" ]; then
        tail -c "$max_bytes" "$path" > "$path.trim" 2>/dev/null && mv -f "$path.trim" "$path"
    fi
}

signal_tree() {
    local pid="$1"
    local signal="$2"
    [ "$pid" -gt 1 ] 2>/dev/null || return 0
    for child in $(pgrep -P "$pid" 2>/dev/null || true); do
        signal_tree "$child" "$signal"
    done
    kill -"$signal" "$pid" 2>/dev/null || true
}

reap_session() {
    signal_tree "$session_pid" TERM
    for _ in $(seq 1 20); do
        kill -0 "$session_pid" 2>/dev/null || break
        sleep 0.1
    done
    # dbus-run-session can wait on a misbehaving child. Escalate only through
    # the same process tree, then reap the session leader without an unbounded
    # wait that would strand the setup WebView.
    if kill -0 "$session_pid" 2>/dev/null; then
        signal_tree "$session_pid" KILL
    fi
    wait "$session_pid" 2>/dev/null || true
}

start_clipboard_bridge() {
    [ -x /usr/local/bin/localdesktop-clipboard-sync ] || return 0
    if [ -z "${LOCALDESKTOP_CLIPBOARD_PORT:-}" ] || [ -z "${LOCALDESKTOP_CLIPBOARD_TOKEN:-}" ]; then
        printf 'stage=clipboard-bridge status=disabled reason=broker-environment-missing\n' >> "$session_log"
        return 0
    fi

    # KWin is the outer client's compositor and exposes the Plasma clipboard
    # on its own inner socket. Start the existing helper only after that
    # socket exists; it reconnects across KWin restarts while this session is
    # alive. The helper's process is a child of this launcher and is reaped by
    # the same bounded session cleanup path.
    (
        inner_socket="$XDG_RUNTIME_DIR/wayland-1"
        while kill -0 "$session_pid" 2>/dev/null; do
            if [ -S "$inner_socket" ]; then
                WAYLAND_DISPLAY=wayland-1 \
                    XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
                    /usr/local/bin/localdesktop-clipboard-sync
                break
            fi
            sleep 1
        done
    ) >> "$session_log" 2>&1 &
    clipboard_bridge_pid=$!
    printf 'stage=clipboard-bridge status=starting pid=%s socket=%s\n' \
        "$clipboard_bridge_pid" "$XDG_RUNTIME_DIR/wayland-1" >> "$session_log"
}

# Enforce systemdBoot=false in startkderc so Plasma 6 never hangs waiting for systemd user manager
home_dir="${HOME:-/root}"
config_dir="$home_dir/.config"
mkdir -p "$config_dir"
startkderc="$config_dir/startkderc"
if command -v kwriteconfig6 >/dev/null 2>&1; then
    kwriteconfig6 --file "$startkderc" --group General --key systemdBoot false
else
    if [ ! -f "$startkderc" ]; then
        printf '[General]\nsystemdBoot=false\n' > "$startkderc"
    elif ! grep -q '^systemdBoot=' "$startkderc"; then
        printf '\n[General]\nsystemdBoot=false\n' >> "$startkderc"
    else
        sed -i 's/^systemdBoot=.*/systemdBoot=false/' "$startkderc"
    fi
fi

# Ensure clean UX without unwanted Konsole or debug terminal popups on startup
ksmserverrc="$config_dir/ksmserverrc"
if command -v kwriteconfig6 >/dev/null 2>&1; then
    kwriteconfig6 --file "$ksmserverrc" --group General --key loginMode emptySession
    kwriteconfig6 --file "$ksmserverrc" --group General --key confirmLogout false
else
    if [ ! -f "$ksmserverrc" ]; then
        printf '[General]\nloginMode=emptySession\nconfirmLogout=false\n' > "$ksmserverrc"
    elif ! grep -q '^loginMode=' "$ksmserverrc"; then
        printf '\n[General]\nloginMode=emptySession\nconfirmLogout=false\n' >> "$ksmserverrc"
    else
        sed -i 's/^loginMode=.*/loginMode=emptySession/' "$ksmserverrc"
    fi
fi
rm -f "$config_dir/autostart/konsole.desktop" "$config_dir/autostart/org.kde.konsole.desktop"

# Ensure default Konsole profile and konsolerc exist
konsole_profile_dir="$home_dir/.local/share/konsole"
mkdir -p "$konsole_profile_dir"
if [ ! -f "$konsole_profile_dir/Profile 1.profile" ]; then
    cat <<'EOF' > "$konsole_profile_dir/Profile 1.profile"
[General]
Command=/bin/bash
Name=Profile 1
Parent=FALLBACK/

[Appearance]
ColorScheme=Breeze
EOF
fi

konsolerc="$config_dir/konsolerc"
if command -v kwriteconfig6 >/dev/null 2>&1; then
    kwriteconfig6 --file "$konsolerc" --group 'Desktop Entry' --key DefaultProfile 'Profile 1.profile'
fi

# The Debian image is assembled by extracting a deterministic package closure,
# so maintainer scripts do not run during image creation. Generate only the
# small runtime databases desktop applications actually consume. Each command
# is guarded and idempotent; failures remain non-fatal for an older image.
if command -v localedef >/dev/null 2>&1 && [ ! -e /usr/lib/locale/locale-archive ]; then
    localedef -i en_GB -f UTF-8 en_GB.UTF-8 >> "$session_log" 2>&1 || true
    localedef -i en_US -f UTF-8 en_US.UTF-8 >> "$session_log" 2>&1 || true
    localedef -i C -f UTF-8 C.UTF-8 >> "$session_log" 2>&1 || true
fi
export LANG=en_GB.UTF-8
export LC_ALL=en_GB.UTF-8

# Package extraction also skips gawk's update-alternatives registration.
if ! command -v awk >/dev/null 2>&1 && [ -x /usr/bin/gawk ]; then
    update-alternatives --install /usr/bin/awk awk /usr/bin/gawk 10 >> "$session_log" 2>&1 || exit 1
fi

# Debian maintainer triggers are not run when extracting the rootfs. GLib's
# absent schemas leave GTK's DPI unset (-1), yielding negative Firefox UI fonts.
# GTK's missing module cache also prevents Wayland text-input from loading.
glib-compile-schemas /usr/share/glib-2.0/schemas >> "$session_log" 2>&1 || exit 1
/usr/lib/aarch64-linux-gnu/libgtk-3-0/gtk-query-immodules-3.0 --update-cache >> "$session_log" 2>&1 || exit 1

# Android denies host CPU counters and hides other apps' processes. Stock
# System Monitor must not present fabricated host-wide statistics.
if [ -x /usr/bin/plasma-systemmonitor ]; then
    dpkg --remove plasma-systemmonitor >> "$session_log" 2>&1 || exit 1
fi

cache_marker="$state_dir/desktop-caches-v3"
if [ ! -e "$cache_marker" ]; then
    if command -v update-mime-database >/dev/null 2>&1; then
        update-mime-database /usr/share/mime >> "$session_log" 2>&1 || true
    fi
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database /usr/share/applications >> "$session_log" 2>&1 || true
    fi
    if command -v kbuildsycoca6 >/dev/null 2>&1; then
        kbuildsycoca6 --noincremental >> "$session_log" 2>&1 || true
    fi
    : > "$cache_marker"
fi

# Configure TabletMode default in kwinrc only if not already set; dynamic host bridge manages it
kwinrc="$config_dir/kwinrc"
if ! grep -q 'TabletMode=' "$kwinrc" 2>/dev/null; then
    if command -v kwriteconfig6 >/dev/null 2>&1; then
        kwriteconfig6 --file "$kwinrc" --group Input --key TabletMode off
    else
        if [ ! -f "$kwinrc" ]; then
            printf '[Input]\nTabletMode=off\n' > "$kwinrc"
        else
            printf '\n[Input]\nTabletMode=off\n' >> "$kwinrc"
        fi
    fi
fi

# Configure Virtual Keyboard bridge in kwinrc
if command -v kwriteconfig6 >/dev/null 2>&1; then
    kwriteconfig6 --file "$kwinrc" --group Wayland --key InputMethod "/usr/share/applications/portal-ime.desktop"
    kwriteconfig6 --file "$kwinrc" --group Wayland --key VirtualKeyboardMode 1
elif ! grep -q 'InputMethod=' "$kwinrc" 2>/dev/null; then
    printf '\n[Wayland]\nInputMethod=/usr/share/applications/portal-ime.desktop\nVirtualKeyboardMode=1\n' >> "$kwinrc"
fi

# Session command runner for in-session D-Bus queries and diagnostics
mkdir -p "$config_dir/autostart"
cat << 'RUNNER_EOF' > /usr/local/bin/portal-session-cmd
#!/bin/bash
fifo=/tmp/portal-session-cmd.fifo
out=/tmp/portal-session-cmd.out
rm -f "$fifo" "$out"
mkfifo "$fifo" 2>/dev/null || true
while true; do
    while read -r cmd; do
        eval "$cmd" > "$out" 2>&1
        echo "---PORTAL_CMD_EOF---" >> "$out"
    done < "$fifo"
done
RUNNER_EOF
chmod +x /usr/local/bin/portal-session-cmd 2>/dev/null || true

cat << 'AUTOS_EOF' > "$config_dir/autostart/portal-session-cmd.desktop"
[Desktop Entry]
Type=Application
Name=Portal Session Runner
Exec=/usr/local/bin/portal-session-cmd
X-KDE-autostart-phase=1
AUTOS_EOF

# Disable ksplash to avoid hanging on splash animation under PRoot
ksplashrc="$config_dir/ksplashrc"
if command -v kwriteconfig6 >/dev/null 2>&1; then
    kwriteconfig6 --file "$ksplashrc" --group KSplash --key Theme None
else
    if [ ! -f "$ksplashrc" ]; then
        printf '[KSplash]\nTheme=None\n' > "$ksplashrc"
    elif ! grep -q '^Theme=' "$ksplashrc"; then
        printf '\n[KSplash]\nTheme=None\n' >> "$ksplashrc"
    else
        sed -i 's/^Theme=.*/Theme=None/' "$ksplashrc"
    fi
fi

# Disable screen locking completely: Android/OxygenOS owns device security
kdeglobals="$config_dir/kdeglobals"
if command -v kwriteconfig6 >/dev/null 2>&1; then
    kwriteconfig6 --file "$kdeglobals" --group 'KDE Action Restrictions][$i' --key 'action/lock_screen' false
else
    if [ ! -f "$kdeglobals" ]; then
        printf '[KDE Action Restrictions][$i]\naction/lock_screen=false\n' > "$kdeglobals"
    elif ! grep -q 'action/lock_screen=' "$kdeglobals"; then
        printf '\n[KDE Action Restrictions][$i]\naction/lock_screen=false\n' >> "$kdeglobals"
    fi
fi

kscreenlockerrc="$config_dir/kscreenlockerrc"
if command -v kwriteconfig6 >/dev/null 2>&1; then
    kwriteconfig6 --file "$kscreenlockerrc" --group 'Daemon][$i' --key Autolock false
    kwriteconfig6 --file "$kscreenlockerrc" --group 'Daemon][$i' --key LockOnResume false
    kwriteconfig6 --file "$kscreenlockerrc" --group 'Daemon][$i' --key Timeout 0
else
    printf '[Daemon][$i]\nAutolock=false\nLockOnResume=false\nTimeout=0\n' > "$kscreenlockerrc"
fi

# A normal dbus-run-session owns the bus for the complete Plasma process tree;
# no user systemd daemon is started in PRoot.
dbus-run-session -- /usr/bin/startplasma-wayland >> "$session_log" 2>&1 &
session_pid=$!
printf 'stage=session-start pid=%s timestamp=%s\n' "$session_pid" "$(date +%s)" >> "$session_log"
clipboard_bridge_pid=''
start_clipboard_bridge

ready=0
for _ in $(seq 1 120); do
    if [ -s "$crash_marker" ] && grep -Fq "attempt=$attempt_id" "$crash_marker"; then
        break
    fi
    # This marker is written only after the Android host has dispatched a
    # client, observed a committed surface buffer, rendered it and completed
    # the EGL swap. It is the readiness contract; process liveness is not.
    if [ -s "$ready_marker" ]; then
        ready=1
        printf 'stage=ready timestamp=%s evidence=%s\n' "$(date +%s)" "$(cat "$ready_marker")" >> "$session_log"
        break
    fi
    if ! kill -0 "$session_pid" 2>/dev/null; then
        break
    fi
    sleep 1
done

if [ "$ready" -ne 1 ]; then
    runtime=$(( $(date +%s) - started ))
    reason=startup-timeout-or-exit
    if [ -s "$crash_marker" ] && grep -Fq "attempt=$attempt_id" "$crash_marker"; then
        reason=kwin-crash
    fi
    printf 'reason=%s runtime=%s pid=%s attempt=%s timestamp=%s\n' \
        "$reason" "$runtime" "$session_pid" "$attempt_id" "$(date +%s)" > "$failure_marker"
    printf 'stage=failed reason=%s runtime=%s\n' "$reason" "$runtime" >> "$session_log"
    reap_session
    trim_log "$session_log"
    exec /usr/local/bin/start-localdesktop-recovery
fi

wait "$session_pid"
status=$?
runtime=$(( $(date +%s) - started ))
printf 'stage=exit status=%s runtime=%s timestamp=%s\n' "$status" "$runtime" "$(date +%s)" >> "$session_log"
trim_log "$session_log"

if [ "$status" -ne 0 ] || [ "$runtime" -lt 30 ]; then
    printf 'reason=session-exit status=%s runtime=%s attempt=%s timestamp=%s\n' \
        "$status" "$runtime" "$attempt_id" "$(date +%s)" > "$failure_marker"
    exec /usr/local/bin/start-localdesktop-recovery
fi

exit 0
