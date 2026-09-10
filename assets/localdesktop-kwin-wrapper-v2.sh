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
attempt_id="${LOCALDESKTOP_ATTEMPT_ID:-$(date +%s)-$$}"
export LOCALDESKTOP_ATTEMPT_ID="$attempt_id"
max_log_bytes=8388608

trim_log() {
    local path="$1"
    [ -f "$path" ] || return 0
    local size
    size=$(wc -c < "$path" 2>/dev/null || echo 0)
    if [ "$size" -gt "$max_log_bytes" ]; then
        tail -c "$max_log_bytes" "$path" > "$path.trim" 2>/dev/null && mv -f "$path.trim" "$path"
    fi
}

rotate_if_large() {
    local path="$1"
    if [ -f "$path" ] && [ "$(wc -c < "$path" 2>/dev/null || echo 0)" -gt "$max_log_bytes" ]; then
        mv -f "$path" "$path.1" 2>/dev/null || :
    fi
}

rotate_if_large "$log_file"
rotate_if_large "$trace_file"
# The launcher clears the marker once per session attempt, before starting
# Plasma. Do not remove it here: KDE's kwin_wayland_wrapper may invoke this
# shim again after a crash, and clearing it would race the launcher's failure
# watcher and lose the first crash evidence.
rm -f "$debugger_output"

printf 'timestamp_ms=%s attempt=%s args=%q\n' \
    "$(date +%s%3N 2>/dev/null || date +%s000)" "$attempt_id" "$*" >> "$log_file"
# Record only the launch-relevant environment. Do not dump arbitrary guest
# variables: callers may carry tokens or other values that do not belong in a
# support archive.
for name in HOME USER LOGNAME WAYLAND_DISPLAY XDG_RUNTIME_DIR XDG_SESSION_TYPE \
    XDG_CURRENT_DESKTOP KDE_FULL_SESSION KDE_SESSION_VERSION KDE_USE_SYSTEMD \
    PLASMA_USE_SYSTEMD QT_SCALE_FACTOR WAYLAND_DEBUG LOCALDESKTOP_DIAGNOSTICS \
    LOCALDESKTOP_ATTEMPT_ID; do
    eval "value=\${$name-}"
    printf 'env %s=%q\n' "$name" "$value" >> "$log_file"
done
ulimit -c unlimited 2>/dev/null || true
# Project Anland: when the host runs a GPU session it exports ANLAND_SOCKET.
# The distro libkwin (lfdevs Anland build) owns the Anland backend, so the
# Portal QPainter overlay in /usr/local/lib/portal must NOT shadow it here.
# (The overlay relocation keeps /usr/local/lib itself free of libkwin.so.6.)
anland_mode=0
if [ -n "${ANLAND_SOCKET:-}" ]; then
    anland_mode=1
fi
printf 'anland_mode=%s socket=%s\n' "$anland_mode" "${ANLAND_SOCKET:-unset}" >> "$log_file"
if [ "$anland_mode" -eq 0 ]; then
    export LD_LIBRARY_PATH="/usr/local/lib/portal${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    # Project Anland load-time stub (QPainter only): the lfdevs binary needs
    # AnlandBackend at load while the overlay lib has none. Covers normal and
    # gdb launches below; never set in Anland mode.
    if [ -r /usr/local/lib/portal/libanland-stub.so ]; then
        export LD_PRELOAD="/usr/local/lib/portal/libanland-stub.so${LD_PRELOAD:+:$LD_PRELOAD}"
    fi
else
    # Project Anland KWin variant selection (deterministic A/B without APK
    # rebuilds): /var/lib/localdesktop/kwin-variant selects the libkwin for
    # Anland sessions. Values: "unified" (default ship behavior),
    # "stock" (exact untouched distro lfdevs binaries), or "ab:<name>"
    # (bisect candidate at /usr/local/lib/portal-ab/<name>/libkwin.so.6.3.6,
    # staged out-of-band; never touched by the per-launch overlay sync).
    # The file is read once here, before KWin starts; nothing is swapped
    # while KWin is alive. Any failed sanity check falls back to stock and
    # is logged, so KWin is never left unloadable or half-deployed.
    kwin_variant=unified
    if [ -r /var/lib/localdesktop/kwin-variant ]; then
        read -r kwin_variant < /var/lib/localdesktop/kwin-variant
    fi
    kwin_anland_dir=/usr/local/lib/portal-anland
    case "$kwin_variant" in
        ab:*)
            ab_name=${kwin_variant#ab:}
            case "$ab_name" in
                *[!a-zA-Z0-9._-]* | "" | .* | *..*)
                    printf 'kwin_variant=%s status=rejected-bad-name falling back to stock\n' \
                        "$kwin_variant" >> "$log_file"
                    kwin_variant=stock
                    ;;
                *)
                    kwin_anland_dir="/usr/local/lib/portal-ab/$ab_name"
                    ;;
            esac
            ;;
    esac
    if [ "$kwin_variant" != "stock" ]; then
        # Sanity: real file, plausible size, resolving soname chain. The
        # per-launch sync writes the library atomically (temp+rename), so a
        # complete file here is never partial; a failed check selects stock.
        if [ -r "$kwin_anland_dir/libkwin.so.6.3.6" ] \
            && [ "$(wc -c < "$kwin_anland_dir/libkwin.so.6.3.6" 2>/dev/null || echo 0)" -ge 1000000 ] \
            && [ -r "$kwin_anland_dir/libkwin.so.6" ]; then
            export LD_LIBRARY_PATH="$kwin_anland_dir${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
        else
            printf 'kwin_variant=%s status=incomplete-lib falling back to stock\n' \
                "$kwin_variant" >> "$log_file"
            kwin_variant=stock
        fi
    fi
    printf 'kwin_variant=%s anland_lib_dir=%s\n' \
        "$kwin_variant" "$kwin_anland_dir" >> "$log_file"
    # Exact binaries mapped for this run (post-mortem A/B attribution).
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum /usr/bin/kwin_wayland >> "$log_file" 2>/dev/null || true
        if [ "$kwin_variant" != "stock" ]; then
            sha256sum "$kwin_anland_dir/libkwin.so.6.3.6" >> "$log_file" 2>/dev/null || true
        else
            sha256sum /usr/lib/aarch64-linux-gnu/libkwin.so.6 >> "$log_file" 2>/dev/null || true
        fi
    fi
    # Project Anland unified libkwin (Anland backend + Portal Touchpad) is
    # served from its own dir so the QPainter overlay and distro libkwin are
    # never shadowed. It provides the AnlandBackend symbol itself, so the
    # load-time stub must never be preloaded here (it would win and trap).
    # Project Anland DRM shim (Anland only): the app sandbox cannot open
    # /dev/dri/renderD128, which KWin's Anland backend requires at init.
    # Never set in QPainter mode.
    if [ -r /usr/local/lib/portal/drmshim.so ]; then
        export LD_PRELOAD="/usr/local/lib/portal/drmshim.so${LD_PRELOAD:+:$LD_PRELOAD}"
    fi
fi
export QT_FORCE_STDERR_LOGGING=1
export QT_LOGGING_RULES="kwin_core.warning=true${QT_LOGGING_RULES:+;$QT_LOGGING_RULES}"

# Prefer the app-provisioned crash handler. It is an in-process signal handler,
# so it can still print PC/register/map evidence when nested gdb is denied by
# Android's PRoot tracer. The small custom handler is enabled whenever setup
# managed to compile it; it has no ptrace/startup debugger cost. libSegFault
# remains a debug-only fallback for older rootfs images that already ship it.
segfault_lib=""
if [ -r /usr/local/lib/localdesktop-crash-handler.so ]; then
    # The custom handler only installs signal hooks and is safe for normal
    # launches, so use it even when gdb=0. This keeps release startup free of
    # ptrace while retaining automatic crash-PC evidence when gcc was
    # available during setup.
    segfault_lib=/usr/local/lib/localdesktop-crash-handler.so
elif [ "${LOCALDESKTOP_GDB_BACKTRACE:-0}" = "1" ]; then
    for candidate in /usr/lib/libSegFault.so /lib/libSegFault.so; do
        if [ -r "$candidate" ]; then
            segfault_lib="$candidate"
            break
        fi
    done
fi
stack_capture=unavailable
if [ -n "$segfault_lib" ]; then
    if [[ "$segfault_lib" == */localdesktop-crash-handler.so ]]; then
        stack_capture=preload
    else
        stack_capture=libSegFault
    fi
fi
printf 'stack_capture=%s path=%s\n' "$stack_capture" "${segfault_lib:-unavailable}" >> "$log_file"

run_real_kwin() {
    if [ "$anland_mode" -eq 0 ]; then
        export LD_LIBRARY_PATH="/usr/local/lib/portal${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    elif [ "${kwin_variant:-unified}" != "stock" ] && [ -n "${kwin_anland_dir:-}" ] \
        && [ -r "$kwin_anland_dir/libkwin.so.6.3.6" ]; then
        case "$LD_LIBRARY_PATH" in
            "$kwin_anland_dir"*) ;;
            *) export LD_LIBRARY_PATH="$kwin_anland_dir${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ;;
        esac
    fi
    export QT_FORCE_STDERR_LOGGING=1
    if [ -n "$segfault_lib" ]; then
        export LD_PRELOAD="$segfault_lib${LD_PRELOAD:+:$LD_PRELOAD}"
        export SEGFAULT_SIGNALS=all
        export LOCALDESKTOP_CRASH_LOG="$trace_file"
    fi
    # Persist the real KWin stdout/stderr as well as forwarding it to the
    # session. This captures loader, protocol and signal-handler diagnostics
    # even when the guest process exits before a host frame exists.
    # Always disable KWin's internal guest screen locker; device locking belongs to Android.
    # In Anland mode force the Anland backend explicitly (env alone also
    # selects it) so the backend choice is unambiguous in the logs.
    if [ "$anland_mode" -eq 1 ]; then
        case " $* " in
            *" --anland "*) ;;
            *) set -- "$@" --anland ;;
        esac
    fi
    /usr/bin/kwin_wayland --no-lockscreen --inputmethod /usr/local/bin/portal-ime-bridge "$@" 2>&1 | tee -a "$log_file"
    return "${PIPESTATUS[0]}"
}

status=0
run_normally=1
if [ "${LOCALDESKTOP_GDB_BACKTRACE:-0}" = "1" ] && command -v gdb >/dev/null 2>&1; then
    if [ -n "$segfault_lib" ]; then
        LD_PRELOAD="$segfault_lib${LD_PRELOAD:+:$LD_PRELOAD}" \
            SEGFAULT_SIGNALS=all \
            LOCALDESKTOP_CRASH_LOG="$trace_file" \
            LOCALDESKTOP_ATTEMPT_ID="$attempt_id" \
            gdb --batch --quiet \
            -ex 'set pagination off' \
            -ex run \
            -ex 'thread apply all bt full' \
            --args /usr/bin/kwin_wayland --no-lockscreen --inputmethod /usr/local/bin/portal-ime-bridge "$@" \
            > "$debugger_output" 2>&1
    else
        gdb --batch --quiet \
            -ex 'set pagination off' \
            -ex run \
            -ex 'thread apply all bt full' \
            --args /usr/bin/kwin_wayland --no-lockscreen --inputmethod /usr/local/bin/portal-ime-bridge "$@" \
            > "$debugger_output" 2>&1
    fi
    gdb_status=$?
    cat "$debugger_output" >> "$trace_file" 2>/dev/null || true

    # A denied ptrace or an inability to launch gdb must never become a second
    # startup failure. Retry the real binary once in the normal path. This
    # denial check is independent of signal parsing: gdb can print both a
    # signal transcript and a ptrace error on different Android builds.
    if grep -Eqi 'ptrace|operation not permitted|permission denied|no such file|cannot execute|could not start|during startup program exited' "$debugger_output"; then
        run_normally=1
    elif [ "$gdb_status" -ne 0 ] && ! grep -Eqi 'Program received signal|exited normally|exited with code' "$debugger_output"; then
        # Unknown debugger startup failures are also not evidence that KWin
        # ran. Preserve the transcript, then execute the real binary once so
        # the session still gets a meaningful exit/crash marker.
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
    run_real_kwin "$@"
    status=$?
fi

# The child has closed its descriptors now, so trimming cannot strand a live
# writer on an unlinked inode. Keep the tail available to both the recovery UI
# and bounded diagnostics exports.
trim_log "$log_file"
trim_log "$trace_file"
trim_log "$debugger_output"

if [ "$status" -ge 128 ]; then
    printf 'timestamp_ms=%s attempt=%s status=%s pid=%s args=%q\n' \
        "$(date +%s%3N 2>/dev/null || date +%s000)" "$attempt_id" "$status" "$$" "$*" \
        > "$crash_file"
    core=""
    for core_dir in "$state_dir" "${KWIN_CORE_DIR:-$PWD}" /tmp; do
        [ -d "$core_dir" ] || continue
        core=$(find "$core_dir" -maxdepth 1 -type f -name 'core*' -print -quit 2>/dev/null || true)
        [ -n "$core" ] && break
    done
    # Always leave a concise result even when a denied gdb attempt already
    # populated the trace. This makes the normal-run fallback and signal
    # visible instead of allowing the gdb denial text to hide it.
    printf 'crash-summary timestamp_ms=%s attempt=%s status=%s signal=%s core=%s\n' \
        "$(date +%s%3N 2>/dev/null || date +%s000)" "$attempt_id" "$status" \
        "$((status - 128))" "${core:-unavailable}" >> "$trace_file"
    if [ -n "$core" ] && command -v gdb >/dev/null 2>&1; then
        gdb --batch --quiet /usr/bin/kwin_wayland "$core" \
            -ex 'set pagination off' -ex 'thread apply all bt full' \
            >> "$trace_file" 2>&1 || true
    fi
    if [ -z "$core" ] || ! command -v gdb >/dev/null 2>&1; then
        printf 'best-effort: gdb/coredump unavailable; signal=%s core=%s\n' \
            "$((status - 128))" "${core:-unavailable}" >> "$trace_file"
    fi
fi
exit "$status"
