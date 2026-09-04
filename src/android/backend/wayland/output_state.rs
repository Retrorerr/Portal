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
pub fn read_kwin_output_scale() -> Option<f64> {
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
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Some(scale) =
                crate::core::coordinate_transform::parse_kwin_scale_from_json(&content)
            {
                return Some(scale);
            }
        }
    }
    None
}

/// Synchronize KWin output scale with AuthoritativeDisplayState.
/// Returns true if the scale factor changed.
pub fn sync_kwin_output_scale(
    state: &mut crate::core::coordinate_transform::AuthoritativeDisplayState,
) -> bool {
    if let Some(scale) = read_kwin_output_scale() {
        return state.update_kwin_scale(scale);
    }
    false
}
