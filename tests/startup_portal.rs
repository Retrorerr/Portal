const PLASMA_LAUNCHER: &str = include_str!("../assets/localdesktop-startplasma.sh");
const SETUP: &str = include_str!("../src/android/proot/setup.rs");
const ROOTFS: &str = include_str!("../scripts/build_debian_rootfs.py");

#[test]
fn desktop_portal_is_provisioned_before_plasma_and_not_globally_disabled() {
    for package in ["xdg-desktop-portal", "xdg-desktop-portal-kde", "xdg-desktop-portal-gtk"] {
        assert!(ROOTFS.contains(package));
        assert!(SETUP.contains(package));
    }
    assert!(SETUP.contains("ensure_desktop_services(Path::new(PRODUCTION_FS_ROOT))?"));
    assert!(PLASMA_LAUNCHER.contains("dbus-run-session -- /usr/bin/startplasma-wayland"));
    for disabled in ["QT_NO_XDG_DESKTOP_PORTAL=1", "KDE_NO_PORTAL=1", "GTK_USE_PORTAL=0"] {
        assert!(!PLASMA_LAUNCHER.contains(disabled));
    }
}

#[test]
fn guest_pipewire_clients_cannot_clamp_rttime_through_the_realtime_portal() {
    const NO_RT: &str = include_str!("../assets/localdesktop-pipewire-client-no-rt.conf");
    // module-rt clamps RLIMIT_RTTIME to the RTKit-less portal's 0 and the
    // kernel then SIGKILLs plasmashell; without D-Bus it never reaches that path.
    assert!(NO_RT.contains("support.dbus = false"));
    // Disabling only rtportal/rtkit makes PipeWire 1.4 abort KWin in libdbus.
    assert!(!NO_RT.contains("rtportal.enabled"));
    assert!(!NO_RT.contains("rtkit.enabled"));
    for path in ["etc/pipewire/client.conf.d/", "etc/pipewire/client-rt.conf.d/"] {
        assert!(SETUP.contains(path));
    }
    assert!(SETUP.contains("PIPEWIRE_CLIENT_NO_RT_PATHS[0],"));
}
