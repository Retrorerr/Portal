use localdesktop::core::guest_compat::{install_command, PACKAGES};
use localdesktop::core::guest_compat::{
    legacy_libreoffice_launcher, library_matches, retire_libreoffice_launchers,
};

#[test]
fn native_plugins_are_seeded_and_upgraded_without_large_runtimes() {
    let rootfs = include_str!("../scripts/build_debian_rootfs.py");
    for package in PACKAGES {
        assert!(
            rootfs.contains(&format!("\"{package}\"")),
            "missing image seed: {package}"
        );
    }
    let command = install_command();
    assert!(command.contains("--no-remove --no-install-recommends"));
    assert!(!command.contains("upgrade"));
    for runtime in [
        "default-jre",
        "wine",
        "mono-runtime",
        "fex",
        "python3-pyside6",
    ] {
        assert!(!PACKAGES.contains(&runtime));
    }
}

#[test]
fn retirement_preserves_custom_launchers_and_retries_safely() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("usr/local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(
        bin.join("libreoffice"),
        legacy_libreoffice_launcher("libreoffice"),
    )
    .unwrap();
    let custom = format!(
        "{}# My own launch settings\n",
        legacy_libreoffice_launcher("soffice")
    );
    std::fs::write(bin.join("soffice"), &custom).unwrap();
    retire_libreoffice_launchers(root.path()).unwrap();
    retire_libreoffice_launchers(root.path()).unwrap();
    assert!(!bin.join("libreoffice").exists());
    assert_eq!(
        std::fs::read_to_string(bin.join("soffice")).unwrap(),
        custom
    );
}

#[test]
fn overlay_never_replaces_changed_or_absent_libraries() {
    use sha2::{Digest, Sha256};
    let root = tempfile::tempdir().unwrap();
    let digest = format!("{:x}", Sha256::digest(b"stock ABI"));
    assert!(!library_matches(root.path(), "library", &digest));
    std::fs::write(root.path().join("library"), b"stock ABI").unwrap();
    assert!(library_matches(root.path(), "library", &digest));
    std::fs::write(root.path().join("library"), b"upgraded ABI").unwrap();
    assert!(!library_matches(root.path(), "library", &digest));
}

#[test]
fn electron_retirement_keeps_native_options_actions_and_user_edits() {
    use localdesktop::core::guest_compat::retire_electron_launch_options;
    let source = "[Desktop Entry]\nName=Example\nExec=example %U\n\n[Desktop Action New]\nExec=example --new\n";
    let managed = source.replace("[Desktop Entry]\n", "[Desktop Entry]\nX-LocalDesktop-NoSandbox=true\n")
        .replace("Exec=example", "Exec=example --disable-features=GlobalShortcutsPortal --enable-wayland-ime --wayland-text-input-version=3 --ozone-platform=wayland --no-stdio-init");
    let retired = retire_electron_launch_options(&managed, source).unwrap();
    assert!(!retired.contains("--no-stdio-init"));
    assert!(!retired.contains("--ozone-platform="));
    assert!(retired.contains("--wayland-text-input-version=3 %U"));
    assert!(retired.contains("--wayland-text-input-version=3 --new"));
    assert_eq!(
        retire_electron_launch_options(&retired, source),
        Some(retired.clone())
    );
    assert_eq!(retire_electron_launch_options(source, source), None);
    for custom in [
        format!("{managed}# User customization\n"),
        managed.replace("--ozone-platform=wayland", "--ozone-platform=x11"),
    ] {
        assert_eq!(retire_electron_launch_options(&custom, source), None);
    }
    let native = source.replace("Exec=example", "Exec=example --ozone-platform=wayland");
    assert_eq!(retire_electron_launch_options(&managed, &native), None);
    let explicit_platform = managed.replace(" --no-stdio-init", "");
    assert_eq!(
        retire_electron_launch_options(&explicit_platform, source),
        Some(explicit_platform)
    );
}
