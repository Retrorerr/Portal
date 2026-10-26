# Sourced by the guest session launcher before starting Plasma. Qt 6.7+
# understands an ordered list: its Wayland backend handles "wayland"
# internally, while XCB skips that unavailable plugin and selects IBus.
# Qt 5 ignores the list and uses IBus, including when an app chooses XCB
# explicitly (VLC 3). GTK keeps its own per-display native/IBus selection.
# Never replace an explicit user choice, including an intentionally empty one.
if [ "${QT_IM_MODULE+x}" != x ] && [ "${QT_IM_MODULES+x}" != x ]; then
    export QT_IM_MODULE=ibus
    export QT_IM_MODULES='wayland;ibus'
fi
