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
    # Anland sessions. Values: "unified" (default: Portal overlay libcarrying
    # the Anland backend plus the Portal Touchpad input path — PixelDelta
    # finger scrolling, axis-stop, NaturalScroll/ScrollFactor, kcminputrc
    # persistence; physically validated), "stock" (exact untouched distro
    # lfdevs binaries; kept as the A/B and recovery option), or "ab:<name>"
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
    # Project Anland XWayland variant selection (Phase B A/B without APK
    # rebuilds): explicit selection only. files/xwayland-variant arrives as
    # LOCALDESKTOP_XWAYLAND_VARIANT ("candidate" or "stock");
    # /var/lib/localdesktop/xwayland-variant is the out-of-band test
    # override (same pattern as kwin-variant above). Default MUST remain
    # stock: anything unrecognized, and any failed staging check, falls back
    # to stock and is logged. Stock keeps today's exact PATH/environment:
    # no wrapper binary, no extra processes, no behavior change.
    xwayland_variant=stock
    case "${LOCALDESKTOP_XWAYLAND_VARIANT:-}" in
        candidate|stock) xwayland_variant="$LOCALDESKTOP_XWAYLAND_VARIANT" ;;
    esac
    if [ -r /var/lib/localdesktop/xwayland-variant ]; then
        read -r xwayland_override < /var/lib/localdesktop/xwayland-variant
        case "$xwayland_override" in
            candidate|stock) xwayland_variant="$xwayland_override" ;;
            *)
                printf 'xwayland_variant override=%q rejected (want candidate|stock), keeping %s\n' \
                    "$xwayland_override" "$xwayland_variant" >> "$log_file"
                ;;
        esac
    fi
    xwayland_bin=/usr/bin/Xwayland
    if [ "$xwayland_variant" = "candidate" ]; then
        # Staging gate (single decision per launch, no retry loop): readable
        # + plausible size + exact pinned SHA. SHA equality also pins ELF
        # architecture and every staged byte. Any failure selects stock.
        xwayland_cand_dir=/usr/local/lib/portal-xwayland
        xwayland_cand_bin="$xwayland_cand_dir/Xwayland"
        xwayland_cand_sha_expected="b91f55794942a9efd66300e352cea8c671cdb6ff0c3243a0cc176525cb96812d"
        if [ -x "$xwayland_cand_bin" ] \
            && [ "$(wc -c < "$xwayland_cand_bin" 2>/dev/null || echo 0)" -ge 1000000 ]; then
            xwayland_cand_sha=$(sha256sum "$xwayland_cand_bin" 2>/dev/null | cut -d' ' -f1)
            if [ "$xwayland_cand_sha" = "$xwayland_cand_sha_expected" ]; then
                # KWin locates Xwayland via PATH: prepending the Portal dir
                # selects the candidate with no wrapper and no stock impact
                # (stock never touches PATH here).
                PATH="$xwayland_cand_dir:$PATH"
                export PATH
                xwayland_bin="$xwayland_cand_bin"
                printf 'xwayland_variant=candidate status=active sha=%s\n' \
                    "$xwayland_cand_sha" >> "$log_file"
            else
                printf 'xwayland_variant=candidate status=rejected-bad-sha falling back to stock\n' \
                    >> "$log_file"
                xwayland_variant=stock
            fi
        else
            printf 'xwayland_variant=candidate status=incomplete-staging falling back to stock\n' \
                >> "$log_file"
            xwayland_variant=stock
        fi
    fi
    printf 'xwayland_variant=%s bin=%s resolved=%s\n' \
        "$xwayland_variant" "$xwayland_bin" "$(command -v Xwayland)" >> "$log_file"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum /usr/bin/Xwayland >> "$log_file" 2>/dev/null || true
        if [ "$xwayland_variant" = "candidate" ]; then
            sha256sum "$xwayland_cand_bin" >> "$log_file" 2>/dev/null || true
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
    # Renderer mode (default: hardware accelerated). The emergency software
    # fallback is explicit: /var/lib/localdesktop/kwin-glmode containing
    # "sw" forces KWin surfaceless software rendering (ANLAND_NO_DRM_DEVICE)
    # and drops the freedreno gallium forcing (inside the software EGL stack
    # it demands the KGSL winsys and kills EGL init; proven by matrix).
    # Anything else (including absent) is the production GPU path: make sure
    # no stale NO_DRM_DEVICE leaks in and keep GALLIUM_DRIVER=freedreno so
    # Mesa selects the kgsl winsys for the KGSL-backed render node.
    if [ "$(cat /var/lib/localdesktop/kwin-glmode 2>/dev/null || echo hw)" = "sw" ]; then
        export ANLAND_NO_DRM_DEVICE=1
        unset GALLIUM_DRIVER
        printf 'anland GL mode=sw (emergency software fallback): NO_DRM_DEVICE=1, GALLIUM_DRIVER unset\n' >> "$log_file"
    else
        unset ANLAND_NO_DRM_DEVICE
        printf 'anland GL mode=hw (accelerated default): NO_DRM_DEVICE unset, GALLIUM_DRIVER=%s\n' "${GALLIUM_DRIVER:-unset}" >> "$log_file"
    fi
    if [ -n "$segfault_lib" ]; then
        export LD_PRELOAD="$segfault_lib${LD_PRELOAD:+:$LD_PRELOAD}"
        export SEGFAULT_SIGNALS=all
        export LOCALDESKTOP_CRASH_LOG="$trace_file"
    fi
    # Persist the real KWin stdout/stderr as well as forwarding it to the
    # session. This captures loader, protocol and signal-handler diagnostics
    # even when the guest process exits before a host frame exists.
    # Always disable KWin's internal guest screen locker; device locking belongs to Android.
    # In Anland mode prefer the Anland backend explicitly, but ONLY when the
    # installed kwin_wayland advertises --anland: the lfdevs Anland build
    # accepts it, while stock distro kwin_wayland exits(1) on unknown
    # options, wedging the session in a compositor restart loop. Env
    # (ANLAND_SOCKET) plus the unified libkwin selects the backend
    # otherwise, so probing keeps both binaries working.
    if [ "$anland_mode" -eq 1 ]; then
        case " $* " in
            *" --anland "*) ;;
            *)
                if /usr/bin/kwin_wayland --help 2>/dev/null | grep -qF -- '--anland'; then
                    set -- "$@" --anland
                else
                    printf 'anland backend via env only (binary lacks --anland)\n' >> "$log_file"
                fi
                ;;
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
