#!/bin/bash
# Send one inner-Plasma Wayland clipboard selection to the Android broker.
#
# This helper is intentionally one-shot: wl-paste --watch starts it for each
# new selection.  The payload is copied to an owner-only temporary file,
# bounded before base64 encoding, and then sent over the authenticated
# loopback protocol.  Clipboard contents and the session token are never
# written to logs.
set -euo pipefail

readonly MAX_TEXT_BYTES=4194304

fail() {
    printf 'Portal clipboard bridge: %s\n' "$1" >&2
    exit 1
}

host=${LOCALDESKTOP_CLIPBOARD_HOST:-127.0.0.1}
port=${LOCALDESKTOP_CLIPBOARD_PORT:-}
token=${LOCALDESKTOP_CLIPBOARD_TOKEN:-}
initial_gate=${LOCALDESKTOP_CLIPBOARD_INITIAL_GATE:-}

[[ "$host" =~ ^[A-Za-z0-9._:-]+$ ]] || fail 'invalid broker host'
[[ "$port" =~ ^[0-9]{1,5}$ ]] || fail 'invalid broker port'
port_number=$((10#$port))
((port_number >= 1 && port_number <= 65535)) || fail 'broker port is out of range'
[[ "$token" =~ ^[0-9a-fA-F]{64}$ ]] || fail 'invalid broker token'

# `wl-paste --watch` immediately reports the selection that predates this
# Portal session. Consume the gate once and do not turn that observation into
# a fresh Android `setPrimaryClip` call.
if [[ -n "$initial_gate" ]] && rmdir "$initial_gate" 2>/dev/null; then
    exit 0
fi

tmp_root=${TMPDIR:-/tmp}/localdesktop-clipboard
mkdir -p "$tmp_root"
chmod 700 "$tmp_root"
tmp_file="$tmp_root/push.$$"
cleanup() {
    rm -f -- "$tmp_file"
}
trap cleanup EXIT
trap 'exit 143' HUP INT TERM

# Reading one byte past the policy limit lets the host reject an oversized
# selection without ever allocating an unbounded buffer.  An empty selection
# is represented by the explicit CLEAR command and is not a PUSH.
head -c "$((MAX_TEXT_BYTES + 1))" > "$tmp_file" || fail 'failed to stage clipboard data'
byte_count=$(wc -c < "$tmp_file")
byte_count=${byte_count//[[:space:]]/}
[[ "$byte_count" =~ ^[0-9]+$ ]] || fail 'could not measure clipboard data'
((byte_count <= MAX_TEXT_BYTES)) || fail 'clipboard selection exceeds 4 MiB'

exec 3<>"/dev/tcp/$host/$port_number" || fail 'could not connect to clipboard broker'
printf 'LDCL/1 HELLO %s\n' "$token" >&3
IFS= read -r -t 5 reply <&3 || fail 'broker handshake timed out'
[[ "$reply" == 'ACK HELLO' ]] || fail 'broker rejected handshake'

if ((byte_count == 0)); then
    printf 'LDCL/1 CLEAR\n' >&3
    IFS= read -r -t 5 reply <&3 || fail 'broker did not acknowledge clear'
    [[ "$reply" == 'ACK CLEAR' ]] || fail 'broker rejected clear'
    exec 3>&-
    exit 0
fi

{
    printf 'LDCL/1 PUSH %s\n' "$byte_count"
    # GNU coreutils (used by Arch) supports -w 0.  The fallback also strips
    # wrapping newlines for implementations with a different base64 CLI.
    if ! base64 -w 0 "$tmp_file" 2>/dev/null; then
        base64 "$tmp_file" | tr -d '\n'
    fi
    printf '\n'
} >&3
IFS= read -r -t 5 reply <&3 || fail 'broker did not acknowledge push'
[[ "$reply" == 'ACK PUSH' ]] || fail 'broker rejected push'
exec 3>&-
