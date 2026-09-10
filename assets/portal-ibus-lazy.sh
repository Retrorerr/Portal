#!/bin/bash
# Portal IBus lazy starter for X11/GTK clients (Firefox URL bar, inputs,
# textareas). The Portal IBus engine bridges editable focus and host commits
# to the Android IME.
#
# This launcher MUST NEVER sit in the critical splash->desktop startup path:
# it returns in milliseconds and does all real work in a detached background
# task with bounded waits. There is deliberately:
#   * no package installation here (IBus packages are provisioned pre-session
#     by setup; a missing daemon simply means X11/GTK input degrades to
#     evdev keys),
#   * no fixed sleep blocking startup (engine selection polls with a bound
#     instead, so a fast daemon is used immediately and a broken one can
#     never stall startup).
(
    # Session bus may not exist yet when autostart fires; wait bounded.
    for _ in $(seq 1 60); do
        if [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
            break
        fi
        sleep 1
    done
    command -v ibus-daemon >/dev/null 2>&1 || exit 0
    ibus-daemon -s -d >>/tmp/portal-ibus-daemon.log 2>&1 || exit 0
    # The daemon needs a moment to own org.freedesktop.IBus; poll with a
    # bound so a fast daemon selects the engine immediately and a broken
    # one can never stall startup.
    for _ in $(seq 1 30); do
        if ibus engine portal >>/tmp/portal-ibus-daemon.log 2>&1; then
            break
        fi
        sleep 1
    done
) >/dev/null 2>&1 < /dev/null &
exit 0
