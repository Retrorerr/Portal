#!/bin/bash
# Corrected KWin crash wrapper. Install this as /usr/local/bin/kwin_wayland.
# Debugger mode is opt-in because ptrace under Android/PRoot is frequently
# denied and running every launch under gdb changes startup timing.
set -o pipefail

state_dir=/var/lib/localdesktop
mkdir -p "$state_dir"
log_file="$state_dir/kwin.log"
trace_file="$state_dir/kwin-backtrace.log"
crash_file="$state_dir/kwin-crash"
debugger_output="$state_dir/kwin-gdb.log"
rm -f "$crash_file" "$debugger_output"

printf 'timestamp_ms=%s args=%q\n' "$(date +%s%3N 2>/dev/null || date +%s000)" "$*" >> "$log_file"
env | sort >> "$log_file" 2>&1 || true
ulimit -c unlimited 2>/dev/null || true

status=0
run_normally=1
if [ "${LOCALDESKTOP_GDB_BACKTRACE:-0}" = "1" ] && command -v gdb >/dev/null 2>&1; then
    gdb --batch --quiet \
        -ex 'set pagination off' \
        -ex run \
        -ex 'thread apply all bt full' \
        --args /usr/bin/kwin_wayland "$@" \
        > "$debugger_output" 2>&1
    gdb_status=$?
    cat "$debugger_output" >> "$trace_file" 2>/dev/null || true

    # A denied ptrace or an inability to launch gdb must never become a second
    # startup failure. Retry the real binary once in the normal path.
    if grep -Eqi 'ptrace|operation not permitted|permission denied|no such file|cannot execute' "$debugger_output" && \
        ! grep -Eqi 'Program received signal|SIG(SEGV|ABRT|BUS|ILL|FPE)' "$debugger_output"; then
        run_normally=1
    else
        run_normally=0
        status="$gdb_status"
        # gdb commonly returns zero after reporting the child signal; derive a
        # shell-compatible signal status from its own transcript.
        if grep -Eqi 'Program received signal SIGSEGV|SIGSEGV' "$debugger_output"; then status=139; fi
        if grep -Eqi 'Program received signal SIGABRT|SIGABRT' "$debugger_output"; then status=134; fi
    fi
fi

if [ "$run_normally" -eq 1 ]; then
    /usr/bin/kwin_wayland "$@"
    status=$?
fi

if [ "$status" -ge 128 ]; then
    printf 'timestamp_ms=%s status=%s pid=%s args=%q\n' \
        "$(date +%s%3N 2>/dev/null || date +%s000)" "$status" "$$" "$*" \
        > "$crash_file"
    core=$(find "$state_dir" -maxdepth 1 -type f -name 'core*' -print -quit 2>/dev/null || true)
    if [ -n "$core" ] && command -v gdb >/dev/null 2>&1; then
        gdb --batch --quiet /usr/bin/kwin_wayland "$core" \
            -ex 'set pagination off' -ex 'thread apply all bt full' \
            >> "$trace_file" 2>&1 || true
    fi
    if [ ! -s "$trace_file" ]; then
        printf 'best-effort: gdb/coredump unavailable; signal=%s\n' "$((status - 128))" >> "$trace_file"
    fi
fi
exit "$status"
