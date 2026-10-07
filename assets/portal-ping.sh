#!/bin/sh
# Managed by Portal: iputils ping inside Android's app sandbox. Android locks
# the securebits ping sets before dropping capabilities, so it needs a small
# preload to start (assets/guest-arm64/ping-keepcaps.c in Portal's sources).
if [ ! -x /usr/bin/ping ]; then
    echo "ping: not installed (sudo apt install iputils-ping)" >&2
    exit 127
fi
LD_PRELOAD="/usr/local/lib/portal/ping-keepcaps.so${LD_PRELOAD:+:$LD_PRELOAD}" exec /usr/bin/ping "$@"
