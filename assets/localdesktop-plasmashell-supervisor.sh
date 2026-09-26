#!/bin/sh
# Plasma's classic (non-systemd) session does not restart a shell killed after
# startup. Keep the accelerated shell alive for the lifetime of KWin's socket.

log=/var/lib/localdesktop/session/plasmashell-supervisor.log
socket=${XDG_RUNTIME_DIR:-/run/user/1000}/${WAYLAND_DISPLAY:-wayland-1}
child=
stopping=0

stop() {
    stopping=1
    if [ -n "$child" ]; then
        kill -TERM "$child" 2>/dev/null || true
    fi
}
trap stop TERM INT HUP

delay=1
while [ -S "$socket" ] && [ "$stopping" -eq 0 ]; do
    started=$(date +%s)
    /usr/bin/plasmashell &
    child=$!
    shell_pid=$child
    wait "$child"
    status=$?
    child=
    [ "$stopping" -eq 0 ] || break
    [ -S "$socket" ] || break
    [ "$status" -ne 0 ] || break

    now=$(date +%s)
    printf 'time=%s pid=%s exit=%s restart_in=%ss\n' "$now" "$shell_pid" "$status" "$delay" >> "$log"
    # A healthy run resets backoff; a failing GPU path cannot spin forever.
    if [ $((now - started)) -ge 60 ]; then
        delay=1
    fi
    sleep "$delay" &
    child=$!
    wait "$child" 2>/dev/null || true
    child=
    [ "$stopping" -eq 0 ] || break
    [ "$delay" -ge 8 ] || delay=$((delay * 2))
done
