#!/bin/bash
# A graphical, self-explanatory recovery session.  It deliberately does not
# launch a terminal: users can retry Plasma or inspect the captured logs from
# a KDE dialog while the lightweight labwc compositor remains responsive.
set -o pipefail

if [ -f /tmp/localdesktop-output ]; then
    . /tmp/localdesktop-output
fi

output_mode="${LOCALDESKTOP_OUTPUT_MODE:-3392x2400}"
output_scale="${LOCALDESKTOP_OUTPUT_SCALE:-@UI_SCALE@}"
case "$output_scale" in
    ''|*@*|*[!0-9.]*) output_scale=2 ;;
esac

export XDG_RUNTIME_DIR=/tmp
export WAYLAND_DISPLAY=wayland-0
export XDG_SESSION_TYPE=wayland
export XDG_CURRENT_DESKTOP=KDE
export KDE_FULL_SESSION=true
export ELECTRON_DISABLE_SANDBOX=1
export QT_SCALE_FACTOR="$output_scale"
export PLASMA_USE_QT_SCALING=1
export QT_USE_PHYSICAL_DPI=1
export QT_AUTO_SCREEN_SCALE_FACTOR=0
export GDK_SCALE="${output_scale%.*}"
cursor_int="${output_scale%.*}"
[ "$cursor_int" -ge 1 ] 2>/dev/null || cursor_int=2
export XCURSOR_SIZE=$(( 24 * cursor_int ))

state_dir=/var/lib/localdesktop
mkdir -p "$state_dir"
failure_marker="$state_dir/plasma-failed"
labwc_pid_file="$state_dir/labwc.pid"
recovery_log="$state_dir/recovery.log"
plasma_log="$state_dir/plasma.log"
kwin_log="$state_dir/kwin.log"
backtrace_log="$state_dir/kwin-backtrace.log"
touch "$plasma_log" "$kwin_log" "$backtrace_log"

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

    # Ensure labwc nested output does not default to 1280x720
    cat > "$config_dir/rc.xml" <<RCXML
<?xml version="1.0"?>
<labwc_config>
  <core>
    <gap>0</gap>
  </core>
  <output name="*">
    <mode>${output_mode}</mode>
    <scale>${output_scale}</scale>
  </output>
</labwc_config>
RCXML

    cat > "$config_dir/environment" <<ENV
QT_SCALE_FACTOR=${output_scale}
PLASMA_USE_QT_SCALING=1
QT_USE_PHYSICAL_DPI=1
GDK_SCALE=${output_scale%.*}
XCURSOR_SIZE=$(( 24 * cursor_int ))
ENV

    cat > "$config_dir/autostart" <<'AUTOSTART'
#!/bin/sh
state_dir=/var/lib/localdesktop
failure_marker="$state_dir/plasma-failed"
message="$state_dir/recovery-message.txt"

if [ -f /tmp/localdesktop-output ]; then
    . /tmp/localdesktop-output
fi

output_mode="${LOCALDESKTOP_OUTPUT_MODE:-3392x2400}"
output_scale="${LOCALDESKTOP_OUTPUT_SCALE:-2}"

if command -v wlr-randr >/dev/null 2>&1; then
    wlr-randr --output WL-1 --custom-mode "${output_mode}@60Hz" --scale "${output_scale}" 2>/dev/null || \
    wlr-randr --output WL-1 --mode "${output_mode}" --scale "${output_scale}" 2>/dev/null || true
fi

(
    # Keep the recovery action available while labwc is alive. Choosing No
    # only opens the logs and returns to the prompt; it must not strand the
    # user in an otherwise empty compositor with no retry path.
    while true; do
        if kdialog --title "Local Desktop recovery" --yes-label "Retry Plasma" --no-label "View logs" --warningyesno "Plasma did not start. Retry Plasma now?"; then
            rm -f "$failure_marker" "$state_dir/kwin-crash" "$state_dir/plasma-ready"
            labwc_pid=$(cat "$state_dir/labwc.pid" 2>/dev/null || true)
            case "$labwc_pid" in
                ''|*[!0-9]*) ;;
                *) [ "$labwc_pid" -gt 1 ] && kill -TERM "$labwc_pid" 2>/dev/null || true ;;
            esac
            exit 0
        fi
        kdialog --title "Local Desktop recovery logs" --textbox "$message" 760 520 2>/dev/null || true
        kdialog --title "Local Desktop recovery logs" --textbox "$state_dir/plasma.log" 980 680 2>/dev/null || true
    done
) &
AUTOSTART
    chmod 755 "$config_dir/autostart"
}

write_recovery_autostart
printf 'stage=recovery-start timestamp=%s reason=%s\n' "$(date +%s)" "$(cat "$failure_marker" 2>/dev/null || echo unknown)" >> "$recovery_log"

labwc >> "$recovery_log" 2>&1 &
labwc_pid=$!
printf '%s\n' "$labwc_pid" > "$labwc_pid_file"
wait "$labwc_pid"
status=$?
rm -f "$labwc_pid_file"
printf 'stage=recovery-exit timestamp=%s status=%s\n' "$(date +%s)" "$status" >> "$recovery_log"
trim_log "$recovery_log"
trim_log "$plasma_log"
trim_log "$kwin_log"
trim_log "$backtrace_log"

if [ ! -e "$failure_marker" ]; then
    exec /usr/local/bin/startplasma-localdesktop
fi
exit "$status"
