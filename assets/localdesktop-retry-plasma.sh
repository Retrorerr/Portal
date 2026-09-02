#!/bin/sh
# Called by the graphical recovery dialog.
set -eu
state_dir=/var/lib/localdesktop
rm -f "$state_dir/plasma-failed" "$state_dir/kwin-crash" "$state_dir/plasma-ready"
pkill -TERM -x labwc 2>/dev/null || true
exit 0
