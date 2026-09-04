use crate::core::runtime::LinuxRuntime;

/// Persist host window geometry for guest-side labwc autostart (`localdesktop-wlroots-output`).
///
/// The file lives in the proot-visible `/tmp` directory so scripts running inside the
/// Xfce/labwc session can align wlroots output mode/scale with the Android winit window.
pub fn write_guest_output_state(width: i32, height: i32, scale: i32) {
    if width <= 0 || height <= 0 || scale <= 0 {
        return;
    }

    let runtime = crate::android::runtime::proot::PRootRuntime::active();
    let path = runtime.rootfs_path().join("tmp/localdesktop-output");
    let content =
        format!("LOCALDESKTOP_OUTPUT_MODE={width}x{height}\nLOCALDESKTOP_OUTPUT_SCALE={scale}\n");
    if let Err(error) = std::fs::write(&path, content) {
        log::warn!(
            "Failed to write guest output state to {}: {error}",
            path.display()
        );
    }
}

/// Read the active KWin output scale factor from the guest configuration (`kwinoutputconfig.json`).
/// Returns `(scale, mtime_ns)` so callers can cache and avoid per-frame JSON parsing.
pub fn read_kwin_output_scale_with_mtime() -> Option<(f64, u64)> {
    let runtime = crate::android::runtime::proot::PRootRuntime::active();
    let candidate_paths = [
        runtime
            .rootfs_path()
            .join("root/.config/kwinoutputconfig.json"),
        std::path::PathBuf::from(
            "/data/data/app.polarbear/files/runtime-B/root/.config/kwinoutputconfig.json",
        ),
        std::path::PathBuf::from(
            "/data/data/app.polarbear/files/arch/root/.config/kwinoutputconfig.json",
        ),
    ];

    for path in &candidate_paths {
        let mtime_ns = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos().min(u128::from(u64::MAX)) as u64);
        // Stat first: missing file → try next candidate without parsing.
        let Some(mtime_ns) = mtime_ns else {
            continue;
        };
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Some(scale) =
                crate::core::coordinate_transform::parse_kwin_scale_from_json(&content)
            {
                return Some((scale, mtime_ns));
            }
        }
        // File exists but unparsable: still report its mtime so callers can
        // cache the negative result until the file actually changes again
        // instead of re-parsing the same broken content every commit.
        // Scale 0.0 is a sentinel meaning "no usable scale in this file".
        return Some((0.0, mtime_ns));
    }
    None
}

/// Read the active KWin output scale factor from the guest configuration (`kwinoutputconfig.json`).
pub fn read_kwin_output_scale() -> Option<f64> {
    match read_kwin_output_scale_with_mtime() {
        Some((scale, _)) if scale > 0.0 && scale.is_finite() => Some(scale),
        _ => None,
    }
}

/// Synchronize KWin output scale with AuthoritativeDisplayState.
/// Returns true if the scale factor changed.
///
/// Caches the config-file mtime inside `state`: the filesystem is stat'ed to
/// learn whether the authoritative file changed, but JSON is parsed ONLY when
/// the mtime is newer than the cached value. Callers must invoke this only on
/// new KWin commits / host resizes — never per-frame unconditionally.
pub fn sync_kwin_output_scale(
    state: &mut crate::core::coordinate_transform::AuthoritativeDisplayState,
) -> bool {
    let Some((scale, mtime_ns)) = read_kwin_output_scale_with_mtime() else {
        return false;
    };
    if let Some(cached) = state.kwin_config_mtime_ns {
        if mtime_ns <= cached {
            return false;
        }
    }
    state.kwin_config_mtime_ns = Some(mtime_ns);
    if scale > 0.0 && scale.is_finite() {
        return state.update_kwin_scale(scale);
    }
    false
}
