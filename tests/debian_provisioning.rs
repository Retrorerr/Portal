#[path = "../src/core/provisioning.rs"]
mod provisioning;
#[path = "../src/core/runtime.rs"]
mod runtime;
use provisioning::{
    begin_installation, InstallOperationState, InstallStart, RuntimeArtifact, IMAGE_MARKER,
    ProvisioningPhase, ProvisioningSnapshot, IMAGE_READY_MARKER, PARTIAL_ARCHIVE, READY_MARKER,
};
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
        let root = temp.path().join("runtime-B");
        assert!(artifact.is_image_ready(&root));
        assert!(!artifact.is_bootable(&root));
        assert!(!temp.path().join(PARTIAL_ARCHIVE).exists());
        artifact.mark_installation_complete(&root).unwrap();
        assert!(artifact.is_bootable(&root));
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
            source_commit: None,
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
    assert!(!staging.join(READY_MARKER).exists());
    assert!(!artifact.is_bootable(&staging));
    assert!(!temp.path().join("runtime-B").exists());
}

#[test]
fn duplicate_begin_calls_attach_and_retry_is_the_only_restart() {
    let mut state = InstallOperationState::Idle;
    assert_eq!(begin_installation(&mut state), InstallStart::Start);
    assert_eq!(state, InstallOperationState::Running);
    assert_eq!(begin_installation(&mut state), InstallStart::Attach);
    state = InstallOperationState::Failed;
    assert_eq!(begin_installation(&mut state), InstallStart::Start);
    assert_eq!(begin_installation(&mut state), InstallStart::Attach);
    state = InstallOperationState::Complete;
    assert_eq!(begin_installation(&mut state), InstallStart::Noop);
    assert_eq!(state, InstallOperationState::Complete);
}

#[test]
fn nonterminal_progress_can_never_claim_completion() {
    let snapshot = ProvisioningSnapshot::update(
        ProvisioningPhase::Finalising,
        100,
        "Finalising Portal installation…",
    );
    assert_eq!(snapshot.progress, 99);
    assert_eq!(snapshot.phase, ProvisioningPhase::Finalising);
    assert_eq!(
        ProvisioningSnapshot::complete("Portal installed").progress,
        100
    );
}

#[test]
fn legacy_completed_runtime_is_migrated_in_place_without_download() {
    let temp = tempfile::tempdir().unwrap();
    let (artifact, archive) = fixture(temp.path(), "test-v1");
    let root = temp.path().join("runtime-B");
    artifact.extract(&archive, &root, &|_| {}).unwrap();
    fs::remove_file(root.join(IMAGE_READY_MARKER)).unwrap();
    fs::write(
        root.join(READY_MARKER),
        format!("{}\n{}\n", artifact.version, artifact.sha256),
    )
    .unwrap();
    fs::write(root.join("user-file"), "keep").unwrap();

    assert!(!artifact.is_bootable(&root));
    assert!(artifact.is_legacy_complete(&root));
    artifact
        .provision(temp.path(), |_| panic!("legacy runtime must not download"))
        .unwrap();
    artifact.mark_installation_complete(&root).unwrap();

    assert!(artifact.is_bootable(&root));
    assert_eq!(fs::read_to_string(root.join("user-file")).unwrap(), "keep");
    assert_eq!(
        fs::read_to_string(root.join(READY_MARKER))
            .unwrap()
            .lines()
            .nth(2),
        Some("portal-installation-v1")
    );
}

#[test]
fn invalid_resume_range_is_rejected_without_mutating_the_partial_prefix() {
    use std::io::Read;

    let temp = tempfile::tempdir().unwrap();
    let (mut artifact, archive) = fixture(temp.path(), "test-v1");
    let bytes = fs::read(&archive).unwrap();
    let offset = bytes.len() / 2;
    let partial = temp.path().join(PARTIAL_ARCHIVE);
    let prefix = bytes[..offset].to_vec();
    fs::write(&partial, &prefix).unwrap();
    fs::write(
        temp.path().join("portal-runtime.tar.xz.part.offset"),
        format!("{offset}\n"),
    )
    .unwrap();

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
        write!(
            stream,
            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
            bytes.len() - offset,
            offset + 1,
            bytes.len() - 1,
            bytes.len()
        )
        .unwrap();
        let _ = stream.write_all(&bytes[offset..]);
    });

    assert!(artifact.provision(temp.path(), |_| {}).is_err());
    server.join().unwrap();
    assert_eq!(fs::read(&partial).unwrap(), prefix);
    assert!(!temp.path().join("portal-runtime.tar.xz").exists());
    assert!(!temp.path().join("runtime-B").exists());
}

#[test]
fn incorrect_response_size_is_rejected_before_any_archive_is_committed() {
    use std::io::Read;

    let temp = tempfile::tempdir().unwrap();
    let (mut artifact, archive) = fixture(temp.path(), "test-v1");
    let bytes = fs::read(&archive).unwrap();
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
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len() + 1
        )
        .unwrap();
        stream.write_all(&bytes).unwrap();
    });

    assert!(artifact.provision(temp.path(), |_| {}).is_err());
    server.join().unwrap();
    assert!(!temp.path().join("portal-runtime.tar.xz").exists());
    assert!(!temp.path().join(PARTIAL_ARCHIVE).exists());
    assert!(!temp.path().join("runtime-B").exists());
}

#[test]
fn completed_image_reuses_without_network_and_preserves_user_files() {
    let temp = tempfile::tempdir().unwrap();
    let (artifact, archive) = fixture(temp.path(), "test-v1");
    let root = temp.path().join("runtime-B");
    artifact.extract(&archive, &root, &|_| {}).unwrap();
    artifact.mark_installation_complete(&root).unwrap();
    fs::write(root.join("user-file"), "keep").unwrap();
    artifact
        .provision(temp.path(), |_| panic!("Ready runtime must not download"))
        .unwrap();
    assert_eq!(fs::read_to_string(root.join("user-file")).unwrap(), "keep");
    assert!(artifact.is_bootable(&root));
}

#[test]
fn compatible_runtime_survives_artifact_revision_and_preserves_user_files() {
    let temp = tempfile::tempdir().unwrap();
    let (artifact, archive) = fixture(temp.path(), "test-v1");
    let root = temp.path().join("runtime-B");
    artifact.extract(&archive, &root, &|_| {}).unwrap();
    artifact.mark_installation_complete(&root).unwrap();
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
    artifact.mark_installation_complete(&root).unwrap();
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
fn malformed_completion_marker_is_never_bootable_and_valid_image_can_repair_it() {
    let temp = tempfile::tempdir().unwrap();
    let (artifact, archive) = fixture(temp.path(), "test-v1");
    let root = temp.path().join("runtime-B");
    artifact.extract(&archive, &root, &|_| {}).unwrap();
    fs::write(root.join(READY_MARKER), "not-a-marker\n").unwrap();
    assert!(!artifact.is_bootable(&root));
    artifact
        .provision(temp.path(), |_| panic!("validated image needs no download"))
        .unwrap();
    artifact.mark_installation_complete(&root).unwrap();
    assert!(artifact.is_bootable(&root));
}

#[test]
fn full_size_wrong_hash_and_overlong_partial_are_discarded_before_retry() {
    let temp = tempfile::tempdir().unwrap();
    let (mut artifact, archive) = fixture(temp.path(), "test-v1");
    let bytes = fs::read(&archive).unwrap();
    let mut wrong = bytes.clone();
    wrong[0] ^= 0xFF;
    fs::write(temp.path().join("portal-runtime.tar.xz"), wrong).unwrap();
    fs::write(
        temp.path().join(PARTIAL_ARCHIVE),
        vec![0u8; bytes.len() + 1],
    )
    .unwrap();
    artifact.url = "http://127.0.0.1:1/not-used".into();
    let error = artifact.provision(temp.path(), |_| {});
    assert!(error.is_err());
    assert!(!temp.path().join("portal-runtime.tar.xz").exists());
    assert!(!temp.path().join(PARTIAL_ARCHIVE).exists());
    assert!(!temp.path().join("runtime-B").exists());
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
    let root = temp.path().join("runtime-B");
    assert!(artifact.is_image_ready(&root));
    assert!(!artifact.is_bootable(&root));
    artifact.mark_installation_complete(&root).unwrap();
    assert!(artifact.is_bootable(&root));
    assert!(!staging.exists());
}

#[test]
fn fully_validated_staging_without_image_marker_is_recovered_without_redownload() {
    let temp = tempfile::tempdir().unwrap();
    let (artifact, archive) = fixture(temp.path(), "validated-staging-v1");
    let staging = temp.path().join("runtime-B.staging");
    artifact.extract(&archive, &staging, &|_| {}).unwrap();
    fs::remove_file(staging.join(IMAGE_READY_MARKER)).unwrap();

    artifact
        .provision(temp.path(), |message| {
            assert!(
                !message.to_ascii_lowercase().contains("download"),
                "validated staging must not download: {message}"
            );
        })
        .unwrap();

    let root = temp.path().join("runtime-B");
    assert!(artifact.is_image_ready(&root));
    assert!(!staging.exists());
}

#[test]
fn successful_provision_quarantines_unmarked_current_and_invalid_previous() {
    use std::io::Read;
    let temp = tempfile::tempdir().unwrap();
    let (mut artifact, archive) = fixture(temp.path(), "test-v1");
    let bytes = fs::read(&archive).unwrap();
    // Seed a live runtime plus a stale `.previous` slot, then remove the
    // live marker so provision must re-extract. The live tree is structurally
    // Debian-compatible but unmarked, so it must be quarantined rather than
    // promoted as a recovery runtime.
    // (The version string stays test-v1: the fixture bytes only validate as
    // test-v1.)
    artifact
        .extract(&archive, &temp.path().join("runtime-B"), &|_| {})
        .unwrap();
    artifact
        .mark_installation_complete(&temp.path().join("runtime-B"))
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
    // replacement is committed without trusting either unmarked/invalid tree.
    fs::remove_file(temp.path().join(format!("runtime-B/{READY_MARKER}"))).unwrap();
    fs::remove_file(temp.path().join(format!("runtime-B/{IMAGE_READY_MARKER}"))).unwrap();
    artifact.provision(temp.path(), |_| {}).unwrap();
    server.join().unwrap();
    let root = temp.path().join("runtime-B");
    assert!(artifact.is_image_ready(&root));
    artifact.mark_installation_complete(&root).unwrap();
    assert!(artifact.is_bootable(&root));
    assert!(!temp.path().join("runtime-B.previous").exists());
    let unknown = temp.path().join("runtime-B.unknown");
    assert!(unknown.exists());
    assert!(unknown.join("usr/bin/bash").exists());
    assert!(!unknown.join(READY_MARKER).exists());
}

#[test]
fn source_routes_only_release_image_and_preserves_session_handoff() {
    let setup = include_str!("../src/android/proot/setup.rs");
    let provisioning = include_str!("../src/core/provisioning.rs");
    let run = include_str!("../src/android/app/run.rs");
    let compose = include_str!("../src/android/kotlin/app/polarbear/ComposeOverlay.kt");
    let setup_screen = include_str!("../src/android/kotlin/app/polarbear/setup/PortalSetupScreen.kt");
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
    assert!(setup.contains("provision_with_progress"));
    assert!(setup.contains("mark_installation_complete"));
    assert!(setup.contains("build_committed_wayland_backend"));
    assert!(setup.contains("run_all_stages(stages(), &options, &registration)"));
    assert!(setup.contains("pub fn begin_install(plan_json: &str) -> bool"));
    assert!(setup.contains("persist_plan_before_start(Some(proposed), false)"));
    assert!(compose.contains("nativeBeginInstall"));
    assert!(compose.contains("updateInstallState"));
    assert!(compose.contains("dismissForRuntimeRecovery"));
    assert!(setup_screen.contains("ComposeOverlay.beginInstall(plan)"));
    assert!(setup_screen.contains("InstallPlan.fromSelections("));
    assert!(!setup_screen.contains("FAKE_INSTALL_DURATION_MS"));
    assert!(provisioning.contains("replace_atomic"));
    assert!(provisioning.contains("sync_parent_directory"));
    assert!(provisioning.contains("PREVIOUS_PENDING_PREFIX"));
    assert!(provisioning.contains("quarantine_path"));
    assert!(provisioning.contains("RuntimeClassification"));
    assert!(provisioning.contains("is_trusted_recovery"));
    assert!(provisioning.contains("UNKNOWN_RUNTIME_PREFIX"));
    assert!(provisioning.contains("PRESERVED_LEGACY_RUNTIME_PREFIX"));
    assert!(!provisioning.contains("is_known_valid_runtime"));
    assert!(provisioning.contains("self.validate_image(&staging).is_ok()"));
    assert!(run.contains("build_committed_wayland_backend"));
    assert!(run.contains("enter_committed_install_runtime_error"));
    assert!(run.contains("pending_runtime_error_page"));
    assert!(run.contains("dismiss_for_runtime_recovery"));
    let handoff = run
        .split("fn handle_setup_complete")
        .nth(1)
        .and_then(|source| source.split("impl ApplicationHandler").next())
        .expect("setup handoff function must remain present");
    assert!(
        !handoff.contains("proot::setup::setup("),
        "immediate first-install handoff must not replay setup stages"
    );
    assert!(!handoff.contains("run_all_stages"));
    assert!(handoff.contains("let resume_failed"));
    assert!(handoff.contains("!resume_wayland"));
    assert!(handoff.contains("enter_committed_install_runtime_error"));
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
fn production_runtime_artifact_matches_manifest_and_archive_verification() {    let artifact = RuntimeArtifact::production();
    assert_eq!(artifact.version, "debian13-arm64-2026.09.10.1");
    assert_eq!(
        artifact.sha256,
        "1e3fb4b38c5824ee98b840ffa1461726242efebaf1993200883e1c558208919f"
    );
    assert_eq!(artifact.compressed_bytes, 896140656);
    assert!(artifact.url.starts_with("https://github.com/Retrorerr/Portal/releases/download/runtime-debian13-arm64-2026.09.10.1/"));
    let archive_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/portal-debian13-arm64-2026.09.10.1.tar.xz");
    if archive_path.exists() {
        artifact
            .verify(&archive_path)
            .expect("RuntimeArtifact::verify failed on target archive");
    }
}

#[test]
fn runtime_manifest_provenance_is_optional_and_forward_compatible() {
    // Older manifests without `source_commit` must still parse, newer ones
    // with provenance (and unknown future fields) must be accepted: the
    // serde model has no `deny_unknown_fields` and `source_commit` defaults.
    let legacy: RuntimeArtifact = serde_json::from_str(
        r#"{"version":"v","url":"u","sha256":"s","compressed_bytes":1}"#,
    )
    .unwrap();
    assert_eq!(legacy.source_commit, None);
    let modern: RuntimeArtifact = serde_json::from_str(
        r#"{"version":"v","url":"u","sha256":"s","compressed_bytes":1,"source_commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","future_field":42}"#,
    )
    .unwrap();
    assert_eq!(
        modern.source_commit.as_deref(),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
}

#[test]
fn runtime_builder_pins_lfdevs_anland_stack_and_publish_validates_it() {
    // The canonical runtime must be Anland-capable: the builder hard-pins the
    // lfdevs KWin/XWayland bundle (URL + size + SHA-256, fail closed) and the
    // publisher refuses any non-legacy runtime whose KWin fell back to stock.
    let builder = include_str!("../scripts/build_debian_rootfs.py");
    for pin in [
        "kwin_anland-5.13-debian-4_6.3.6-95.zip",
        "10604606",
        "56ce1da27b640c977bad5ca0b7b13196e609b5fc419703429e51805ec05e4ee4",
        "xwayland_24.1.6-91_arm64.deb",
        "825848",
        "59f9c7486d6a10ad50a13622bf1d1bbf5accd015d630e4b2b0152a80577dcc64",
        "prepare_locked_packages_with_anland",
        "assert_overlay_payload_safe",
    ] {
        assert!(builder.contains(pin), "builder missing lfdevs pin/guard: {pin}");
    }
    let publisher = include_str!("../scripts/publish_runtime_release.py");
    for guard in [
        "ANLAND_KWIN_WAYLAND_SHA256",
        "4ad23a5aefbde02dae70ec270423b75205906be8ef8b0fd473fd32a11424bdbf",
        "ANLAND_LIBKWIN_BACKEND_MARKER",
        "ANLAND_XWAYLAND_SHA256",
        "3a25266671b7615740a7da602bd6a645bc8966be04d1a69c3536f09e67df2f87",
        "STOCK_FALLBACK_VERSIONS",
        "validate_anland_capable",
    ] {
        assert!(
            publisher.contains(guard),
            "publisher missing Anland fail-closed guard: {guard}"
        );
    }
}

