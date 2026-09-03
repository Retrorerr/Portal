#!/bin/bash
# Diagnostic-only temporary wrapper. The capture script stages this at
# /usr/local/bin/kwin_wayland, preserves the existing wrapper at the private
# copy path, and restores both paths after the run.
set -eu

probe_source=/usr/local/lib/localdesktop-kwin-stack-probe.c
probe_library=/usr/local/lib/localdesktop-kwin-stack-probe.so
probe_tmp=/usr/local/lib/localdesktop-kwin-stack-probe.so.tmp
probe_log=/var/lib/localdesktop/kwin-stack.log
original_wrapper=/usr/local/lib/localdesktop-kwin-wrapper.before
export LOCALDESKTOP_KWIN_STACK_NONCE="__LOCALDESKTOP_KWIN_STACK_NONCE__"

if [ ! -r "$probe_library" ] && [ -r "$probe_source" ] && command -v gcc >/dev/null 2>&1; then
    gcc -shared -fPIC -fno-omit-frame-pointer -O0 -g -Wall -Wextra \
        -o "$probe_tmp" "$probe_source" -ldl \
        && chmod 0755 "$probe_tmp" \
        && mv -f "$probe_tmp" "$probe_library"
fi

if [ ! -r "$probe_library" ] || [ ! -x "$original_wrapper" ]; then
    printf 'kwin-stack-probe unavailable library=%s wrapper=%s\n' \
        "$probe_library" "$original_wrapper" >&2
    exit 127
fi

export LOCALDESKTOP_KWIN_STACK_LOG="$probe_log"
export LD_PRELOAD="$probe_library${LD_PRELOAD:+:$LD_PRELOAD}"
exec "$original_wrapper" "$@"
