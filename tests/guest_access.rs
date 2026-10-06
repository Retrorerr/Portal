//! Guest privilege and browser defaults: passwordless `sudo` for the desktop
//! account, the default browser, and Portal's Firefox preferences for every
//! Firefox a user can install with apt.
//!
//! Both modules are platform-independent, so these run on the Windows dev
//! host; the Unix-only checks (modes, the setuid bit, the dpkg hook script)
//! run wherever the tests do on Linux.
#![allow(dead_code)]

#[path = "../src/core/guest_browser.rs"]
mod guest_browser;
#[path = "../src/core/guest_sudo.rs"]
mod guest_sudo;

use std::{fs, path::Path};
use tempfile::TempDir;

const BUILDER: &str = include_str!("../scripts/build_debian_rootfs.py");

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn read(root: &Path, relative: &str) -> String {
    fs::read_to_string(root.join(relative)).unwrap()
}

// ---------------------------------------------------------------- sudo ----

#[test]
fn desktop_joins_an_empty_sudo_group_and_nothing_else_changes() {
    let before = "root:x:0:\ndesktop:x:1000:\nsudo:x:27:\nusers:x:100:\n";
    assert_eq!(
        guest_sudo::ensure_group_member(before).as_deref(),
        Some("root:x:0:\ndesktop:x:1000:\nsudo:x:27:desktop\nusers:x:100:\n")
    );
}

#[test]
fn desktop_is_added_after_existing_sudo_members_and_only_once() {
    assert_eq!(
        guest_sudo::ensure_group_member("sudo:x:27:alice\n").as_deref(),
        Some("sudo:x:27:alice,desktop\n")
    );
    assert_eq!(
        guest_sudo::ensure_group_member("sudo:x:27:alice,desktop\n"),
        None
    );
    assert_eq!(guest_sudo::ensure_group_member("sudo:x:27:desktop\n"), None);
}

#[test]
fn a_missing_sudo_group_is_created_with_debians_gid() {
    assert_eq!(
        guest_sudo::ensure_group_member("root:x:0:\ndesktop:x:1000:").as_deref(),
        Some("root:x:0:\ndesktop:x:1000:\nsudo:x:27:desktop\n"),
        "also handles a file without a trailing newline"
    );
}

#[test]
fn the_sudo_group_avoids_a_gid_someone_else_uses() {
    let text = guest_sudo::ensure_group_member("root:x:0:\nother:x:27:\n").unwrap();
    assert!(text.contains("\nsudo:x:999:desktop\n"), "{text}");
    assert!(text.contains("\nother:x:27:\n"));
}

#[test]
fn a_malformed_sudo_record_is_left_alone() {
    assert_eq!(guest_sudo::ensure_group_member("sudo:x:27\n"), None);
    assert_eq!(guest_sudo::ensure_group_member("sudo:x:27:a:b\n"), None);
}

#[test]
fn gshadow_is_updated_only_when_it_already_has_a_sudo_entry() {
    assert_eq!(
        guest_sudo::ensure_gshadow_member("root:*::\nsudo:!*::\n").as_deref(),
        Some("root:*::\nsudo:!*::desktop\n")
    );
    assert_eq!(guest_sudo::ensure_gshadow_member("root:*::\n"), None);
    assert_eq!(
        guest_sudo::ensure_gshadow_member("sudo:!*::desktop\n"),
        None
    );
}

#[test]
fn shadow_gets_locked_entries_for_root_and_desktop_when_missing() {
    let passwd = "root:x:0:0:root:/root:/bin/bash\ndesktop:x:1000:1000::/home/desktop:/bin/bash\n";
    let shadow = "daemon:!*:20725::::::\n";
    let updated = guest_sudo::ensure_shadow_entries(shadow, passwd, 20_000).unwrap();
    assert_eq!(
        updated,
        "daemon:!*:20725::::::\nroot:*:20000:0:99999:7:::\ndesktop:*:20000:0:99999:7:::\n"
    );
    assert_eq!(
        guest_sudo::ensure_shadow_entries(&updated, passwd, 20_001),
        None
    );
}

#[test]
fn existing_shadow_entries_are_never_modified() {
    let passwd = "root:x:0:0::/root:/bin/bash\ndesktop:x:1000:1000::/home/desktop:/bin/bash\n";
    let shadow = "root:$y$hash:19000:0:99999:7:::\ndesktop:!:19000::::::";
    assert_eq!(
        guest_sudo::ensure_shadow_entries(shadow, passwd, 20_000),
        None
    );
}

#[test]
fn shadow_entries_are_only_added_for_accounts_the_passwd_file_has() {
    let passwd = "root:x:0:0::/root:/bin/bash\n";
    assert_eq!(
        guest_sudo::ensure_shadow_entries("", passwd, 1).as_deref(),
        Some("root:*:1:0:99999:7:::\n")
    );
}

fn sudo_rootfs() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "etc/passwd",
        "root:x:0:0::/root:/bin/bash\ndesktop:x:1000:1000::/home/desktop:/bin/bash\n",
    );
    write(
        root,
        "etc/group",
        "root:x:0:\ndesktop:x:1000:\nsudo:x:27:\n",
    );
    write(root, "etc/gshadow", "root:*::\nsudo:!*::\n");
    write(root, "etc/shadow", "daemon:!*:20725::::::\n");
    write(
        root,
        "etc/sudoers",
        "%sudo ALL=(ALL:ALL) ALL\n@includedir /etc/sudoers.d\n",
    );
    write(root, "etc/sudoers.d/README", "# sudo\n");
    write(root, "usr/bin/sudo", "\x7fELF");
    dir
}

#[test]
fn a_launch_makes_desktop_a_passwordless_sudoer_and_the_next_changes_nothing() {
    let dir = sudo_rootfs();
    let root = dir.path();

    let first = guest_sudo::sync_sudo_access(root);
    assert!(first.failed.is_empty(), "{:?}", first.failed);
    assert!(first.changed.contains(&"group"));
    assert!(first.changed.contains(&"gshadow"));
    assert!(first.changed.contains(&"shadow"));
    assert!(first.changed.contains(&"sudoers"));

    assert!(read(root, "etc/group").contains("\nsudo:x:27:desktop\n"));
    assert!(read(root, "etc/gshadow").contains("\nsudo:!*::desktop\n"));
    let shadow = read(root, "etc/shadow");
    assert!(shadow.contains("\nroot:*:") && shadow.contains("\ndesktop:*:"));
    assert_eq!(
        read(root, "etc/sudoers.d/portal-desktop"),
        guest_sudo::SUDOERS_DROPIN
    );
    assert_eq!(
        read(root, "etc/sudoers.d/README"),
        "# sudo\n",
        "other drop-ins untouched"
    );

    let second = guest_sudo::sync_sudo_access(root);
    assert!(second.failed.is_empty(), "{:?}", second.failed);
    assert!(
        second.changed.is_empty(),
        "idempotent, but changed {:?}",
        second.changed
    );
    assert!(
        !root.join("etc/.group.portal-tmp").exists(),
        "no temporary files left behind"
    );
}

#[test]
fn an_edited_or_replaced_sudoers_dropin_is_never_overwritten() {
    for edited in [
        "desktop ALL=(ALL) ALL\n",
        "",
        guest_sudo::SUDOERS_DROPIN.trim_end(),
    ] {
        let dir = sudo_rootfs();
        write(dir.path(), guest_sudo::SUDOERS_DROPIN_RELATIVE, edited);
        let report = guest_sudo::sync_sudo_access(dir.path());
        assert!(!report.changed.contains(&"sudoers"));
        assert_eq!(
            read(dir.path(), guest_sudo::SUDOERS_DROPIN_RELATIVE),
            edited
        );
    }
}

#[test]
fn without_sudo_installed_no_dropin_or_directory_is_created() {
    let dir = sudo_rootfs();
    fs::remove_dir_all(dir.path().join("etc/sudoers.d")).unwrap();
    fs::remove_file(dir.path().join("usr/bin/sudo")).unwrap();
    let report = guest_sudo::sync_sudo_access(dir.path());
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(!dir.path().join("etc/sudoers.d").exists());
    assert!(
        report.changed.contains(&"group"),
        "group membership is still recorded"
    );
}

#[test]
fn a_tree_without_account_databases_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let report = guest_sudo::sync_sudo_access(dir.path());
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(report.changed.is_empty());
}

#[test]
fn the_dropin_grants_exactly_the_sudo_group_without_a_password() {
    let policy: Vec<&str> = guest_sudo::SUDOERS_DROPIN
        .lines()
        .filter(|line| !line.starts_with('#'))
        .collect();
    assert_eq!(policy, ["%sudo ALL=(ALL:ALL) NOPASSWD: ALL"]);
    assert!(guest_sudo::SUDOERS_DROPIN.ends_with('\n'));
}

#[cfg(unix)]
#[test]
fn sudo_gets_the_setuid_bit_and_the_dropin_gets_mode_0440() {
    use std::os::unix::fs::PermissionsExt;
    let dir = sudo_rootfs();
    let sudo = dir.path().join("usr/bin/sudo");
    fs::set_permissions(&sudo, fs::Permissions::from_mode(0o755)).unwrap();

    let report = guest_sudo::sync_sudo_access(dir.path());
    assert!(report.changed.contains(&"setuid"));
    assert_eq!(
        fs::metadata(&sudo).unwrap().permissions().mode() & 0o7777,
        0o4755
    );
    let dropin = dir.path().join(guest_sudo::SUDOERS_DROPIN_RELATIVE);
    assert_eq!(
        fs::metadata(dropin).unwrap().permissions().mode() & 0o7777,
        0o440
    );

    assert!(!guest_sudo::sync_sudo_access(dir.path())
        .changed
        .contains(&"setuid"));
}

#[cfg(unix)]
#[test]
fn edited_account_files_keep_their_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = sudo_rootfs();
    let shadow = dir.path().join("etc/shadow");
    fs::set_permissions(&shadow, fs::Permissions::from_mode(0o640)).unwrap();
    guest_sudo::sync_sudo_access(dir.path());
    assert_eq!(
        fs::metadata(shadow).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[test]
fn the_image_builder_ships_the_same_group_and_dropin_the_launch_sync_restores() {
    for line in guest_sudo::SUDOERS_DROPIN.lines() {
        assert!(
            BUILDER.contains(line),
            "builder is missing the drop-in line: {line}"
        );
    }
    assert!(
        BUILDER.contains("\"sudo:x:27:desktop\\n\""),
        "builder must put desktop in the sudo group"
    );
    assert!(BUILDER.contains(guest_sudo::SUDOERS_DROPIN_RELATIVE));
    assert_eq!(guest_sudo::SUDO_GID, 27);
}

// ------------------------------------------------------- default browser ----

fn default_line<'a>(text: &'a str, mime: &str) -> &'a str {
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{mime}=")))
        .unwrap_or_else(|| panic!("no default for {mime}"))
}

#[test]
fn mozillas_firefox_is_preferred_over_debians_esr_for_every_web_type() {
    let text = guest_browser::default_applications();
    for mime in [
        "x-scheme-handler/http",
        "x-scheme-handler/https",
        "text/html",
        "application/xhtml+xml",
    ] {
        let ids: Vec<&str> = default_line(&text, mime)
            .split(';')
            .filter(|id| !id.is_empty())
            .collect();
        assert_eq!(ids[0], "firefox.desktop", "{mime}");
        assert_eq!(ids[1], "firefox-esr.desktop", "{mime}");
        for channel in [
            "firefox-beta.desktop",
            "firefox-devedition.desktop",
            "firefox-nightly.desktop",
        ] {
            assert!(ids.contains(&channel), "{mime} lacks {channel}");
        }
    }
}

#[test]
fn the_non_browser_defaults_are_unchanged() {
    let text = guest_browser::default_applications();
    assert!(text.starts_with("# Managed by Portal"));
    assert_eq!(
        default_line(&text, "x-scheme-handler/mailto"),
        "thunderbird.desktop;"
    );
    assert_eq!(default_line(&text, "text/csv"), "libreoffice-calc.desktop;");
    assert_eq!(
        default_line(
            &text,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        ),
        "libreoffice-writer.desktop;"
    );
    assert!(text.ends_with('\n'));
}

#[test]
fn a_stale_portal_managed_mimeapps_is_upgraded_and_a_users_own_file_is_not() {
    let stale = "# Managed by Portal: system-wide default applications.\n[Default Applications]\nx-scheme-handler/http=firefox-esr.desktop;\n";
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "etc/xdg/mimeapps.list", stale);
    guest_browser::sync_default_applications(dir.path());
    assert_eq!(
        read(dir.path(), "etc/xdg/mimeapps.list"),
        guest_browser::default_applications()
    );

    let own = "[Default Applications]\nx-scheme-handler/http=chromium.desktop;\n";
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "etc/xdg/mimeapps.list", own);
    guest_browser::sync_default_applications(dir.path());
    assert_eq!(read(dir.path(), "etc/xdg/mimeapps.list"), own);
}

#[test]
fn a_missing_mimeapps_is_created() {
    let dir = tempfile::tempdir().unwrap();
    guest_browser::sync_default_applications(dir.path());
    assert_eq!(
        read(dir.path(), "etc/xdg/mimeapps.list"),
        guest_browser::default_applications()
    );
}

// ------------------------------------------------------- firefox prefs ----

fn firefox_rootfs() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "usr/lib/firefox-esr/application.ini", "");
    write(root, "usr/lib/firefox-nightly/libxul.so", "");
    write(root, "usr/lib/firefox-devedition/firefox-bin", "");
    write(root, "usr/lib/firefox-beta/application.ini", "");
    // Not browsers: no marker, or not a Firefox name.
    write(root, "usr/lib/firefox-locale-data/README", "");
    write(root, "usr/lib/firefoxish/application.ini", "");
    write(root, "usr/lib/thunderbird/application.ini", "");
    dir
}

fn names(root: &Path) -> Vec<String> {
    guest_browser::firefox_install_dirs(root)
        .iter()
        .map(|dir| dir.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn every_installed_firefox_channel_is_found_without_naming_it() {
    let dir = firefox_rootfs();
    assert_eq!(
        names(dir.path()),
        [
            "firefox",
            "firefox-beta",
            "firefox-devedition",
            "firefox-esr",
            "firefox-nightly"
        ]
    );
}

#[test]
fn mozillas_firefox_directory_is_found_once_it_is_installed() {
    let dir = firefox_rootfs();
    write(dir.path(), "usr/lib/firefox/libxul.so", "");
    let found = names(dir.path());
    assert_eq!(found.iter().filter(|name| *name == "firefox").count(), 1);
}

#[test]
fn prefs_reach_every_firefox_and_no_other_directory() {
    let dir = firefox_rootfs();
    let root = dir.path();
    guest_browser::sync_firefox_config(root);

    for name in [
        "firefox",
        "firefox-esr",
        "firefox-beta",
        "firefox-devedition",
        "firefox-nightly",
    ] {
        let base = format!("usr/lib/{name}");
        assert_eq!(
            read(root, &format!("{base}/defaults/pref/autoconfig.js")),
            guest_browser::AUTOCONFIG_JS,
            "{name}"
        );
        assert_eq!(
            read(root, &format!("{base}/localdesktop.cfg")),
            guest_browser::FIREFOX_CFG,
            "{name}"
        );
    }
    for name in ["firefox-locale-data", "firefoxish", "thunderbird"] {
        assert!(
            !root
                .join(format!("usr/lib/{name}/localdesktop.cfg"))
                .exists(),
            "{name}"
        );
    }
}

#[test]
fn the_prefs_keep_the_sandbox_and_force_gpu_compositing() {
    for sandbox in [
        "security.sandbox.content.level",
        "media.cubeb.sandbox",
        "media.allow-audio-non-utility",
        "media.rdd-process.enabled",
    ] {
        assert!(!guest_browser::FIREFOX_CFG.contains(sandbox), "{sandbox}");
    }
    for required in [
        "defaultPref(\"gfx.webrender.all\", true);",
        "defaultPref(\"layers.acceleration.force-enabled\", true);",
    ] {
        assert!(guest_browser::FIREFOX_CFG.contains(required), "{required}");
    }
    assert!(guest_browser::AUTOCONFIG_JS
        .contains("pref(\"general.config.filename\", \"localdesktop.cfg\");"));
    assert!(
        guest_browser::AUTOCONFIG_JS.contains("pref(\"general.config.sandbox_enabled\", false);")
    );
}

#[test]
fn prefs_sync_repairs_a_replaced_config_and_leaves_matching_files_alone() {
    let dir = firefox_rootfs();
    let root = dir.path();
    guest_browser::sync_firefox_config(root);
    let cfg = root.join("usr/lib/firefox-nightly/localdesktop.cfg");
    fs::write(&cfg, "// replaced by a package upgrade\n").unwrap();
    guest_browser::sync_firefox_config(root);
    assert_eq!(
        fs::read_to_string(&cfg).unwrap(),
        guest_browser::FIREFOX_CFG
    );
}

#[test]
fn a_firefox_launcher_counts_as_installed_whatever_its_channel() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!guest_browser::firefox_installed(dir.path()));
    write(dir.path(), "usr/bin/firefox-nightly", "");
    assert!(guest_browser::firefox_installed(dir.path()));
}

#[cfg(unix)]
#[test]
fn a_dangling_absolute_symlink_launcher_counts_as_installed() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("usr/bin")).unwrap();
    // Mozilla's package links /usr/bin/firefox to an absolute guest path,
    // which does not resolve from the host side.
    std::os::unix::fs::symlink(
        "/usr/lib/firefox/firefox",
        dir.path().join("usr/bin/firefox"),
    )
    .unwrap();
    assert!(guest_browser::firefox_installed(dir.path()));
}

// ------------------------------------------------------------ dpkg hook ----

#[test]
fn the_dpkg_hook_files_are_installed_and_named_the_way_dpkg_reads_them() {
    let dir = tempfile::tempdir().unwrap();
    guest_browser::sync_firefox_prefs_hook(dir.path());
    let root = dir.path();

    assert_eq!(
        read(root, "usr/local/share/portal/firefox/autoconfig.js"),
        guest_browser::AUTOCONFIG_JS
    );
    assert_eq!(
        read(root, "usr/local/share/portal/firefox/localdesktop.cfg"),
        guest_browser::FIREFOX_CFG
    );
    assert_eq!(
        read(root, guest_browser::PREFS_HOOK_SCRIPT_PATH),
        guest_browser::PREFS_HOOK_SCRIPT
    );

    let conf = read(root, guest_browser::DPKG_HOOK_CONF_PATH);
    let command = conf
        .lines()
        .find_map(|line| line.strip_prefix("post-invoke="))
        .expect("dpkg config has a post-invoke line");
    assert_eq!(
        command,
        format!("/{}", guest_browser::PREFS_HOOK_SCRIPT_PATH)
    );

    // dpkg reads only dpkg.cfg.d files named with letters, digits, '_' and '-'.
    let name = Path::new(guest_browser::DPKG_HOOK_CONF_PATH)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        "{name}"
    );
    assert!(guest_browser::PREFS_HOOK_SCRIPT.starts_with("#!/bin/sh\n"));
}

#[test]
fn syncing_the_hook_twice_writes_nothing_new() {
    let dir = tempfile::tempdir().unwrap();
    guest_browser::sync_firefox_prefs_hook(dir.path());
    let script = dir.path().join(guest_browser::PREFS_HOOK_SCRIPT_PATH);
    let before = fs::metadata(&script).unwrap().modified().unwrap();
    guest_browser::sync_firefox_prefs_hook(dir.path());
    assert_eq!(fs::metadata(&script).unwrap().modified().unwrap(), before);
    assert!(!dir
        .path()
        .join("usr/local/bin/.portal-firefox-prefs.portal-tmp")
        .exists());
}

#[cfg(unix)]
#[test]
fn the_hook_script_is_executable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    guest_browser::sync_firefox_prefs_hook(dir.path());
    let mode = fs::metadata(dir.path().join(guest_browser::PREFS_HOOK_SCRIPT_PATH))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
}

#[cfg(unix)]
#[test]
fn the_hook_script_restores_prefs_after_a_package_change_without_a_portal_launch() {
    use std::process::Command;
    let dir = firefox_rootfs();
    let root = dir.path();
    guest_browser::sync_firefox_prefs_hook(root);
    let script = root.join(guest_browser::PREFS_HOOK_SCRIPT_PATH);

    let run = || {
        Command::new("sh")
            .arg(&script)
            .arg(root)
            .status()
            .expect("sh is available on the unix test host")
    };

    // `sudo apt install firefox-nightly` (or an upgrade) landed after the
    // last Portal launch: the browser directory has no Portal config yet.
    assert!(!root
        .join("usr/lib/firefox-nightly/localdesktop.cfg")
        .exists());
    assert!(run().success());
    for name in [
        "firefox-esr",
        "firefox-nightly",
        "firefox-devedition",
        "firefox-beta",
    ] {
        let base = root.join("usr/lib").join(name);
        assert_eq!(
            fs::read_to_string(base.join("localdesktop.cfg")).unwrap(),
            guest_browser::FIREFOX_CFG,
            "{name}"
        );
        assert_eq!(
            fs::read_to_string(base.join("defaults/pref/autoconfig.js")).unwrap(),
            guest_browser::AUTOCONFIG_JS,
            "{name}"
        );
    }
    assert!(!root
        .join("usr/lib/firefox-locale-data/localdesktop.cfg")
        .exists());
    assert!(!root.join("usr/lib/firefoxish/localdesktop.cfg").exists());
    assert!(!root.join("usr/lib/thunderbird/localdesktop.cfg").exists());

    // A reinstall replaces the file with the package's own copy.
    fs::write(
        root.join("usr/lib/firefox-nightly/localdesktop.cfg"),
        "// package copy\n",
    )
    .unwrap();
    assert!(run().success());
    assert_eq!(
        fs::read_to_string(root.join("usr/lib/firefox-nightly/localdesktop.cfg")).unwrap(),
        guest_browser::FIREFOX_CFG
    );
}

#[cfg(unix)]
#[test]
fn the_hook_script_never_fails_even_with_nothing_to_do() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("hook.sh");
    fs::write(&script, guest_browser::PREFS_HOOK_SCRIPT).unwrap();
    // No share directory, no browsers: exit 0 so dpkg never warns.
    assert!(Command::new("sh")
        .arg(&script)
        .arg(dir.path())
        .status()
        .unwrap()
        .success());
}

#[test]
fn firefoxs_userapp_default_is_pointed_back_at_its_packaged_entry() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "usr/share/applications/firefox.desktop", "[Desktop Entry]\n");
    write(
        dir.path(),
        "home/desktop/.local/share/applications/userapp-Firefox-FVUIW3.desktop",
        "[Desktop Entry]\nType=Application\nNoDisplay=true\nExec=/usr/lib/firefox/firefox-bin %u\nName=Firefox\n",
    );
    write(
        dir.path(),
        "home/desktop/.local/share/applications/userapp-Other-AAAAAA.desktop",
        "[Desktop Entry]\nExec=/opt/other/bin %u\n",
    );
    write(
        dir.path(),
        "home/desktop/.config/mimeapps.list",
        "[Added Associations]\nx-scheme-handler/http=userapp-Firefox-FVUIW3.desktop;firefox.desktop;\n\n[Default Applications]\nx-scheme-handler/http=userapp-Firefox-FVUIW3.desktop\ntext/plain=userapp-Other-AAAAAA.desktop\n",
    );
    guest_browser::repair_user_default_browser(dir.path());
    assert_eq!(
        read(dir.path(), "home/desktop/.config/mimeapps.list"),
        "[Added Associations]\nx-scheme-handler/http=firefox.desktop;\n\n[Default Applications]\nx-scheme-handler/http=firefox.desktop;\ntext/plain=userapp-Other-AAAAAA.desktop\n"
    );
}

#[test]
fn a_userapp_for_a_firefox_without_a_packaged_entry_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "home/desktop/.local/share/applications/userapp-Firefox-X.desktop",
        "[Desktop Entry]\nExec=/usr/lib/firefox-nightly/firefox-bin %u\n",
    );
    let own = "[Default Applications]\nx-scheme-handler/http=userapp-Firefox-X.desktop\n";
    write(dir.path(), "home/desktop/.config/mimeapps.list", own);
    guest_browser::repair_user_default_browser(dir.path());
    assert_eq!(read(dir.path(), "home/desktop/.config/mimeapps.list"), own);
}
