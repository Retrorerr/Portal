#!/bin/bash
# Classic/non-systemd Plasma startup used inside the Android PRoot guest.
# `plasma-dbus-run-session-if-needed` is intentionally not used here: it may
# delegate to a per-user systemd instance, which cannot own a stable user bus
# in this environment.
set -o pipefail

export PIPEWIRE_RUNTIME_DIR=/tmp
export PULSE_SERVER=unix:/tmp/pulse/native
export XDG_RUNTIME_DIR=/run/user/1000
export WAYLAND_DISPLAY=/tmp/wayland-0
export XDG_SESSION_TYPE=wayland
export XDG_SESSION_DESKTOP=plasma
export XDG_SESSION_CLASS=user
export XDG_CURRENT_DESKTOP=KDE
export XDG_CONFIG_DIRS=/etc/xdg
export XDG_DATA_DIRS=/usr/local/share:/usr/share
export XDG_MENU_PREFIX=plasma-
export DESKTOP_SESSION=plasma
export KDE_FULL_SESSION=true
export KDE_SESSION_VERSION=6
export KDE_USE_SYSTEMD=0
export PLASMA_USE_SYSTEMD=0
export QT_WAYLAND_SHELL_INTEGRATION=xdg-shell
export LOCALDESKTOP_DIAGNOSTICS=${LOCALDESKTOP_DIAGNOSTICS:-0}
if [ "$LOCALDESKTOP_DIAGNOSTICS" != 1 ]; then
    export QT_LOGGING_RULES="*.debug=false;*.info=false${QT_LOGGING_RULES:+;$QT_LOGGING_RULES}"
fi
export SHELL=/bin/bash
# Debugger capture is opt-in.  Running every KWin instance under gdb changes
# startup timing and ptrace is commonly denied by Android's sandbox.
export LOCALDESKTOP_GDB_BACKTRACE=${LOCALDESKTOP_GDB_BACKTRACE:-@GDB_BACKTRACE@}
# Keep a protocol trace in the bounded guest session log when startup needs
# diagnosis. Set WAYLAND_DEBUG=1 explicitly when tracing protocols.
export WAYLAND_DEBUG=${WAYLAND_DEBUG:-0}

# Project Anland: the host exports ANLAND_SOCKET for GPU sessions. KWin then
# selects its Anland backend (see the kwin_wayland wrapper) and must NOT use
# Smithay's wayland-0: it serves its own clients on wayland-1, exactly like
# the nested QPainter KWin does today.
if [ -n "${ANLAND_SOCKET:-}" ]; then
    export WAYLAND_DISPLAY=wayland-1
    # Force Plasma/KDE Qt clients onto the Wayland QPA backend on the Anland
    # path: without this, ksmserver/plasmashell can fall back to xcb, fail to
    # start, and plasma_session waits forever for org.kde.ksmserver. DISPLAY
    # stays set for XWayland/Firefox (X11), which select xcb explicitly.
    export QT_QPA_PLATFORM=wayland
    export QT_LOGGING_RULES="kwin_core.debug=true;kwin_backend_anland.debug=true;kwin_scene_opengl.debug=true${QT_LOGGING_RULES:+;$QT_LOGGING_RULES}"
fi

state_dir=/var/lib/localdesktop/session
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
for name in HOME USER LOGNAME WAYLAND_DISPLAY XDG_RUNTIME_DIR XDG_SESSION_TYPE XDG_SESSION_DESKTOP \
    XDG_CURRENT_DESKTOP DESKTOP_SESSION KDE_FULL_SESSION KDE_SESSION_VERSION \
    KDE_USE_SYSTEMD PLASMA_USE_SYSTEMD \
    WAYLAND_DEBUG LOCALDESKTOP_CLIPBOARD_HOST LOCALDESKTOP_CLIPBOARD_PORT \
    ANLAND_SOCKET ANLAND ANLAND_NO_DRM_DEVICE MESA_LOADER_DRIVER_OVERRIDE GALLIUM_DRIVER \
    FD_FORCE_KGSL FD_KGSL_ENABLE_DMABUF ANLAND_SKIP_IMPLICIT_SYNC_WAIT \
    ANLAND_DISABLE_AUDIO; do
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
        # Keep the inode: running children retain append descriptors to it.
        # Renaming here would leave them writing an unlinked, unbounded file.
        if tail -c 4194304 "$path" > "$path.trim" 2>/dev/null; then
            : > "$path"
            cat "$path.trim" >> "$path"
            rm -f "$path.trim"
        fi
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
    # KWin's wrapper takes the first free wayland-N in the per-user runtime
    # directory (Anland's host socket lives in /tmp), so discover the name.
    (
        while kill -0 "$session_pid" 2>/dev/null; do
            for inner_socket in "$XDG_RUNTIME_DIR"/wayland-[0-9]*; do
                [ -S "$inner_socket" ] || continue
                printf 'stage=clipboard-bridge status=connecting socket=%s\n' "$inner_socket"
                WAYLAND_DISPLAY="${inner_socket##*/}" \
                    XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
                    /usr/local/bin/localdesktop-clipboard-sync
                break 2
            done
            sleep 1
        done
    ) >> "$session_log" 2>&1 &
    clipboard_bridge_pid=$!
    printf 'stage=clipboard-bridge status=starting pid=%s runtime_dir=%s\n' \
        "$clipboard_bridge_pid" "$XDG_RUNTIME_DIR" >> "$session_log"
}

export LANG=en_GB.UTF-8
export LC_ALL=en_GB.UTF-8

# The Android audio owner starts asynchronously before this launcher. Wait
# for its Pulse endpoint before Plasma's startup notification is dispatched.
audio_connects() {
    python3 -c 'import socket, sys; s=socket.socket(socket.AF_UNIX); s.settimeout(0.2); s.connect(sys.argv[1]); s.close()' /tmp/pulse/native >/dev/null 2>&1
}
audio_ready=0
for _ in $(seq 1 150); do
    if [ -S /tmp/pulse/native ] && audio_connects; then
        audio_ready=1
        break
    fi
    sleep 0.1
done
if [ "$audio_ready" -ne 1 ]; then
    printf 'stage=audio-unavailable action=retry-portal\n' >> "$session_log"
fi

# A normal dbus-run-session owns the bus for the complete Plasma process tree;
# no user systemd daemon is started in PRoot.
dbus-run-session -- /usr/bin/startplasma-wayland >> "$session_log" 2>&1 &
session_pid=$!
printf 'stage=session-start pid=%s timestamp=%s\n' "$session_pid" "$(date +%s)" >> "$session_log"
clipboard_bridge_pid=''
start_clipboard_bridge
(
    while kill -0 "$session_pid" 2>/dev/null; do
        sleep 30
        trim_log "$session_log"
        trim_log "$state_dir/kwin.log"
        trim_log "$state_dir/kwin-backtrace.log"
    done
) &
log_monitor_pid=$!
trap 'kill "$log_monitor_pid" 2>/dev/null || true' EXIT

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
    kill "$log_monitor_pid" 2>/dev/null || true
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
kill "$log_monitor_pid" 2>/dev/null || true
runtime=$(( $(date +%s) - started ))
printf 'stage=exit status=%s runtime=%s timestamp=%s\n' "$status" "$runtime" "$(date +%s)" >> "$session_log"
trim_log "$session_log"

if [ "$status" -ne 0 ] || [ "$runtime" -lt 30 ]; then
    printf 'reason=session-exit status=%s runtime=%s attempt=%s timestamp=%s\n' \
        "$status" "$runtime" "$attempt_id" "$(date +%s)" > "$failure_marker"
    exec /usr/local/bin/start-localdesktop-recovery
fi

exit 0
