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
