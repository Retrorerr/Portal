//! Debian system update policy: apt pin, simulation and status parsing.

use localdesktop::core::system_updates::{
    apt_pin_content, changed_protected_packages, check_due, friendly_status, installed_versions,
    overall_progress, parse_apt_status, parse_upgrade_simulation, sort_for_display, still_pending,
    AptStatus, PendingUpdate, UpdateCache, CHECK_INTERVAL_SECS, PROTECTED_PACKAGES,
};
use std::collections::HashMap;

fn update(package: &str, from: &str, to: &str, security: bool) -> PendingUpdate {
    PendingUpdate {
        package: package.into(),
        from: from.into(),
        to: to.into(),
        security,
    }
}

#[test]
fn pin_covers_every_anland_package_for_all_debian_archives() {
    let pin = apt_pin_content();
    let package_line = pin
        .lines()
        .find(|line| line.starts_with("Package: "))
        .expect("pin names its packages");
    let pinned: Vec<&str> = package_line["Package: ".len()..].split(' ').collect();
    assert_eq!(pinned, PROTECTED_PACKAGES);
    assert!(pin.lines().any(|line| line == "Pin: release o=Debian"));
    assert!(pin.lines().any(|line| line == "Pin-Priority: -1"));
}

#[test]
fn pin_matches_the_image_builder_overlay() {
    let builder = include_str!("../scripts/build_debian_rootfs.py");
    let overlay = builder
        .lines()
        .find(|line| line.starts_with("LFDEVS_OVERLAY_ORDER"))
        .expect("builder declares its overlay packages");
    for package in PROTECTED_PACKAGES {
        assert!(overlay.contains(&format!("\"{package}\"")), "{package} not in overlay");
    }
    assert_eq!(overlay.matches('"').count() / 2, PROTECTED_PACKAGES.len());
}

#[test]
fn simulation_lists_upgrades_new_packages_and_security_fixes() {
    let stdout = "\
Reading package lists...
Building dependency tree...
The following packages have been kept back:
  libqt6core6t64
The following packages will be upgraded:
  firefox-esr libssl3t64
Inst libssl3t64 [3.5.1-1] (3.5.1-1+deb13u1 Debian-Security:13/stable-security [arm64])
Inst firefox-esr [140.3.0esr-1~deb13u1] (140.4.0esr-1~deb13u1 Debian:13.2/stable [arm64])
Inst libnew1:arm64 (2.0-1 Debian:13.2/stable [arm64])
Conf libssl3t64 (3.5.1-1+deb13u1 Debian-Security:13/stable-security [arm64])
Conf firefox-esr (140.4.0esr-1~deb13u1 Debian:13.2/stable [arm64])
";
    assert_eq!(
        parse_upgrade_simulation(stdout),
        vec![
            update("libssl3t64", "3.5.1-1", "3.5.1-1+deb13u1", true),
            update("firefox-esr", "140.3.0esr-1~deb13u1", "140.4.0esr-1~deb13u1", false),
            update("libnew1", "", "2.0-1", false),
        ]
    );
}

#[test]
fn simulation_without_upgrades_is_empty() {
    let stdout = "Reading package lists...\n0 upgraded, 0 newly installed, 0 to remove and 0 not upgraded.\n";
    assert!(parse_upgrade_simulation(stdout).is_empty());
}

#[test]
fn installed_versions_ignore_removed_packages() {
    let status = "\
Package: kwin-wayland
Status: install ok installed
Version: 4:6.3.6-95

Package: gone
Status: deinstall ok config-files
Version: 1.0

Package: bash
Status: install ok installed
Priority: required
Version: 5.2.37-2+b9
";
    let installed = installed_versions(status);
    assert_eq!(installed.get("kwin-wayland").map(String::as_str), Some("4:6.3.6-95"));
    assert_eq!(installed.get("bash").map(String::as_str), Some("5.2.37-2+b9"));
    assert!(!installed.contains_key("gone"));
}

#[test]
fn updates_installed_elsewhere_are_no_longer_pending() {
    let installed: HashMap<String, String> = [
        ("firefox-esr".to_owned(), "140.4.0esr-1".to_owned()),
        ("libssl3t64".to_owned(), "3.5.1-1".to_owned()),
    ]
    .into();
    let cached = vec![
        update("firefox-esr", "140.3.0esr-1", "140.4.0esr-1", false),
        update("libssl3t64", "3.5.1-1", "3.5.1-1+deb13u1", true),
        update("libnew1", "", "2.0-1", false),
        update("removed", "1.0", "1.1", false),
    ];
    let pending = still_pending(&cached, &installed);
    assert_eq!(
        pending.iter().map(|u| u.package.as_str()).collect::<Vec<_>>(),
        ["libssl3t64", "libnew1"]
    );
}

#[test]
fn protected_package_changes_are_detected() {
    let before: HashMap<String, String> = PROTECTED_PACKAGES
        .iter()
        .map(|package| (package.to_string(), "4:6.3.6-95".to_owned()))
        .collect();
    let mut after = before.clone();
    after.insert("bash".into(), "5.3".into());
    assert!(changed_protected_packages(&before, &after).is_empty());
    after.insert("kwin-wayland".into(), "4:6.3.7-1".into());
    after.remove("libkwin6");
    assert_eq!(
        changed_protected_packages(&before, &after),
        ["kwin-wayland", "libkwin6"]
    );
}

#[test]
fn checks_are_rate_limited_but_never_stuck_by_clock_changes() {
    let cache = UpdateCache {
        checked_at: 1_000_000,
        updates: Vec::new(),
    };
    assert!(check_due(None, 5));
    assert!(!check_due(Some(&cache), 1_000_000 + CHECK_INTERVAL_SECS - 1));
    assert!(check_due(Some(&cache), 1_000_000 + CHECK_INTERVAL_SECS));
    assert!(check_due(Some(&cache), 999_999));
}

#[test]
fn cache_round_trips_and_rejects_garbage() {
    let cache = UpdateCache {
        checked_at: 42,
        updates: vec![update("bash", "1", "2", true)],
    };
    assert_eq!(UpdateCache::parse(&cache.to_json()), Some(cache));
    assert_eq!(UpdateCache::parse("not json"), None);
}

#[test]
fn security_fixes_sort_first() {
    let mut updates = vec![
        update("zlib1g", "1", "2", false),
        update("openssl", "1", "2", true),
        update("bash", "1", "2", false),
        update("curl", "1", "2", true),
    ];
    sort_for_display(&mut updates);
    assert_eq!(
        updates.iter().map(|u| u.package.as_str()).collect::<Vec<_>>(),
        ["curl", "openssl", "bash", "zlib1g"]
    );
}

#[test]
fn apt_status_lines_parse_with_colons_in_package_names() {
    assert_eq!(
        parse_apt_status("dlstatus:3:25.0000:Retrieving file 3 of 12"),
        Some(AptStatus::Download(25.0))
    );
    assert_eq!(
        parse_apt_status("pmstatus:libfoo:arm64:12.5:Unpacking libfoo:arm64 (1.0-2)\n"),
        Some(AptStatus::Install(12.5, "Unpacking libfoo:arm64 (1.0-2)".into()))
    );
    assert_eq!(
        parse_apt_status("pmerror:libfoo:arm64:40:subprocess installed post-installation script returned error exit status 1"),
        Some(AptStatus::Error(
            "libfoo:arm64: subprocess installed post-installation script returned error exit status 1".into()
        ))
    );
    assert_eq!(parse_apt_status("Reading package lists..."), None);
    assert_eq!(parse_apt_status("pmconffile:/etc/foo:'/etc/foo' '/etc/foo.dpkg-new' 1 1"), None);
}

#[test]
fn progress_moves_forward_through_download_then_install() {
    let download_start = overall_progress(&AptStatus::Download(0.0)).unwrap();
    let download_end = overall_progress(&AptStatus::Download(100.0)).unwrap();
    let install_end = overall_progress(&AptStatus::Install(100.0, String::new())).unwrap();
    assert_eq!(download_start, 10);
    assert_eq!(download_end, 50);
    assert_eq!(install_end, 96);
    assert_eq!(overall_progress(&AptStatus::Error(String::new())), None);
}

#[test]
fn dpkg_actions_are_shortened_for_the_launch_screen() {
    assert_eq!(friendly_status("Unpacking libfoo:arm64 (1.0-2)"), "Unpacking libfoo");
    assert_eq!(friendly_status("Installing firefox-esr"), "Installing firefox-esr");
    assert_eq!(
        friendly_status("Preparing to configure libc6:arm64 (2.41-12)"),
        "Preparing to configure libc6"
    );
}
