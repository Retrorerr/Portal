// On Wayland rofi stays open when another window takes focus, and Esc then goes to that window.
// Close rofi whenever a different window becomes active (e.g. you click an app behind it).
// KWin can't close a layer-shell surface itself, so this triggers a close-only command
// (rice-rofi-close.desktop → pkill -x rofi) through kglobalaccel; it never opens anything.
function isRofi(w) { return w && w.resourceClass === "rofi"; }

workspace.windowActivated.connect(function (active) {
    if (!active || isRofi(active)) return;
    const all = workspace.stackingOrder;
    for (let i = 0; i < all.length; ++i) {
        if (isRofi(all[i])) {
            callDBus("org.kde.kglobalaccel", "/component/rice_rofi_close_desktop",
                     "org.kde.kglobalaccel.Component", "invokeShortcut", "_launch");
            return;
        }
    }
});
