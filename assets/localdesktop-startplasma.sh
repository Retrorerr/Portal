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
export DESKTOP_SESSION=plasma
export KDE_FULL_SESSION=true
export KDE_SESSION_VERSION=6
export KDE_USE_SYSTEMD=0
export PLASMA_USE_SYSTEMD=0
export QT_WAYLAND_SHELL_INTEGRATION=xdg-shell
export QT_SCALE_FACTOR=@UI_SCALE@
export PLASMA_USE_QT_SCALING=1
export ELECTRON_DISABLE_SANDBOX=1
export LOCALDESKTOP_DIAGNOSTICS=1
# Debugger capture is opt-in.  Running every KWin instance under gdb changes
# startup timing and ptrace is commonly denied by Android's sandbox.
export LOCALDESKTOP_GDB_BACKTRACE=${LOCALDESKTOP_GDB_BACKTRACE:-@GDB_BACKTRACE@}
# Keep a protocol trace in the bounded guest session log when startup needs
# diagnosis.  Callers can explicitly set WAYLAND_DEBUG=0 for normal runs.
export WAYLAND_DEBUG=${WAYLAND_DEBUG:-1}

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
for package in kwin plasma-workspace plasma-desktop qt6-wayland kwayland-integration; do
    if command -v pacman >/dev/null 2>&1; then
        version=$(pacman -Q "$package" 2>/dev/null || true)
    else
        version="unavailable"
    fi
    printf 'package %s=%q\n' "$package" "${version:-unavailable}" >> "$session_log"
done

# Keep the diagnostic archive useful even if WAYLAND_DEBUG produces a very
# verbose session. Only the launch-relevant values are persisted.
for name in HOME USER LOGNAME WAYLAND_DISPLAY XDG_RUNTIME_DIR XDG_SESSION_TYPE \
    XDG_CURRENT_DESKTOP DESKTOP_SESSION KDE_FULL_SESSION KDE_SESSION_VERSION \
    KDE_USE_SYSTEMD PLASMA_USE_SYSTEMD QT_SCALE_FACTOR WAYLAND_DEBUG; do
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

# A normal dbus-run-session owns the bus for the complete Plasma process tree;
# no user systemd daemon is started in PRoot.
dbus-run-session -- /usr/bin/startplasma-wayland >> "$session_log" 2>&1 &
session_pid=$!
printf 'stage=session-start pid=%s timestamp=%s\n' "$session_pid" "$(date +%s)" >> "$session_log"

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
