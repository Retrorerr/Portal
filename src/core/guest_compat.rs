//! Common native desktop plugins. Larger language and translation runtimes
//! remain dependencies of the apps that need them, rather than image seeds.
pub const PACKAGES: &[&str] = &[
    "qtwayland5",
    "qt6-wayland",
    "qt6-qpa-plugins",
    "qt5-image-formats-plugins",
    "qt6-image-formats-plugins",
    "qt6-svg-plugins",
    "libqt5svg5",
    "libxkbcommon-x11-0",
    "libxcb-cursor0",
];

pub const MARKER: &str = "var/lib/localdesktop/app-compatibility-v1";

/// Content guards preserve a user's own libraries and fail closed after
/// Debian upgrades a private ABI. No loader path is exported to bundled apps.
pub fn library_matches(root: &std::path::Path, path: &str, expected: &str) -> bool {
    use sha2::{Digest, Sha256};
    std::fs::read(root.join(path))
        .is_ok_and(|bytes| format!("{:x}", Sha256::digest(bytes)) == expected)
}

/// Exact text of the retired managed launcher, for ownership-safe cleanup.
pub fn legacy_libreoffice_launcher(name: &str) -> String {
    format!(
        "#!/bin/sh\n\
         # Managed by Portal: LibreOffice's Qt VCL clips at fractional Wayland scales.\n\
         export QT_WAYLAND_DISABLED_INTERFACES=\"${{QT_WAYLAND_DISABLED_INTERFACES:+$QT_WAYLAND_DISABLED_INTERFACES,}}wp_fractional_scale_manager_v1\"\n\
         # Merge bursts of scroll/drag input so each repaint is presented.\n\
         [ -r /usr/local/lib/portal/qt-input-coalesce.so ] && export LD_PRELOAD=\"/usr/local/lib/portal/qt-input-coalesce.so${{LD_PRELOAD:+:$LD_PRELOAD}}\"\n\
         exec /usr/bin/{name} \"$@\"\n"
    )
}

pub fn retire_libreoffice_launchers(root: &std::path::Path) -> std::io::Result<()> {
    for name in ["libreoffice", "soffice"] {
        let path = root.join("usr/local/bin").join(name);
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file())
            && std::fs::read_to_string(&path)
                .is_ok_and(|text| text == legacy_libreoffice_launcher(name))
        {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

pub fn install_command() -> String {
    format!(
        "apt-get update && apt-get install -y --no-remove --no-install-recommends {}",
        PACKAGES.join(" ")
    )
}
