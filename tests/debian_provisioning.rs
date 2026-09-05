#[path = "../src/core/provisioning.rs"]
mod provisioning;
#[path = "../src/core/runtime.rs"]
mod runtime;
use provisioning::{RuntimeArtifact, IMAGE_MARKER, READY_MARKER};
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::Path};

fn fixture(directory: &Path, version: &str) -> (RuntimeArtifact, std::path::PathBuf) {
    let archive = directory.join("image.xz");
    let encoder = xz2::write::XzEncoder::new(fs::File::create(&archive).unwrap(), 1);
    let mut tar = tar::Builder::new(encoder);
    for (path, value) in [(IMAGE_MARKER, version), ("usr/lib/os-release", "ID=debian\nVERSION_ID=\"13\"\n"),
        ("usr/bin/dpkg", "binary"), ("usr/bin/apt", "binary"), ("usr/bin/bash", "binary"),
        ("usr/bin/kwin_wayland", "binary"), ("usr/bin/plasmashell", "binary"),
        ("usr/bin/python3", "binary"), ("var/lib/dpkg/status", "Package: dpkg\n")] {
        let mut header = tar::Header::new_gnu();
        header.set_size(value.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, path, value.as_bytes()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
    let bytes = fs::read(&archive).unwrap();
    (RuntimeArtifact { version: version.into(), url: "http://127.0.0.1:1/not-used".into(),
        sha256: format!("{:x}", Sha256::digest(&bytes)), compressed_bytes: bytes.len() as u64 }, archive)
}

#[test]
fn clean_and_legacy_selection_are_always_debian() {
    let temp = tempfile::tempdir().unwrap();
    let layout = runtime::RuntimeLayout::new(temp.path());
    assert_eq!(layout.active_slot().rootfs_path, temp.path().join("runtime-B"));
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
    artifact.provision(temp.path(), |_| panic!("Ready runtime must not download")).unwrap();
    assert_eq!(fs::read_to_string(root.join("user-file")).unwrap(), "keep");
}

#[test]
fn rejects_wrong_version_and_corrupt_download_without_completion_marker() {
    let temp = tempfile::tempdir().unwrap();
    let (mut artifact, archive) = fixture(temp.path(), "test-v1");
    let stage = temp.path().join("staging");
    artifact.version = "test-v2".into();
    assert!(artifact.extract(&archive, &stage, &|_| {}).is_err());
    assert!(!stage.join(READY_MARKER).exists());
    fs::OpenOptions::new().append(true).open(&archive).unwrap().write_all(b"corrupt").unwrap();
    assert!(artifact.verify(&archive).is_err());
}

#[test]
fn source_routes_only_release_image_and_preserves_session_handoff() {
    let setup = include_str!("../src/android/proot/setup.rs");
    let config_full = include_str!("../src/core/config.rs");
    // Production defaults live above the unit-test module; legacy migration
    // fixtures below `#[cfg(test)]` intentionally mention old managers.
    let config = config_full.split("mod tests {").next().unwrap_or(config_full);
    for removed in ["setup_arch_fs", "ARCH_FS_ARCHIVE", "pacman", "install_dependencies"] {
        assert!(!setup.contains(removed), "{removed} remains in setup");
        assert!(!config.contains(removed), "{removed} remains in defaults");
    }
    assert!(setup.contains("Box::new(setup_debian_runtime)"));
    assert!(setup.contains("sync_session_runtime_files(fs_root, ui_scale)"));
    for function in ["sync_android_timezone(fs_root)", "sync_guest_network_config(fs_root)",
                     "sync_firefox_config(fs_root)", "PORTAL_IME_BRIDGE"] {
        assert!(setup.contains(function));
    }
    assert!(setup.contains("on_complete();"));
    let lifecycle = include_str!("../src/android/app/build.rs");
    assert!(lifecycle.contains("webview_handoff::complete_setup"));
    assert!(include_str!("../src/android/proot/launch.rs").contains("RuntimeArtifact::production().is_ready"));
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
