const PLASMA_LAUNCHER: &str = include_str!("../assets/localdesktop-startplasma.sh");

#[test]
fn plasma_launcher_disables_qt_portal_probe_before_session_start() {
    let portal_setting = "export QT_NO_XDG_DESKTOP_PORTAL=1";
    let session_start = "dbus-run-session -- /usr/bin/startplasma-wayland";

    assert!(PLASMA_LAUNCHER.contains(portal_setting));
    assert!(PLASMA_LAUNCHER.contains("export KDE_NO_PORTAL=1"));
    assert!(PLASMA_LAUNCHER.contains("export GTK_USE_PORTAL=0"));
    assert!(PLASMA_LAUNCHER.contains("env %s=%q"));
    assert!(PLASMA_LAUNCHER.find(portal_setting) < PLASMA_LAUNCHER.find(session_start));
}
