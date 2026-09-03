#!/bin/sh
# Diagnostic-only wrapper. It is staged temporarily by
# scripts/capture-kcminit-stack.ps1 and must be removed/restored afterward.
set -eu

probe_source=/usr/local/lib/localdesktop-kcminit-stack-probe.c
probe_library=/usr/local/lib/localdesktop-kcminit-stack-probe.so
probe_tmp=/usr/local/lib/localdesktop-kcminit-stack-probe.so.tmp
probe_log=/var/lib/localdesktop/kcminit-stack.log
export LOCALDESKTOP_KCMINIT_STACK_NONCE="__LOCALDESKTOP_KCMINIT_STACK_NONCE__"

if [ ! -r "$probe_library" ] && [ -r "$probe_source" ] && command -v gcc >/dev/null 2>&1; then
    gcc -shared -fPIC -fno-omit-frame-pointer -O0 -g -Wall -Wextra \
        -o "$probe_tmp" "$probe_source" -ldl \
        && chmod 0755 "$probe_tmp" \
        && mv -f "$probe_tmp" "$probe_library"
fi

if [ -r "$probe_library" ]; then
    export LOCALDESKTOP_KCMINIT_STACK_LOG="$probe_log"
    export LD_PRELOAD="$probe_library${LD_PRELOAD:+:$LD_PRELOAD}"
fi

exec /usr/bin/kcminit_startup "$@"
