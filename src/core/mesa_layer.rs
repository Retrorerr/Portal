//! Host-testable Mesa KGSL layer validation + transactional promotion.
//!
//! The Android wrapper (`src/android/proot/mesa_layer.rs`) owns the network
//! fetch and the pinned version constants. Everything here is pure
//! filesystem logic operating on an explicit base directory so unit and
//! integration tests can exercise crash windows with tempdirs without
//! touching the real production asset.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// Files that must exist as regular files inside a provisioned layer.
/// Derived from `session_binds()` in `src/android/anland/mod.rs`: the DRI
/// driver, gallium core, Vulkan driver, and the real-name EGL/GLX/GBM
/// libraries. SONAME symlinks are validated separately.
pub const REQUIRED_REGULAR_FILES: &[&str] = &[
    "usr/lib/aarch64-linux-gnu/dri/kgsl_dri.so",
    "usr/lib/aarch64-linux-gnu/libgallium-26.3.0-devel.so",
    "usr/lib/aarch64-linux-gnu/libvulkan_freedreno.so",
    "usr/lib/aarch64-linux-gnu/libEGL_mesa.so.0.0.0",
    "usr/lib/aarch64-linux-gnu/libGLX_mesa.so.0.0.0",
    "usr/lib/aarch64-linux-gnu/libgbm.so.1.0.0",
];

/// SONAME entries the loader resolves by name. The second element is the
/// expected real-name target inside the same directory. Accept a symlink
/// resolving within the layer; a plain regular file is tolerated (some tar
/// materializations) but a missing/broken/out-of-layer link is rejected.
pub const REQUIRED_SONAME_LINKS: &[(&str, &str)] = &[
    (
        "usr/lib/aarch64-linux-gnu/libEGL_mesa.so.0",
        "libEGL_mesa.so.0.0.0",
    ),
    (
        "usr/lib/aarch64-linux-gnu/libGLX_mesa.so.0",
        "libGLX_mesa.so.0.0.0",
    ),
    ("usr/lib/aarch64-linux-gnu/libgbm.so.1", "libgbm.so.1.0.0"),
];

/// Directories that must exist because `session_binds()` mounts them.
pub const REQUIRED_DIRS: &[&str] = &[
    "usr/lib/aarch64-linux-gnu/dri",
    "usr/lib/aarch64-linux-gnu/gbm",
    "usr/share/vulkan/icd.d",
    "usr/share/drirc.d",
];

/// Filesystem locations for one Mesa base directory.
#[derive(Debug, Clone)]
pub struct LayerPaths {
    pub target: PathBuf,
    pub staging: PathBuf,
    pub previous: PathBuf,
    pub archive: PathBuf,
    pub marker: PathBuf,
}

/// Resolve the five transaction paths for a given files root.
pub fn layer_paths(base: &Path) -> LayerPaths {
    LayerPaths {
        target: base.join("mesa-kgsl-layer"),
        staging: base.join("mesa-kgsl-layer.staging"),
        previous: base.join("mesa-kgsl-layer.previous"),
        archive: base.join("mesa-kgsl-layer.tar.gz"),
        marker: base.join("mesa-kgsl-layer.complete"),
    }
}

/// Exact completion marker content for a version + SHA pair.
pub fn marker_content(version: &str, sha256: &str) -> String {
    format!("{version}\n{sha256}\n")
}

/// True when the marker file exists with the exact expected content.
pub fn is_marker_valid(marker_path: &Path, version: &str, sha256: &str) -> bool {
    fs::read_to_string(marker_path)
        .map(|s| s == marker_content(version, sha256))
        .unwrap_or(false)
}

/// Authoritative layer validation. Cheap: existence + file-type + symlink
/// shape + minimal content presence. No full-tree hashing.
pub fn validate_layer_dir(layer: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        layer.is_dir(),
        "mesa layer missing directory: {}",
        layer.display()
    );
    for rel in REQUIRED_REGULAR_FILES {
        let path = layer.join(rel);
        let meta = fs::symlink_metadata(&path)
            .map_err(|_| anyhow::anyhow!("mesa layer missing required file {rel}"))?;
        anyhow::ensure!(
            meta.file_type().is_file(),
            "mesa layer entry is not a regular file: {rel}"
        );
    }
    for (link_rel, expected_target) in REQUIRED_SONAME_LINKS {
        let link_path = layer.join(link_rel);
        let meta = fs::symlink_metadata(&link_path)
            .map_err(|_| anyhow::anyhow!("mesa layer missing required SONAME {link_rel}"))?;
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&link_path)
                .map_err(|e| anyhow::anyhow!("mesa layer broken SONAME link {link_rel}: {e}"))?;
            let resolved = if target.is_absolute() {
                PathBuf::from(&target)
            } else {
                link_path
                    .parent()
                    .unwrap_or_else(|| Path::new("/"))
                    .join(&target)
            };
            // Resolve lexically and require containment within the layer.
            let layer_canon = normalize_lexically(layer);
            let resolved_canon = normalize_lexically(&resolved);
            anyhow::ensure!(
                resolved_canon.starts_with(&layer_canon),
                "mesa layer SONAME {link_rel} escapes layer: {}",
                target.display()
            );
            // Expected shape: points at the real-name sibling where practical.
            if let Some(file_name) = target.file_name().and_then(|s| s.to_str()) {
                anyhow::ensure!(
                    file_name == *expected_target,
                    "mesa layer SONAME {link_rel} points at {file_name}, expected {expected_target}"
                );
            }
            anyhow::ensure!(
                resolved.is_file(),
                "mesa layer SONAME {link_rel} target is not a file"
            );
        } else if meta.file_type().is_file() {
            // Tolerated: materialized as a regular file copy.
        } else {
            anyhow::bail!("mesa layer SONAME {link_rel} is not a file or symlink");
        }
    }
    for rel in REQUIRED_DIRS {
        let path = layer.join(rel);
        anyhow::ensure!(
            path.is_dir()
                && !fs::symlink_metadata(&path)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(true),
            "mesa layer missing required directory {rel}"
        );
    }
    // GBM backend content: at least one shared object inside gbm/.
    let gbm_dir = layer.join("usr/lib/aarch64-linux-gnu/gbm");
    let mut gbm_entries = 0usize;
    let mut gbm_has_so = false;
    for entry in fs::read_dir(&gbm_dir)
        .map_err(|e| anyhow::anyhow!("mesa layer cannot list gbm backends: {e}"))?
    {
        let entry = entry.map_err(|e| anyhow::anyhow!("mesa layer gbm entry unreadable: {e}"))?;
        gbm_entries += 1;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.contains(".so"))
        {
            gbm_has_so = true;
        }
    }
    anyhow::ensure!(gbm_entries > 0, "mesa layer gbm backend directory is empty");
    anyhow::ensure!(
        gbm_has_so,
        "mesa layer gbm backend directory has no shared object"
    );
    // Vulkan ICD: at least one JSON, at least one referencing freedreno.
    let icd_dir = layer.join("usr/share/vulkan/icd.d");
    let mut json_count = 0usize;
    let mut freedreno_ref = false;
    for entry in fs::read_dir(&icd_dir)
        .map_err(|e| anyhow::anyhow!("mesa layer cannot list Vulkan ICDs: {e}"))?
    {
        let entry = entry.map_err(|e| anyhow::anyhow!("mesa layer ICD entry unreadable: {e}"))?;
        let name = entry.file_name();
        let name_str = name.to_str().unwrap_or_default().to_owned();
        if !name_str.ends_with(".json") {
            continue;
        }
        json_count += 1;
        if let Ok(text) = fs::read_to_string(entry.path()) {
            if text.to_ascii_lowercase().contains("freedreno") {
                freedreno_ref = true;
            }
        }
    }
    anyhow::ensure!(
        json_count > 0,
        "mesa layer Vulkan ICD directory has no JSON"
    );
    anyhow::ensure!(
        freedreno_ref,
        "mesa layer Vulkan ICD has no freedreno reference"
    );
    Ok(())
}

/// Lexically normalize a path (resolve `.`/`..` without touching the FS).
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        use std::path::Component::*;
        match comp {
            CurDir => {}
            ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// True when marker matches exactly AND the target tree validates.
pub fn is_provisioned_at(base: &Path, version: &str, sha256: &str) -> bool {
    let paths = layer_paths(base);
    is_marker_valid(&paths.marker, version, sha256) && validate_layer_dir(&paths.target).is_ok()
}

/// Best-effort fsync of a file.
pub fn sync_file(path: &Path) {
    if let Ok(f) = fs::File::open(path) {
        let _ = f.sync_all();
    }
}

/// Best-effort fsync of a directory (durability of renames).
pub fn sync_dir(path: &Path) {
    if let Ok(f) = fs::File::open(path) {
        let _ = f.sync_all();
    }
}

/// Durably write the completion marker (temp + rename + sync).
pub fn write_marker_durable(marker_path: &Path, version: &str, sha256: &str) -> anyhow::Result<()> {
    if let Some(parent) = marker_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = marker_path.with_extension("complete.tmp");
    fs::write(&tmp, marker_content(version, sha256))?;
    sync_file(&tmp);
    fs::rename(&tmp, marker_path)?;
    sync_file(marker_path);
    if let Some(parent) = marker_path.parent() {
        sync_dir(parent);
    }
    Ok(())
}

/// Outcome of crash recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// Nothing to do.
    None,
    /// Valid staging was promoted to target.
    PromotedStaging,
    /// Valid previous was restored to target.
    RestoredPrevious,
    /// Stale marker was repaired because the target already validates.
    RepairedMarker,
    /// Stale staging/previous cleaned; a fresh download is needed.
    NeedsProvision,
}

/// Recover an interrupted promotion (crash between `target -> previous`
/// and `staging -> target`).
///
/// - target missing + staging valid -> promote staging.
/// - target missing + staging invalid + previous valid -> restore previous.
/// - target valid + marker stale -> repair marker (no download).
/// - otherwise clean disposable staging and report.
pub fn recover_interrupted(base: &Path, version: &str, sha256: &str) -> anyhow::Result<Recovery> {
    let paths = layer_paths(base);
    let target_valid = validate_layer_dir(&paths.target).is_ok();
    let staging_valid = validate_layer_dir(&paths.staging).is_ok();
    let previous_valid = validate_layer_dir(&paths.previous).is_ok();
    let marker_valid = is_marker_valid(&paths.marker, version, sha256);

    let target_exists = fs::symlink_metadata(&paths.target).is_ok();

    if !target_exists {
        if staging_valid {
            fs::rename(&paths.staging, &paths.target)
                .map_err(|e| anyhow::anyhow!("recovery: cannot promote staging: {e}"))?;
            validate_layer_dir(&paths.target).map_err(|e| {
                anyhow::anyhow!("recovery: promoted staging failed validation: {e}")
            })?;
            write_marker_durable(&paths.marker, version, sha256)?;
            sync_dir(base);
            if previous_valid {
                let _ = fs::remove_dir_all(&paths.previous);
            }
            return Ok(Recovery::PromotedStaging);
        }
        if previous_valid {
            if paths.staging.exists() {
                let _ = fs::remove_dir_all(&paths.staging);
            }
            fs::rename(&paths.previous, &paths.target)
                .map_err(|e| anyhow::anyhow!("recovery: cannot restore previous: {e}"))?;
            validate_layer_dir(&paths.target).map_err(|e| {
                anyhow::anyhow!("recovery: restored previous failed validation: {e}")
            })?;
            write_marker_durable(&paths.marker, version, sha256)?;
            sync_dir(base);
            return Ok(Recovery::RestoredPrevious);
        }
        // No valid tree anywhere: drop invalid staging so the next
        // provision starts fresh, but never claim completion.
        if fs::symlink_metadata(&paths.staging).is_ok() {
            let _ = fs::remove_dir_all(&paths.staging);
        }
        if !marker_valid {
            let _ = fs::remove_file(&paths.marker);
        }
        return Ok(Recovery::NeedsProvision);
    }

    // Target exists.
    if target_valid {
        // Discard corrupt staging; it must never replace a good tree.
        if fs::symlink_metadata(&paths.staging).is_ok() && !staging_valid {
            let _ = fs::remove_dir_all(&paths.staging);
        }
        if !marker_valid {
            write_marker_durable(&paths.marker, version, sha256)?;
            return Ok(Recovery::RepairedMarker);
        }
        return Ok(Recovery::None);
    }

    // Target exists but is invalid: do not touch previous here; the
    // promotion path decides. Just report that provisioning is needed.
    Ok(Recovery::NeedsProvision)
}

/// Promote an already-extracted, already-validated staging tree.
///
/// Uses `validator` for both the pre-promotion and post-promotion checks
/// so tests can inject post-promotion failure. Production passes
/// `validate_layer_dir`.
pub fn promote_with_validator(
    base: &Path,
    version: &str,
    sha256: &str,
    mut validator: impl FnMut(&Path) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let paths = layer_paths(base);

    // 4. Fully validate staging BEFORE touching the working tree or the
    // marker, so corrupt staging never invalidates a good installation.
    validator(&paths.staging)
        .map_err(|e| anyhow::anyhow!("mesa promotion: staging failed validation: {e}"))?;

    // 1. Invalidate the completion marker BEFORE modifying the working
    // target so a crash can never leave a valid marker over a
    // half-replaced tree.
    match fs::remove_file(&paths.marker) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => anyhow::bail!("mesa promotion: cannot invalidate marker: {e}"),
    }

    let target_exists = fs::symlink_metadata(&paths.target).is_ok();
    let target_valid = validator(&paths.target).is_ok();
    let mut moved_to_previous = false;

    if target_exists && target_valid {
        // 5. Preserve last known-good before replacing.
        if fs::symlink_metadata(&paths.previous).is_ok() {
            fs::remove_dir_all(&paths.previous)
                .map_err(|e| anyhow::anyhow!("mesa promotion: cannot clear previous: {e}"))?;
        }
        fs::rename(&paths.target, &paths.previous)
            .map_err(|e| anyhow::anyhow!("mesa promotion: cannot park current target: {e}"))?;
        moved_to_previous = true;
    } else if target_exists {
        // Corrupt target is not worth preserving; make room for staging.
        // Never delete a valid previous here: it may be the last good tree.
        fs::remove_dir_all(&paths.target)
            .map_err(|e| anyhow::anyhow!("mesa promotion: cannot remove corrupt target: {e}"))?;
    }

    // 6. Promote staging.
    if let Err(e) = fs::rename(&paths.staging, &paths.target) {
        // Restore the parked good tree if we moved it.
        if moved_to_previous {
            let _ = fs::rename(&paths.previous, &paths.target);
        }
        anyhow::bail!("mesa promotion: staging rename failed: {e}");
    }

    // 7. Validate the promoted target AGAIN.
    if let Err(e) = validator(&paths.target) {
        // Roll back: drop the bad promotion, restore previous.
        let _ = fs::remove_dir_all(&paths.target);
        if moved_to_previous {
            let _ = fs::rename(&paths.previous, &paths.target);
        }
        anyhow::bail!("mesa promotion: promoted target failed validation: {e}");
    }

    // 8-9. Only now claim completion, durably.
    write_marker_durable(&paths.marker, version, sha256)?;
    sync_file(
        &paths
            .target
            .join("usr/lib/aarch64-linux-gnu/dri/kgsl_dri.so"),
    );
    sync_dir(&paths.target);
    sync_dir(base);

    // 10. Drop the parked tree only after the new target + marker prove out.
    if moved_to_previous {
        let _ = fs::remove_dir_all(&paths.previous);
    }
    Ok(())
}

/// Production promotion entry point.
pub fn promote_staged(base: &Path, version: &str, sha256: &str) -> anyhow::Result<()> {
    promote_with_validator(base, version, sha256, validate_layer_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: &str = "26.3.0-test";
    const S: &str = "abc123";

    #[cfg(unix)]
    fn make_soname(link: &Path, target: &str) {
        use std::os::unix::fs::symlink;
        let _ = fs::remove_file(link);
        symlink(target, link).unwrap();
    }

    #[cfg(not(unix))]
    fn make_soname(link: &Path, target: &str) {
        // Windows CI lacks symlink privilege: materialize a copy, which
        // validation tolerates as a regular file.
        let sibling = link.parent().unwrap().join(target);
        let bytes = fs::read(&sibling).unwrap_or_else(|_| b"fake-so".to_vec());
        let _ = fs::remove_file(link);
        fs::write(link, bytes).unwrap();
    }

    fn valid_tree(base: &Path, root_name: &str) {
        let root = base.join(root_name);
        for rel in REQUIRED_REGULAR_FILES
            .iter()
            .chain(REQUIRED_DIRS.iter())
            .chain(REQUIRED_SONAME_LINKS.iter().map(|(l, _)| l))
        {
            // SONAME entries are created as links below.
            if REQUIRED_SONAME_LINKS.iter().any(|(l, _)| l == rel) {
                continue;
            }
            let p = root.join(rel);
            if rel.ends_with(".so")
                || rel.ends_with(".so.0.0.0")
                || rel.ends_with(".so.1.0.0")
                || rel.contains("kgsl_dri")
                || rel.contains("libgallium")
                || rel.contains("libvulkan")
            {
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(&p, b"fake-so").unwrap();
            } else {
                fs::create_dir_all(&p).unwrap();
            }
        }
        for (link, target) in REQUIRED_SONAME_LINKS {
            let lp = root.join(link);
            let _ = fs::remove_file(&lp);
            make_soname(&lp, target);
        }
        fs::write(
            root.join("usr/lib/aarch64-linux-gnu/gbm/dri_gbm.so"),
            b"backend",
        )
        .unwrap();
        fs::write(
            root.join("usr/share/vulkan/icd.d/freedreno_icd.aarch64.json"),
            r#"{"ICD": {"library_path": "libvulkan_freedreno.so"}}"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("usr/share/drirc.d")).unwrap();
    }

    #[test]
    fn validation_accepts_complete_tree() {
        let t = tempfile::tempdir().unwrap();
        valid_tree(t.path(), "mesa-kgsl-layer");
        validate_layer_dir(&t.path().join("mesa-kgsl-layer")).unwrap();
    }

    #[test]
    fn validation_rejects_missing_critical_file() {
        let t = tempfile::tempdir().unwrap();
        valid_tree(t.path(), "mesa-kgsl-layer");
        fs::remove_file(
            t.path()
                .join("mesa-kgsl-layer/usr/lib/aarch64-linux-gnu/libgbm.so.1.0.0"),
        )
        .unwrap();
        assert!(validate_layer_dir(&t.path().join("mesa-kgsl-layer")).is_err());
    }

    #[test]
    fn validation_rejects_broken_soname() {
        let t = tempfile::tempdir().unwrap();
        valid_tree(t.path(), "mesa-kgsl-layer");
        let link = t
            .path()
            .join("mesa-kgsl-layer/usr/lib/aarch64-linux-gnu/libEGL_mesa.so.0");
        fs::remove_file(&link).unwrap();
        #[cfg(unix)]
        {
            make_soname(&link, "libEGL_mesa.so.9.9.9");
        }
        // On Windows symlinks are materialized as copies (tolerated), so a
        // missing SONAME is the meaningful failure there.
        assert!(validate_layer_dir(&t.path().join("mesa-kgsl-layer")).is_err());
    }

    #[test]
    fn validation_rejects_empty_gbm_and_missing_icd() {
        let t = tempfile::tempdir().unwrap();
        valid_tree(t.path(), "mesa-kgsl-layer");
        fs::remove_file(
            t.path()
                .join("mesa-kgsl-layer/usr/lib/aarch64-linux-gnu/gbm/dri_gbm.so"),
        )
        .unwrap();
        assert!(validate_layer_dir(&t.path().join("mesa-kgsl-layer")).is_err());
        valid_tree(t.path(), "mesa-kgsl-layer2");
        fs::remove_file(
            t.path()
                .join("mesa-kgsl-layer2/usr/share/vulkan/icd.d/freedreno_icd.aarch64.json"),
        )
        .unwrap();
        assert!(validate_layer_dir(&t.path().join("mesa-kgsl-layer2")).is_err());
    }
}
