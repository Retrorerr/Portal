#!/bin/sh
# Called by the graphical recovery dialog.
set -eu
state_dir=/var/lib/localdesktop
rm -f "$state_dir/plasma-failed" "$state_dir/kwin-crash" "$state_dir/plasma-ready"
labwc_pid=$(cat "$state_dir/labwc.pid" 2>/dev/null || true)
case "$labwc_pid" in
    ''|*[!0-9]*) ;;
    *) [ "$labwc_pid" -gt 1 ] && kill -TERM "$labwc_pid" 2>/dev/null || true ;;
esac
exit 0
