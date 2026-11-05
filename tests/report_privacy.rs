#[path = "../src/core/report_privacy.rs"]
mod report_privacy;
use report_privacy::{looks_binary, scrub};

#[test]
fn removes_what_identifies_the_person() {
    for (line, secret) in [
        ("mail me at jane.doe+x@example.co.uk now", "jane.doe"),
        (
            "GET https://bank.example.com/acct?id=9 failed",
            "bank.example",
        ),
        ("https://user:token@github.com/x", "token"),
        ("content://media/external/images/44", "media/external"),
        ("connected to 192.168.0.244:5555", "192.168.0.244"),
        ("peer=8.8.8.8", "8.8.8.8"),
        ("wlan0 2a02:c7c:1234:5678::1 up", "2a02:c7c"),
        ("bssid aa:bb:cc:dd:ee:ff", "aa:bb"),
        ("open /storage/emulated/0/DCIM/me.jpg", "me.jpg"),
        ("open /sdcard/Download/tax.pdf: denied", "tax.pdf"),
        ("/home/desktop/Documents/tax.pdf not found", "tax.pdf"),
        ("arch/root/secret-plans.odt", "secret-plans"),
    ] {
        let scrubbed = scrub(line);
        assert!(!scrubbed.contains(secret), "{line} -> {scrubbed}");
    }
}

#[test]
fn keeps_what_identifies_the_bug() {
    for line in [
        "kwin_wayland: failed to load /usr/lib/aarch64-linux-gnu/libEGL.so.1.1.0",
        "libfoo.so.1.2.3.4 missing",
        "mesa-26.3.0.1 loaded",
        "bound to 127.0.0.1:8080 and 0.0.0.0",
        "netmask 255.255.255.0",
        "GET https://github.com/Retrorerr/Portal/releases/download/x/y.tar.zst 503",
        "fetch https://deb.debian.org/debian/dists/trixie/InRelease failed",
        "reading /home/desktop/.config/kwinrc",
        "file:///usr/share/icons/breeze/index.theme",
        "unix:path=/run/user/1000/bus",
        "chip_id=0x43051401 gpu_id=830",
        "12:34:56.789 thread=7 version=2.8.0",
    ] {
        assert_eq!(scrub(line), line);
    }
}

#[test]
fn file_urls_still_lose_document_names() {
    assert!(!scrub("file:///home/desktop/Pictures/me.png").contains("me.png"));
}

#[test]
fn multibyte_neighbours_do_not_panic() {
    assert_eq!(scrub("é8.8.8.8é"), "é8.8.8.8é");
    assert_eq!(scrub("→ 8.8.8.8 ←"), "→ <ip> ←");
}

#[test]
fn binary_is_detected_early_in_the_file() {
    assert!(looks_binary(b"ELF\0\x01"));
    assert!(!looks_binary("plain log line\n".as_bytes()));
}

/// The app fills these form fields through the URL. GitHub ignores an unknown
/// field without an error, so a rename would silently empty every report.
#[test]
fn issue_forms_have_the_fields_the_app_prefills() {
    for form in ["bug-report.yml", "compatibility-report.yml"] {
        let path = format!(
            "{}/.github/ISSUE_TEMPLATE/{form}",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("id: device-info"), "{form} lacks device-info");
    }
    let bug = std::fs::read_to_string(format!(
        "{}/.github/ISSUE_TEMPLATE/bug-report.yml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    assert!(bug.contains("id: evidence"));
    let app = std::fs::read_to_string(format!(
        "{}/src/android/kotlin/app/polarbear/PortalReport.kt",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    for needle in [
        "bug-report.yml",
        "compatibility-report.yml",
        "\"device-info\"",
        "\"evidence\"",
    ] {
        assert!(app.contains(needle), "PortalReport.kt lacks {needle}");
    }
}
