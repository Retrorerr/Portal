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
export LOCALDESKTOP_GDB_BACKTRACE=${LOCALDESKTOP_GDB_BACKTRACE:-1}

state_dir=/var/lib/localdesktop
mkdir -p "$state_dir"
ready_marker="$state_dir/plasma-ready"
failure_marker="$state_dir/plasma-failed"
crash_marker="$state_dir/kwin-crash"
session_log="$state_dir/plasma.log"
rm -f "$ready_marker" "$failure_marker"

started=$(date +%s)
printf 'stage=launch timestamp=%s mode=classic-dbus-run-session\n' "$started" > "$session_log"

# A normal dbus-run-session owns the bus for the complete Plasma process tree;
# no user systemd daemon is started in PRoot.
dbus-run-session -- /usr/bin/startplasma-wayland >> "$session_log" 2>&1 &
session_pid=$!
printf 'stage=session-start pid=%s timestamp=%s\n' "$session_pid" "$(date +%s)" >> "$session_log"

ready=0
for _ in $(seq 1 120); do
    if [ -e "$crash_marker" ]; then
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
    [ -e "$crash_marker" ] && reason=kwin-crash
    printf 'reason=%s runtime=%s pid=%s timestamp=%s\n' "$reason" "$runtime" "$session_pid" "$(date +%s)" > "$failure_marker"
    printf 'stage=failed reason=%s runtime=%s\n' "$reason" "$runtime" >> "$session_log"
    kill -TERM "$session_pid" 2>/dev/null || true
    wait "$session_pid" 2>/dev/null || true
    exec /usr/local/bin/start-localdesktop-recovery
fi

wait "$session_pid"
status=$?
runtime=$(( $(date +%s) - started ))
printf 'stage=exit status=%s runtime=%s timestamp=%s\n' "$status" "$runtime" "$(date +%s)" >> "$session_log"

if [ "$status" -ne 0 ] || [ "$runtime" -lt 30 ]; then
    printf 'reason=session-exit status=%s runtime=%s timestamp=%s\n' "$status" "$runtime" "$(date +%s)" > "$failure_marker"
    exec /usr/local/bin/start-localdesktop-recovery
fi

exit 0
