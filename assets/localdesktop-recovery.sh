#!/bin/bash
# A graphical, self-explanatory recovery session.  It deliberately does not
# launch a terminal: users can retry Plasma or inspect the captured logs from
# a KDE dialog while the lightweight labwc compositor remains responsive.
set -o pipefail

export XDG_RUNTIME_DIR=/tmp
export WAYLAND_DISPLAY=wayland-0
export XDG_SESSION_TYPE=wayland
export XDG_CURRENT_DESKTOP=KDE
export KDE_FULL_SESSION=true
export ELECTRON_DISABLE_SANDBOX=1

state_dir=/var/lib/localdesktop
mkdir -p "$state_dir"
failure_marker="$state_dir/plasma-failed"
recovery_log="$state_dir/recovery.log"
plasma_log="$state_dir/plasma.log"
kwin_log="$state_dir/kwin.log"
backtrace_log="$state_dir/kwin-backtrace.log"
touch "$plasma_log" "$kwin_log" "$backtrace_log"

cat > "$state_dir/recovery-message.txt" <<EOF
Local Desktop could not start Plasma.

Your Linux files are safe. Choose Retry Plasma to try again, or View logs to
inspect the captured startup and KWin diagnostics. Export diagnostics is
available from the Android setup/error screen.

$(cat "$failure_marker" 2>/dev/null || true)
EOF

write_recovery_autostart() {
    local home_dir="${HOME:-/root}"
    local config_dir="$home_dir/.config/labwc"
    mkdir -p "$config_dir"
    cat > "$config_dir/autostart" <<'AUTOSTART'
#!/bin/sh
state_dir=/var/lib/localdesktop
failure_marker="$state_dir/plasma-failed"
message="$state_dir/recovery-message.txt"

(
    if kdialog --title "Local Desktop recovery" --warningyesno "Plasma did not start. Retry Plasma now?"; then
        rm -f "$failure_marker" "$state_dir/kwin-crash" "$state_dir/plasma-ready"
        pkill -TERM -x labwc 2>/dev/null || true
    else
        kdialog --title "Local Desktop recovery logs" --textbox "$message" 760 520 2>/dev/null || true
        kdialog --title "Local Desktop recovery logs" --textbox "$state_dir/plasma.log" 980 680 2>/dev/null || true
    fi
) &
AUTOSTART
    chmod 755 "$config_dir/autostart"
}

write_recovery_autostart
printf 'stage=recovery-start timestamp=%s reason=%s\n' "$(date +%s)" "$(cat "$failure_marker" 2>/dev/null || echo unknown)" >> "$recovery_log"

labwc >> "$recovery_log" 2>&1
status=$?
printf 'stage=recovery-exit timestamp=%s status=%s\n' "$(date +%s)" "$status" >> "$recovery_log"

if [ ! -e "$failure_marker" ]; then
    exec /usr/local/bin/startplasma-localdesktop
fi
exit "$status"
