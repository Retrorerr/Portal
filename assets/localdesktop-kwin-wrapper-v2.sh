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
    LOCALDESKTOP_ATTEMPT_ID PORTAL_GRAPHICS_TRACE; do
    eval "value=\${$name-}"
    printf 'env %s=%q\n' "$name" "$value" >> "$log_file"
done
ulimit -c unlimited 2>/dev/null || true
# Portal has one graphics path: Android Surface -> Anland -> KWin/Plasma.
# The Forky Anland-capable KWin binary and its matching libkwin are staged by
# setup in this private directory. XWayland stays the Debian package for
# applications that genuinely need X11; no Firefox or compositor A/B variant
# is selected here.
anland_mode=1
kwin_anland_dir=/usr/local/lib/portal-anland
kwin_bin="$kwin_anland_dir/kwin_wayland"
kwin_screencast_plugin="$kwin_anland_dir/kwin/plugins/screencast.so"
printf 'anland_mode=%s socket=%s kwin=%s\n' \
    "$anland_mode" "${ANLAND_SOCKET:-unset}" "$kwin_bin" >> "$log_file"
if [ ! -x "$kwin_bin" ] || [ ! -r "$kwin_anland_dir/libkwin.so.6.7.4" ] || \
    [ ! -r "$kwin_screencast_plugin" ]; then
    printf 'anland KWin assets are incomplete\n' >> "$log_file"
    exit 127
fi
export LD_LIBRARY_PATH="$kwin_anland_dir${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
# The Anland KWin build owns the matching screencast plugin. Put its private
# Qt plugin root first so KPluginMetaData::findPlugins("kwin/plugins") cannot
# load Debian's unpatched screencast.so, while retaining distro plugin roots
# for every other KWin module.
export QT_PLUGIN_PATH="$kwin_anland_dir${QT_PLUGIN_PATH:+:$QT_PLUGIN_PATH}"
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$kwin_bin" "$kwin_anland_dir/libkwin.so.6.7.4" \
        "$kwin_screencast_plugin" >> "$log_file" 2>/dev/null || true
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

# Phase B XI2 proof harness (inert unless explicitly flagged): when
# /var/lib/localdesktop/xwayland-xinput-probe exists, a bounded background
# job enumerates the XWayland XI2 devices (list + per-device properties)
# into xwayland-xinput.log, and with content "capture:<seconds>" also dumps
# raw XI2 events from the touchpad-classified device while the physical
# touchpad is exercised. Stock sessions without the flag file behave
# exactly as before (no extra processes, no environment change).
portal_xinput_probe() {
    local mode="$1"
    shift
    local xinput_bin=/usr/local/lib/portal-xwayland/xinput
    local out="$state_dir/xwayland-xinput.log"
    printf 'xinput probe start mode=%s time=%s variant=%s\n' \
        "$mode" "$(date +%s)" "${xwayland_variant:-stock}" >> "$out"
    if [ ! -x "$xinput_bin" ]; then
        printf 'xinput probe: tool missing at %s\n' "$xinput_bin" >> "$out"
        return 0
    fi
    # KWin launches its Xwayland with --xwayland-display/--xwayland-xauthority
    # in our own argv: prefer those over scanning (the cookie suffix is
    # random per launch). Fall back to the socket scan when absent.
    local display=":0" xauth_file="" prev=""
    for arg in "$@"; do
        case "$arg" in
            --xwayland-xauthority=*) xauth_file=${arg#*=} ;;
            --xwayland-display=*) display=${arg#*=} ;;
            *)
                if [ "$prev" = "--xwayland-xauthority" ]; then xauth_file="$arg"; fi
                if [ "$prev" = "--xwayland-display" ]; then display="$arg"; fi
                ;;
        esac
        prev="$arg"
    done
    if [ -n "$xauth_file" ] && [ -r "$xauth_file" ]; then
        export XAUTHORITY="$xauth_file"
        printf 'xinput probe: using xauthority from argv\n' >> "$out"
    else
        printf 'xinput probe: no readable xauthority in argv (file=%s)\n' \
            "${xauth_file:-none}" >> "$out"
    fi
    export DISPLAY="$display"
    if ! "$xinput_bin" list >> "$out" 2>&1; then
        printf 'xinput probe: argv display %s failed, falling back to socket scan\n' \
            "$display" >> "$out"
        display=""
    fi
    # Bounded fallback wait for an XWayland socket that answers XI2 queries.
    local attempt=0
    while [ -z "$display" ] && [ "$attempt" -lt 45 ]; do
        for sock in /tmp/.X11-unix/X[0-9]; do
            [ -S "$sock" ] || continue
            dnum=${sock##*X}
            if DISPLAY=":$dnum" "$xinput_bin" list >> "$out" 2>&1; then
                display=":$dnum"
                break 2
            fi
            printf 'xinput probe: display :%s not answering (%s)\n' \
                "$dnum" "$(DISPLAY=":$dnum" "$xinput_bin" list 2>&1 | head -n 1)" >> "$out"
        done
        sleep 2
        attempt=$((attempt + 1))
    done
    if [ -z "$display" ]; then
        printf 'xinput probe: no X display answered within bounds\n' >> "$out"
        return 0
    fi
    printf 'xinput probe: display=%s\n' "$display" >> "$out"
    export DISPLAY="$display"
    "$xinput_bin" list --long >> "$out" 2>&1
    # Per-device properties: the classification proof (which device, if any,
    # exposes libinput Tapping Enabled, and with what value).
    for id in $("$xinput_bin" list --id-only 2>/dev/null); do
        printf -- '--- props for id %s ---\n' "$id" >> "$out"
        "$xinput_bin" list-props "$id" >> "$out" 2>&1
    done
    case "$mode" in
        capture:*)
            secs=${mode#capture:}
            case "$secs" in
                ''|*[!0-9]*) secs=30 ;;
            esac
            [ "$secs" -ge 5 ] || secs=5
            [ "$secs" -le 120 ] || secs=120
            # Resolve the touchpad-classified device by its property, not by
            # name: the device whose props carry libinput Tapping Enabled.
            touch_id=""
            for id in $("$xinput_bin" list --id-only 2>/dev/null); do
                if "$xinput_bin" list-props "$id" 2>/dev/null | grep -q 'libinput Tapping Enabled'; then
                    touch_id="$id"
                    break
                fi
            done
            if [ -z "$touch_id" ]; then
                printf 'xinput probe: no touchpad-classified device for capture\n' >> "$out"
                return 0
            fi
            printf 'xinput probe: capturing XI2 events from id %s for %ss (exercise the touchpad now)\n' \
                "$touch_id" "$secs" >> "$out"
            if command -v timeout >/dev/null 2>&1; then
                timeout "$secs" "$xinput_bin" test-xi2 --root "$touch_id" >> "$out" 2>&1 || true
            else
                "$xinput_bin" test-xi2 --root "$touch_id" >> "$out" 2>&1 &
                cap_pid=$!
                sleep "$secs"
                kill "$cap_pid" 2>/dev/null || true
                wait "$cap_pid" 2>/dev/null || true
            fi
            printf 'xinput probe: capture done\n' >> "$out"
            ;;
    esac
    return 0
}

run_real_kwin() {
    # KWin keeps its local HW copy (same values clients now also inherit
    # guest-wide via guest_mesa_env). KWin-only flags stay here: ANLAND*,
    # ANLAND_NO_DRM_DEVICE, ANLAND_SKIP_IMPLICIT_SYNC_WAIT, and
    # XWAYLAND_FORCE_KGSL_SURFACELESS (XWayland is a KWin child, so it
    # inherits this). XWayland is
    # a Debian package child of KWin; the Forky 2:24.1.13-1portal1 build
    # carries the KGSL surfaceless forward-port, so export its force flag
    # here (hw only) so KWin-launched XWayland reaches the KGSL path.
    gl_mode=hw
    if [ "$(cat /var/lib/localdesktop/kwin-glmode 2>/dev/null || true)" = "sw" ]; then
        gl_mode=sw
        export MESA_LOADER_DRIVER_OVERRIDE=llvmpipe
        export GALLIUM_DRIVER=llvmpipe
        unset FD_FORCE_KGSL FD_KGSL_ENABLE_DMABUF TURNIP_KMD
        unset ANLAND_SKIP_IMPLICIT_SYNC_WAIT
        unset XWAYLAND_FORCE_KGSL_SURFACELESS
    else
        export MESA_LOADER_DRIVER_OVERRIDE=kgsl
        export GALLIUM_DRIVER=freedreno
        export FD_FORCE_KGSL=1
        export FD_KGSL_ENABLE_DMABUF=1
        export TURNIP_KMD=kgsl
        export ANLAND_SKIP_IMPLICIT_SYNC_WAIT=1
        export XWAYLAND_FORCE_KGSL_SURFACELESS=1
    fi
    export ANLAND_SOCKET="${ANLAND_SOCKET:-/tmp/anland/display.sock}"
    export ANLAND=1
    export ANLAND_DISABLE_AUDIO=1
    if [ "${LOCALDESKTOP_KWIN_GL_DEBUG:-0}" = "1" ]; then
        export KWIN_GL_DEBUG=1
    else
        unset KWIN_GL_DEBUG
    fi
    printf 'kwin_env mode=%s mesa_override=%s gallium=%s fd_force=%s dmabuf=%s turnip=%s xwayland_force=%s\n' \
        "$gl_mode" "$MESA_LOADER_DRIVER_OVERRIDE" "$GALLIUM_DRIVER" \
        "${FD_FORCE_KGSL:-unset}" "${FD_KGSL_ENABLE_DMABUF:-unset}" \
        "${TURNIP_KMD:-unset}" "${XWAYLAND_FORCE_KGSL_SURFACELESS:-unset}" >> "$log_file"
    printf 'kwin_debug=%s\n' "${KWIN_GL_DEBUG:-unset}" >> "$log_file"
    export LD_LIBRARY_PATH="$kwin_anland_dir${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    export QT_FORCE_STDERR_LOGGING=1
    # Portal's app UID cannot open Android's /dev/dri node. Hardware mode
    # uses KGSL-backed surfaceless EGL while Anland owns Android dmabuf
    # presentation. The explicit sw mode is troubleshooting-only.
    export ANLAND_NO_DRM_DEVICE=1
    printf 'anland GL mode=%s (%s; Anland dmabuf path)\n' "$gl_mode" \
        "$( [ "$gl_mode" = sw ] && printf 'llvmpipe software EGL' || printf 'KGSL surfaceless EGL' )" >> "$log_file"
    if [ -n "$segfault_lib" ]; then
        export LD_PRELOAD="$segfault_lib${LD_PRELOAD:+:$LD_PRELOAD}"
        export SEGFAULT_SIGNALS=all
        export LOCALDESKTOP_CRASH_LOG="$trace_file"
    fi
    # Persist the real KWin stdout/stderr as well as forwarding it to the
    # session. This captures loader, protocol and signal-handler diagnostics
    # even when the guest process exits before a host frame exists.
    # Always disable KWin's internal guest screen locker; device locking belongs to Android.
    case " $* " in
        *" --anland "*) ;;
        *) set -- "$@" --anland ;;
    esac
    "$kwin_bin" --no-lockscreen --inputmethod /usr/local/bin/portal-ime-bridge "$@" 2>&1 | tee -a "$log_file"
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
            --args "$kwin_bin" --no-lockscreen --inputmethod /usr/local/bin/portal-ime-bridge "$@" \
            > "$debugger_output" 2>&1
    else
        gdb --batch --quiet \
            -ex 'set pagination off' \
            -ex run \
            -ex 'thread apply all bt full' \
            --args "$kwin_bin" --no-lockscreen --inputmethod /usr/local/bin/portal-ime-bridge "$@" \
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
    # Phase B probe trigger (flag-gated only; stock without the flag file is
    # untouched). Runs concurrently with KWin; bounded waits guarantee it
    # can never wedge session startup or shutdown.
    if [ -r /var/lib/localdesktop/xwayland-xinput-probe ]; then
        read -r xinput_probe_mode < /var/lib/localdesktop/xwayland-xinput-probe
        case "$xinput_probe_mode" in
            list|capture:*) portal_xinput_probe "$xinput_probe_mode" "$@" & ;;
            *)
                printf 'xinput probe mode=%q rejected (want list|capture:<secs>)\n' \
                    "$xinput_probe_mode" >> "$log_file"
                ;;
        esac
    fi
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
        gdb --batch --quiet "$kwin_bin" "$core" \
            -ex 'set pagination off' -ex 'thread apply all bt full' \
            >> "$trace_file" 2>&1 || true
    fi
    if [ -z "$core" ] || ! command -v gdb >/dev/null 2>&1; then
        printf 'best-effort: gdb/coredump unavailable; signal=%s core=%s\n' \
            "$((status - 128))" "${core:-unavailable}" >> "$trace_file"
    fi
fi
exit "$status"
