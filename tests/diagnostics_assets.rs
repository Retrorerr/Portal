const KWIN_WRAPPER: &str = include_str!("../assets/localdesktop-kwin-wrapper-v2.sh");
const PLASMA_LAUNCHER: &str = include_str!("../assets/localdesktop-startplasma.sh");
const RECOVERY: &str = include_str!("../assets/localdesktop-recovery.sh");
const SETUP_PAGE: &str = include_str!("../assets/setup-progress-v2.html");
const ERROR_PAGE: &str = include_str!("../assets/runtime-error.html");

#[test]
fn kwin_wrapper_does_not_make_gdb_a_release_requirement() {
    assert!(KWIN_WRAPPER.contains("LOCALDESKTOP_GDB_BACKTRACE:-0"));
    assert!(KWIN_WRAPPER.contains("run_normally=1"));
    assert!(KWIN_WRAPPER.contains("ptrace"));
    assert!(KWIN_WRAPPER.contains("status=139"));
}

#[test]
fn plasma_launcher_waits_for_host_presented_marker() {
    assert!(PLASMA_LAUNCHER.contains("plasma-ready"));
    assert!(PLASMA_LAUNCHER.contains("dbus-run-session -- /usr/bin/startplasma-wayland"));
    assert!(PLASMA_LAUNCHER.contains("KDE_USE_SYSTEMD=0"));
    assert!(!PLASMA_LAUNCHER.contains("pgrep plasmashell"));
}

#[test]
fn recovery_is_graphical_and_never_autostarts_a_terminal() {
    assert!(RECOVERY.contains("kdialog"));
    assert!(!RECOVERY.contains("konsole"));
}

#[test]
fn setup_and_error_pages_offer_one_tap_export() {
    assert!(SETUP_PAGE.contains("Export diagnostics"));
    assert!(SETUP_PAGE.contains("export_diagnostics"));
    assert!(ERROR_PAGE.contains("Export diagnostics"));
    assert!(ERROR_PAGE.contains("export_diagnostics"));
}
