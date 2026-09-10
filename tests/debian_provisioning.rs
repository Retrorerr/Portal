#[path = "../src/core/provisioning.rs"]
mod provisioning;
#[path = "../src/core/runtime.rs"]
mod runtime;
use provisioning::{RuntimeArtifact, IMAGE_MARKER, READY_MARKER};
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::Path};

#[test]
fn interrupted_download_resumes_and_servers_ignoring_ranges_restart_safely() {
    use std::io::Read;
    for supports_range in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let (mut artifact, archive) = fixture(temp.path(), "test-v1");
        let bytes = fs::read(&archive).unwrap();
        let offset = bytes.len() / 2;
        fs::write(temp.path().join("portal-runtime.tar.xz"), &bytes[..offset]).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        artifact.url = format!("http://{}/runtime", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            assert!(String::from_utf8(request)
                .unwrap()
                .to_lowercase()
                .contains(&format!("range: bytes={offset}-")));
            let body = if supports_range {
                &bytes[offset..]
            } else {
                &bytes[..]
            };
            let status = if supports_range {
                "206 Partial Content"
            } else {
                "200 OK"
            };
            // Content-Range is only valid on 206: the 200 fallback must not
            // carry it, so the client's range-ignoring path is genuinely hit.
            let range_header = if supports_range {
                format!(
                    "Content-Range: bytes {offset}-{}/{}\r\n",
                    bytes.len() - 1,
                    bytes.len()
                )
            } else {
                String::new()
            };
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{range_header}Connection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });
        artifact.provision(temp.path(), |_| {}).unwrap();
        server.join().unwrap();
        assert!(artifact.is_ready(&temp.path().join("runtime-B")));
    }
}

fn fixture(directory: &Path, version: &str) -> (RuntimeArtifact, std::path::PathBuf) {
    let archive = directory.join("image.xz");
    let encoder = xz2::write::XzEncoder::new(fs::File::create(&archive).unwrap(), 1);
    let mut tar = tar::Builder::new(encoder);
    for (path, value) in [
        (IMAGE_MARKER, version),
        ("usr/lib/os-release", "ID=debian\nVERSION_ID=\"13\"\n"),
        ("usr/bin/dpkg", "binary"),
        ("usr/bin/apt", "binary"),
        ("usr/bin/bash", "binary"),
        ("usr/bin/kwin_wayland", "binary"),
        ("usr/bin/plasmashell", "binary"),
        ("usr/bin/python3", "binary"),
        ("var/lib/dpkg/status", "Package: dpkg\n"),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(value.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, path, value.as_bytes())
            .unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
    let bytes = fs::read(&archive).unwrap();
    (
        RuntimeArtifact {
            version: version.into(),
            url: "http://127.0.0.1:1/not-used".into(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            compressed_bytes: bytes.len() as u64,
        },
        archive,
    )
}

#[test]
fn clean_and_legacy_selection_are_always_debian() {
    let temp = tempfile::tempdir().unwrap();
    let layout = runtime::RuntimeLayout::new(temp.path());
    assert_eq!(
        layout.active_slot().rootfs_path,
        temp.path().join("runtime-B")
    );
    fs::create_dir(temp.path().join("arch")).unwrap();
    layout.set_active_slot("slot-a").unwrap();
    assert_eq!(layout.active_slot().id, "slot-b");
}

#[test]
fn interrupted_extraction_is_not_ready_and_retries_from_staging() {
    let temp = tempfile::tempdir().unwrap();
    let (artifact, archive) = fixture(temp.path(), "test-v1");
    let staging = temp.path().join("runtime-B.staging");
    fs::create_dir(&staging).unwrap();
    fs::write(staging.join("partial"), "interrupted").unwrap();
    assert!(!artifact.is_ready(&staging));
    artifact.extract(&archive, &staging, &|_| {}).unwrap();
    assert!(!staging.join("partial").exists());
    assert!(artifact.is_ready(&staging));
    assert!(!temp.path().join("runtime-B").exists());
}

#[test]
fn completed_image_reuses_without_network_and_preserves_user_files() {
    let temp = tempfile::tempdir().unwrap();
    let (artifact, archive) = fixture(temp.path(), "test-v1");
    let root = temp.path().join("runtime-B");
    artifact.extract(&archive, &root, &|_| {}).unwrap();
    fs::write(root.join("user-file"), "keep").unwrap();
    artifact
        .provision(temp.path(), |_| panic!("Ready runtime must not download"))
        .unwrap();
    assert_eq!(fs::read_to_string(root.join("user-file")).unwrap(), "keep");
}

#[test]
fn compatible_runtime_survives_artifact_revision_and_preserves_user_files() {
    let temp = tempfile::tempdir().unwrap();
    let (artifact, archive) = fixture(temp.path(), "test-v1");
    let root = temp.path().join("runtime-B");
    artifact.extract(&archive, &root, &|_| {}).unwrap();
    fs::write(root.join("user-installed-package"), "keep").unwrap();

    // The image is still a complete Debian 13 installation, but the APK now
    // points at a newer exact artifact identity.  Mutable runtime-B must be
    // reused without a download or rotation.
    let mut newer = fixture(temp.path(), "test-v2").0;
    newer.url = "http://127.0.0.1:1/not-used".into();
    assert!(!newer.is_ready(&root));
    assert!(newer.is_bootable(&root));
    newer
        .provision(temp.path(), |_| {
            panic!("compatible runtime must not download")
        })
        .unwrap();
    assert_eq!(
        fs::read_to_string(root.join("user-installed-package")).unwrap(),
        "keep"
    );
}

#[test]
fn marked_but_corrupt_runtime_is_rejected_without_destructive_reprovisioning() {
    let temp = tempfile::tempdir().unwrap();
    let (artifact, archive) = fixture(temp.path(), "test-v1");
    let root = temp.path().join("runtime-B");
    artifact.extract(&archive, &root, &|_| {}).unwrap();
    fs::write(root.join("user-installed-package"), "keep").unwrap();
    fs::remove_file(root.join("usr/bin/bash")).unwrap();

    let error = artifact.provision(temp.path(), |_| {
        panic!("corrupt marked runtime must not download")
    });
    assert!(error.is_err());
    assert!(root.join(READY_MARKER).exists());
    assert!(!root.join("usr/bin/bash").exists());
    assert_eq!(
        fs::read_to_string(root.join("user-installed-package")).unwrap(),
        "keep"
    );
}

#[test]
fn rejects_wrong_version_and_corrupt_download_without_completion_marker() {
    let temp = tempfile::tempdir().unwrap();
    let (mut artifact, archive) = fixture(temp.path(), "test-v1");
    let stage = temp.path().join("staging");
    artifact.version = "test-v2".into();
    assert!(artifact.extract(&archive, &stage, &|_| {}).is_err());
    assert!(!stage.join(READY_MARKER).exists());
    fs::OpenOptions::new()
        .append(true)
        .open(&archive)
        .unwrap()
        .write_all(b"corrupt")
        .unwrap();
    assert!(artifact.verify(&archive).is_err());
}

#[test]
fn crashed_rename_promotes_ready_staging_without_redownload() {
    let temp = tempfile::tempdir().unwrap();
    let (artifact, archive) = fixture(temp.path(), "test-v1");
    // Simulate a crash between `rename(root, previous)` and
    // `rename(staging, root)`: no live root, but a READY-marked staging dir.
    let staging = temp.path().join("runtime-B.staging");
    artifact.extract(&archive, &staging, &|_| {}).unwrap();
    assert!(artifact.is_ready(&staging));
    artifact
        .provision(temp.path(), |msg| {
            assert!(
                !msg.to_lowercase().contains("download"),
                "Ready staging must not download: {msg}"
            );
        })
        .unwrap();
    assert!(artifact.is_ready(&temp.path().join("runtime-B")));
    assert!(!staging.exists());
}

#[test]
fn successful_provision_rotates_and_preserves_previous_runtime_slot() {
    use std::io::Read;
    let temp = tempfile::tempdir().unwrap();
    let (mut artifact, archive) = fixture(temp.path(), "test-v1");
    let bytes = fs::read(&archive).unwrap();
    // Seed a live runtime plus a stale `.previous` slot, then invalidate the
    // live root by removing its READY marker so provision must re-extract.
    // (The version string stays test-v1: the fixture bytes only validate as
    // test-v1.)
    artifact
        .extract(&archive, &temp.path().join("runtime-B"), &|_| {})
        .unwrap();
    fs::create_dir_all(temp.path().join("runtime-B.previous")).unwrap();
    fs::write(temp.path().join("runtime-B.previous/stale"), "old").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    artifact.url = format!("http://{}/runtime", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        while !request.ends_with(b"\r\n\r\n") {
            if stream.read_exact(&mut byte).is_err() {
                break;
            }
            request.push(byte[0]);
        }
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        )
        .unwrap();
        use std::io::Write;
        stream.write_all(&bytes).unwrap();
    });
    // Invalidate the live root so provision must re-extract, then confirm the
    // stale backup is rotated and the just-replaced runtime is retained.
    fs::remove_file(temp.path().join(format!("runtime-B/{READY_MARKER}"))).unwrap();
    artifact.provision(temp.path(), |_| {}).unwrap();
    server.join().unwrap();
    assert!(artifact.is_ready(&temp.path().join("runtime-B")));
    assert!(temp.path().join("runtime-B.previous").exists());
    assert!(!temp.path().join("runtime-B.previous/stale").exists());
}

#[test]
fn source_routes_only_release_image_and_preserves_session_handoff() {
    let setup = include_str!("../src/android/proot/setup.rs");
    let config_full = include_str!("../src/core/config.rs");
    // Production defaults live above the unit-test module; legacy migration
    // fixtures below `#[cfg(test)]` intentionally mention old managers.
    let config = config_full
        .split("mod tests {")
        .next()
        .unwrap_or(config_full);
    for removed in [
        "setup_arch_fs",
        "ARCH_FS_ARCHIVE",
        "pacman",
        "install_dependencies",
    ] {
        assert!(!setup.contains(removed), "{removed} remains in setup");
        assert!(!config.contains(removed), "{removed} remains in defaults");
    }
    assert!(setup.contains("Box::new(setup_debian_runtime)"));
    assert!(setup.contains("sync_session_runtime_files(fs_root, ui_scale)"));
    assert!(setup.contains("migrate_multiarch_dpkg_info"));
    assert!(setup.contains("dpkg-info-multiarch-v1"));
    assert!(setup.contains("dpkg-info-unqualified-v1"));
    assert!(setup.contains("pam-auth-update --package --force"));
    assert!(setup.contains("sync_base_files_defaults"));
    assert!(setup.contains("repair_base_files_runtime_links"));
    for function in [
        "sync_android_timezone(fs_root)",
        "sync_guest_network_config(fs_root)",
        "sync_firefox_config(fs_root)",
        "PORTAL_IME_BRIDGE",
    ] {
        assert!(setup.contains(function));
    }
    assert!(setup.contains("on_complete();"));
    let lifecycle = include_str!("../src/android/app/build.rs");
    assert!(lifecycle.contains("webview_handoff::complete_setup"));
    assert!(include_str!("../src/android/proot/launch.rs")
        .contains("RuntimeArtifact::production().is_bootable"));
    let main = include_str!("../src/android/main.rs");
    assert!(main.contains("\"XKB_CONFIG_ROOT\""));
    assert!(main.contains("config::PRODUCTION_FS_ROOT"));
    assert!(main.find("\"XKB_CONFIG_ROOT\"") < main.find("ApplicationContext::build"));
    let support = include_bytes!("../assets/guest-arm64/localdesktop-crash-handler.so");
    assert_eq!(&support[..4], b"\x7fELF");
    assert_eq!(u16::from_le_bytes([support[18], support[19]]), 183); // AArch64
    for symbol in [b"fstat\0".as_slice(), b"fstat64\0".as_slice()] {
        assert!(support.windows(symbol.len()).any(|bytes| bytes == symbol));
    }
    assert!(setup.contains("CRASH_HANDLER_BINARY"));
    assert!(!setup.contains("command -v gcc"));
    assert!(setup.contains("\"tmp/.X11-unix\""));
    assert!(setup.contains("\"tmp/.ICE-unix\""));
}

#[test]
fn production_runtime_artifact_matches_manifest_and_archive_verification() {
    let artifact = RuntimeArtifact::production();
    assert_eq!(artifact.version, "debian13-arm64-2026.09.05.3");
    assert_eq!(
        artifact.sha256,
        "aa75ea96300c26a9cfdffb443aff954a3cbe89146ffe32bba7287415e89e00f3"
    );
    assert_eq!(artifact.compressed_bytes, 896188212);
    assert!(artifact.url.starts_with("https://github.com/Retrorerr/Portal/releases/download/runtime-debian13-arm64-2026.09.05.3/"));
    let archive_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/portal-debian13-arm64-2026.09.05.3.tar.xz");
    if archive_path.exists() {
        artifact
            .verify(&archive_path)
            .expect("RuntimeArtifact::verify failed on target archive");
    }
}

