#!/bin/bash
# Keep the inner KDE Plasma clipboard synchronized with Android.
#
# The Android process owns the broker; this guest-side worker owns the inner
# KWin selection.  It deliberately uses the ext-data-control-capable
# wl-clipboard 2.3+ package.  Core wl_data_device is focus dependent and
# cannot provide a reliable background watcher for Plasma's lock screen.
set -euo pipefail

readonly MAX_TEXT_BYTES=4194304
readonly PUSH_HELPER=${LOCALDESKTOP_CLIPBOARD_PUSH_HELPER:-/usr/local/bin/localdesktop-clipboard-push}

fail() {
    printf 'Portal clipboard bridge: %s\n' "$1" >&2
    exit 1
}

host=${LOCALDESKTOP_CLIPBOARD_HOST:-127.0.0.1}
port=${LOCALDESKTOP_CLIPBOARD_PORT:-}
token=${LOCALDESKTOP_CLIPBOARD_TOKEN:-}
[[ "$host" =~ ^[A-Za-z0-9._:-]+$ ]] || fail 'invalid broker host'
[[ "$port" =~ ^[0-9]{1,5}$ ]] || fail 'invalid broker port'
port_number=$((10#$port))
((port_number >= 1 && port_number <= 65535)) || fail 'broker port is out of range'
[[ "$token" =~ ^[0-9a-fA-F]{64}$ ]] || fail 'invalid broker token'

[[ -n "${WAYLAND_DISPLAY:-}" ]] || fail 'WAYLAND_DISPLAY is not set'
[[ -n "${XDG_RUNTIME_DIR:-}" ]] || fail 'XDG_RUNTIME_DIR is not set'
command -v wl-copy >/dev/null 2>&1 || fail 'wl-copy is not installed'
command -v wl-paste >/dev/null 2>&1 || fail 'wl-paste is not installed'
[[ -x "$PUSH_HELPER" ]] || fail 'clipboard push helper is not installed'

# wl-paste 2.3 is the first release with ext-data-control-v1 support.  KWin
# 6.7 advertises that standardized protocol and no longer guarantees the old
# wlroots data-control global, so accepting older binaries would silently
# fall back to a focus-stealing, non-working watcher.
version_line=$(wl-paste --version 2>/dev/null || true)
version_fields=$(printf '%s\n' "$version_line" | sed -nE 's/.*[^0-9]([0-9]+)\.([0-9]+)(\.[0-9]+)?.*/\1 \2/p' | head -n 1)
version_major=''
version_minor=''
read -r version_major version_minor <<< "$version_fields"
[[ "$version_major" =~ ^[0-9]+$ && "$version_minor" =~ ^[0-9]+$ ]] || fail 'could not determine wl-clipboard version'
if ((version_major < 2 || (version_major == 2 && version_minor < 3))); then
    fail 'wl-clipboard 2.3 or newer is required for KWin ext-data-control'
fi
wl-paste --help 2>&1 | grep -Fq -- '--watch' || fail 'wl-paste lacks clipboard watch support'

tmp_root=${TMPDIR:-/tmp}/localdesktop-clipboard
mkdir -p "$tmp_root"
chmod 700 "$tmp_root"
cleanup_pid=''
push_pid=''
initial_gate="$tmp_root/ignore-initial"
cleanup() {
    trap - EXIT HUP INT TERM
    if [[ -n "$push_pid" ]] && kill -0 "$push_pid" 2>/dev/null; then
        kill -TERM "$push_pid" 2>/dev/null || true
        wait "$push_pid" 2>/dev/null || true
    fi
    if [[ -n "$cleanup_pid" ]] && kill -0 "$cleanup_pid" 2>/dev/null; then
        kill -TERM "$cleanup_pid" 2>/dev/null || true
        wait "$cleanup_pid" 2>/dev/null || true
    fi
    rmdir "$initial_gate" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 143' HUP INT TERM

stop_injected_owner() {
    if [[ -n "$cleanup_pid" ]] && kill -0 "$cleanup_pid" 2>/dev/null; then
        kill -TERM "$cleanup_pid" 2>/dev/null || true
        wait "$cleanup_pid" 2>/dev/null || true
    fi
    cleanup_pid=''
}

run_guest_watcher() {
    # --watch is backed by ext-data-control-v1 in wl-clipboard 2.3+.  The
    # command receives the complete text on stdin; it does not use shell
    # command substitution, which would lose trailing newlines or NUL bytes.
    local watch_pid=''
    trap 'if [[ -n "$watch_pid" ]]; then kill -TERM "$watch_pid" 2>/dev/null || true; wait "$watch_pid" 2>/dev/null || true; fi; exit 0' TERM HUP INT
    while :; do
        LOCALDESKTOP_CLIPBOARD_INITIAL_GATE="$initial_gate" \
            wl-paste --type 'text/plain;charset=utf-8' --no-newline --watch "$PUSH_HELPER" &
        watch_pid=$!
        wait "$watch_pid" 2>/dev/null || true
        watch_pid=''
        sleep 1
    done
}

# wl-paste --watch invokes its command once for the selection that already
# exists when observation begins. That is session initialization, not a user
# copy. The one-shot directory gate lets the push helper discard it atomically.
rmdir "$initial_gate" 2>/dev/null || true
mkdir "$initial_gate"
run_guest_watcher &
push_pid=$!

while :; do
    # PRoot shares the loopback network namespace with the Android process;
    # the random token is the authorization boundary, not the address.
    if ! exec 3<>"/dev/tcp/$host/$port_number"; then
        sleep 1
        continue
    fi
    printf 'LDCL/1 HELLO %s\nLDCL/1 SUBSCRIBE\n' "$token" >&3 || true

    while IFS= read -r line <&3; do
        case "$line" in
            ACK\ *)
                # The handshake ACK is informational; the first VALUE/CLEAR
                # event is the broker's current snapshot.
                continue
                ;;
            CLEAR)
                stop_injected_owner
                wl-copy --clear >/dev/null 2>&1 || true
                ;;
            VALUE\ [0-9]*)
                byte_count=${line#VALUE }
                [[ "$byte_count" =~ ^[1-9][0-9]{0,6}$ ]] || break
                ((byte_count <= MAX_TEXT_BYTES)) || break
                encoded_count=$(( ((byte_count + 2) / 3) * 4 ))
                IFS= read -r encoded <&3 || break
                [[ "${#encoded}" -eq "$encoded_count" ]] || break
                decoded_file="$tmp_root/value.$$"
                rm -f -- "$decoded_file"
                if ! printf '%s\n' "$encoded" | base64 --decode > "$decoded_file" 2>/dev/null; then
                    rm -f -- "$decoded_file"
                    break
                fi
                actual_count=$(wc -c < "$decoded_file")
                actual_count=${actual_count//[[:space:]]/}
                if [[ "$actual_count" != "$byte_count" ]]; then
                    rm -f -- "$decoded_file"
                    break
                fi
                stop_injected_owner
                # Keep this source alive after the command returns so Plasma
                # clients can paste even while no app owns the selection.
                # Open the file in the parent before spawning: unlinking it
                # after '&' races the child's input redirection.
                exec 4< "$decoded_file"
                wl-copy --foreground --type 'text/plain;charset=utf-8' <&4 3>&- 4<&- &
                cleanup_pid=$!
                exec 4<&-
                rm -f -- "$decoded_file"
                ;;
            *)
                # Protocol errors cause a reconnect and are surfaced by the
                # Android-side diagnostics log without exposing text data.
                break
                ;;
        esac
    done
    exec 3>&- || true
    sleep 1
done
