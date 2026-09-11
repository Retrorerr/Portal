//! Phase A failure-injection tests for the transactional Mesa KGSL layer.
//!
//! All state is built under tempdirs; the real production asset is never
//! touched.

use localdesktop::core::mesa_layer::{
    is_marker_valid, is_provisioned_at, layer_paths, marker_content, promote_with_validator,
    recover_interrupted, validate_layer_dir, write_marker_durable, Recovery, REQUIRED_DIRS,
    REQUIRED_REGULAR_FILES, REQUIRED_SONAME_LINKS,
};
use std::{fs, path::Path};

const VERSION: &str = "26.3.0-20260824";
const SHA: &str = "c014cf66bdbff96417ee30d34f006cf51df64ae04893d599711b0b6b73b52ccf";

fn build_valid_tree(root: &Path) {
    for rel in REQUIRED_REGULAR_FILES.iter().chain(REQUIRED_DIRS.iter()) {
        if *rel == "usr/lib/aarch64-linux-gnu/dri/kgsl_dri.so" {
            let dri = root.join("usr/lib/aarch64-linux-gnu/dri");
            fs::create_dir_all(&dri).unwrap();
            fs::write(dri.join("libdril_dri.so"), b"fake-dri").unwrap();
            #[cfg(unix)]
            {
                let _ = fs::remove_file(dri.join("kgsl_dri.so"));
                std::os::unix::fs::symlink("libdril_dri.so", dri.join("kgsl_dri.so")).unwrap();
            }
            #[cfg(not(unix))]
            {
                fs::write(dri.join("kgsl_dri.so"), b"fake-dri").unwrap();
            }
            continue;
        }
        let p = root.join(rel);
        if REQUIRED_DIRS.contains(rel) {
            fs::create_dir_all(&p).unwrap();
        } else {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"fake-so").unwrap();
        }
    }
    for (link, target) in REQUIRED_SONAME_LINKS {
        let lp = root.join(link);
        let _ = fs::remove_file(&lp);
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, &lp).unwrap();
        #[cfg(windows)]
        {
            // Windows CI: materialize as a copy (tolerated by validation).
            let sibling = lp.parent().unwrap().join(target);
            fs::write(&lp, fs::read(&sibling).unwrap()).unwrap();
        }
    }
    fs::create_dir_all(root.join("usr/lib/aarch64-linux-gnu/gbm")).unwrap();
    fs::write(
        root.join("usr/lib/aarch64-linux-gnu/gbm/dri_gbm.so"),
        b"backend",
    )
    .unwrap();
    fs::create_dir_all(root.join("usr/share/vulkan/icd.d")).unwrap();
    fs::write(
        root.join("usr/share/vulkan/icd.d/freedreno_icd.aarch64.json"),
        r#"{"file_format_version": "1.0.0", "ICD": {"library_path": "libvulkan_freedreno.so"}}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("usr/share/drirc.d")).unwrap();
}

fn mark(base: &Path) {
    let paths = layer_paths(base);
    write_marker_durable(&paths.marker, VERSION, SHA).unwrap();
}

#[test]
fn valid_target_no_staging_is_provisioned() {
    let t = tempfile::tempdir().unwrap();
    let paths = layer_paths(t.path());
    build_valid_tree(&paths.target);
    mark(t.path());
    assert!(is_provisioned_at(t.path(), VERSION, SHA));
    assert!(validate_layer_dir(&paths.target).is_ok());
    // Recovery must be a no-op and must not delete the good tree.
    assert_eq!(
        recover_interrupted(t.path(), VERSION, SHA).unwrap(),
        Recovery::None
    );
    assert!(paths.target.is_dir());
}

#[test]
fn valid_target_corrupt_staging_keeps_good_tree() {
    let t = tempfile::tempdir().unwrap();
    let paths = layer_paths(t.path());
    build_valid_tree(&paths.target);
    mark(t.path());
    fs::create_dir_all(&paths.staging).unwrap();
    fs::write(paths.staging.join("partial"), b"junk").unwrap();
    assert!(is_provisioned_at(t.path(), VERSION, SHA));
    recover_interrupted(t.path(), VERSION, SHA).unwrap();
    // Good tree survives; corrupt staging is reclaimed.
    assert!(validate_layer_dir(&paths.target).is_ok());
    assert!(is_provisioned_at(t.path(), VERSION, SHA));
    assert!(!paths.staging.join("partial").exists());
}

#[test]
fn missing_target_valid_staging_and_previous_promotes_staging() {
    let t = tempfile::tempdir().unwrap();
    let paths = layer_paths(t.path());
    build_valid_tree(&paths.previous);
    build_valid_tree(&paths.staging);
    assert!(!paths.target.exists());
    assert_eq!(
        recover_interrupted(t.path(), VERSION, SHA).unwrap(),
        Recovery::PromotedStaging
    );
    assert!(validate_layer_dir(&paths.target).is_ok());
    assert!(is_provisioned_at(t.path(), VERSION, SHA));
}

#[test]
fn missing_target_corrupt_staging_restores_previous() {
    let t = tempfile::tempdir().unwrap();
    let paths = layer_paths(t.path());
    build_valid_tree(&paths.previous);
    fs::create_dir_all(&paths.staging).unwrap();
    fs::write(paths.staging.join("junk"), b"x").unwrap();
    assert_eq!(
        recover_interrupted(t.path(), VERSION, SHA).unwrap(),
        Recovery::RestoredPrevious
    );
    assert!(validate_layer_dir(&paths.target).is_ok());
    assert!(is_provisioned_at(t.path(), VERSION, SHA));
    assert!(!paths.previous.exists());
}

#[test]
fn valid_target_stale_marker_is_repaired_without_download() {
    let t = tempfile::tempdir().unwrap();
    let paths = layer_paths(t.path());
    build_valid_tree(&paths.target);
    fs::write(&paths.marker, "stale\n").unwrap();
    assert!(!is_provisioned_at(t.path(), VERSION, SHA));
    assert_eq!(
        recover_interrupted(t.path(), VERSION, SHA).unwrap(),
        Recovery::RepairedMarker
    );
    assert!(is_provisioned_at(t.path(), VERSION, SHA));
    assert_eq!(
        fs::read_to_string(&paths.marker).unwrap(),
        marker_content(VERSION, SHA)
    );
}

#[test]
fn valid_target_missing_marker_is_repaired() {
    let t = tempfile::tempdir().unwrap();
    let paths = layer_paths(t.path());
    build_valid_tree(&paths.target);
    assert!(!paths.marker.exists());
    assert_eq!(
        recover_interrupted(t.path(), VERSION, SHA).unwrap(),
        Recovery::RepairedMarker
    );
    assert!(is_provisioned_at(t.path(), VERSION, SHA));
}

#[test]
fn marker_valid_but_critical_file_missing_is_not_provisioned() {
    let t = tempfile::tempdir().unwrap();
    let paths = layer_paths(t.path());
    build_valid_tree(&paths.target);
    mark(t.path());
    assert!(is_provisioned_at(t.path(), VERSION, SHA));
    fs::remove_file(
        paths
            .target
            .join("usr/lib/aarch64-linux-gnu/dri/kgsl_dri.so"),
    )
    .unwrap();
    assert!(!is_marker_valid(&paths.marker, VERSION, "wrong"));
    // Marker still matches version+sha, but the tree is incomplete.
    assert!(is_marker_valid(&paths.marker, VERSION, SHA));
    assert!(!is_provisioned_at(t.path(), VERSION, SHA));
}

#[test]
fn interrupted_promotion_target_missing_is_recoverable() {
    let t = tempfile::tempdir().unwrap();
    let paths = layer_paths(t.path());
    // Crash after `target -> previous` but before `staging -> target`.
    build_valid_tree(&paths.previous);
    build_valid_tree(&paths.staging);
    assert!(!paths.target.exists());
    recover_interrupted(t.path(), VERSION, SHA).unwrap();
    assert!(validate_layer_dir(&paths.target).is_ok());
    assert!(is_provisioned_at(t.path(), VERSION, SHA));
}

#[test]
fn failed_validation_after_promotion_restores_previous() {
    let t = tempfile::tempdir().unwrap();
    let paths = layer_paths(t.path());
    build_valid_tree(&paths.target);
    mark(t.path());
    build_valid_tree(&paths.staging);
    let before = fs::read(
        paths
            .target
            .join("usr/lib/aarch64-linux-gnu/libgbm.so.1.0.0"),
    )
    .unwrap();
    // First validation (staging) passes, second (existing target check)
    // passes, third (promoted target) fails.
    let mut calls = 0;
    let result = promote_with_validator(t.path(), VERSION, SHA, |p| {
        calls += 1;
        if calls <= 2 {
            validate_layer_dir(p)
        } else {
            anyhow::bail!("injected post-promotion validation failure")
        }
    });
    assert!(result.is_err());
    // Last known-good tree is restored, no valid marker is left behind.
    assert!(validate_layer_dir(&paths.target).is_ok());
    assert_eq!(
        fs::read(
            paths
                .target
                .join("usr/lib/aarch64-linux-gnu/libgbm.so.1.0.0")
        )
        .unwrap(),
        before
    );
    assert!(!is_provisioned_at(t.path(), VERSION, SHA));
}

#[test]
fn corrupt_staging_never_replaces_good_target() {
    let t = tempfile::tempdir().unwrap();
    let paths = layer_paths(t.path());
    build_valid_tree(&paths.target);
    mark(t.path());
    fs::create_dir_all(&paths.staging).unwrap();
    fs::write(paths.staging.join("junk"), b"bad").unwrap();
    let result = promote_with_validator(t.path(), VERSION, SHA, validate_layer_dir);
    assert!(result.is_err());
    assert!(validate_layer_dir(&paths.target).is_ok());
    assert!(is_provisioned_at(t.path(), VERSION, SHA));
}

#[test]
fn successful_promotion_parks_and_drops_previous_only_after_proof() {
    let t = tempfile::tempdir().unwrap();
    let paths = layer_paths(t.path());
    build_valid_tree(&paths.target);
    mark(t.path());
    build_valid_tree(&paths.staging);
    // Distinguish staging content so we know promotion happened.
    fs::write(
        paths
            .staging
            .join("usr/lib/aarch64-linux-gnu/libgbm.so.1.0.0"),
        b"new-bytes",
    )
    .unwrap();
    promote_with_validator(t.path(), VERSION, SHA, validate_layer_dir).unwrap();
    assert_eq!(
        fs::read(
            paths
                .target
                .join("usr/lib/aarch64-linux-gnu/libgbm.so.1.0.0")
        )
        .unwrap(),
        b"new-bytes"
    );
    assert!(is_provisioned_at(t.path(), VERSION, SHA));
    assert!(!paths.previous.exists());
    assert!(!paths.staging.exists());
}

#[test]
fn validation_covers_session_binds_runtime_chain() {
    // The authoritative list must stay identical to what session_binds
    // mounts, or the layer silently stops engaging.
    let binds = include_str!("../src/android/anland/mod.rs");
    for required in [
        "libgallium-26.3.0-devel.so",
        "libvulkan_freedreno.so",
        "libEGL_mesa.so.0",
        "libGLX_mesa.so.0",
        "libgbm.so.1",
        "vulkan/icd.d",
    ] {
        assert!(binds.contains(required), "session_binds missing {required}");
    }
    let core_src = include_str!("../src/core/mesa_layer.rs");
    for required in [
        "kgsl_dri.so",
        "libgallium-26.3.0-devel.so",
        "libvulkan_freedreno.so",
        "libEGL_mesa.so.0",
        "libGLX_mesa.so.0",
        "libgbm.so.1",
        "freedreno",
    ] {
        assert!(
            core_src.contains(required),
            "core validation missing {required}"
        );
    }
    // Pinned identity must not drift.
    let android_src = include_str!("../src/android/proot/mesa_layer.rs");
    for pin in [
        "26.3.0-20260824",
        "11648933",
        "c014cf66bdbff96417ee30d34f006cf51df64ae04893d599711b0b6b73b52ccf",
        "mesa-kgsl-layer.complete",
    ] {
        assert!(android_src.contains(pin), "mesa pin missing: {pin}");
    }
}

#[test]
fn setup_defers_mesa_to_spawned_stage_and_plasma_never_downloads_inline() {
    let setup = include_str!("../src/android/proot/setup.rs");
    // Dedicated stage exists and is ordered before plasma-wayland.
    assert!(setup.contains("fn setup_mesa_layer"));
    assert!(setup.contains("\"mesa-kgsl-layer\""));
    assert!(setup.contains("Box::new(setup_mesa_layer)"));
    let mesa_stage = setup.find("\"mesa-kgsl-layer\"").unwrap();
    let plasma_stage = setup.find("\"plasma-wayland\"").unwrap();
    assert!(mesa_stage < plasma_stage);
    // Heavy work is spawned; progress is reported.
    let mesa_fn = setup.find("fn setup_mesa_layer").unwrap();
    let plasma_fn = setup.find("fn setup_plasma_wayland").unwrap();
    let mesa_body = &setup[mesa_fn..plasma_fn];
    assert!(mesa_body.contains("thread::spawn"));
    assert!(mesa_body.contains("provision_with_progress"));
    assert!(mesa_body.contains("SetupMessage::Progress"));
    assert!(mesa_body.contains("is_anland_requested"));
    // Plasma setup must not download inline anymore.
    let plasma_body = &setup[plasma_fn..plasma_fn + 3000.min(setup.len() - plasma_fn)];
    assert!(!plasma_body.contains("mesa_layer::provision()"));
    assert!(!plasma_body.contains("provision_with_progress"));
    assert!(plasma_body.contains("is_provisioned"));
}
