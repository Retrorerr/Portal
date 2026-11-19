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

const ENTRY_MARKER: &str = "X-LocalDesktop-NoSandbox=true";
/// Every flag a Portal helper ever added; the current helper's come first.
const MANAGED: &[&str] = &[
    " --disable-features=GlobalShortcutsPortal",
    " --enable-wayland-ime",
    " --wayland-text-input-version=3",
    " --ozone-platform=wayland",
    " --no-stdio-init",
    " --no-sandbox",
    " --password-store=basic",
];

/// `entry` is `source` plus the marker and, on Exec lines, nothing but `flags`
/// the vendor's own line lacks. Line by line, as the source's last line may
/// lack the newline awk always adds. The vendor side is never stripped, so a
/// copy that lost a flag the vendor ships does not match.
fn only_adds(entry: &str, source: &str, flags: &[&str]) -> bool {
    let mut vendor_lines = source.lines();
    entry
        .lines()
        .filter(|line| line.trim_end() != ENTRY_MARKER)
        .all(|line| {
            let Some(vendor) = vendor_lines.next() else {
                return false;
            };
            if line.starts_with("Exec=") {
                flags
                    .iter()
                    .filter(|flag| !vendor.contains(**flag))
                    .fold(line.to_owned(), |text, flag| text.replace(flag, ""))
                    == vendor
            } else {
                line == vendor
            }
        })
        && vendor_lines.next().is_none()
}

/// `entry` carries nothing beyond what the current helper derives from
/// `source`. A proven copy can still fail this: it keeps a lone
/// `--ozone-platform=wayland` or `--no-stdio-init` as the user's explicit
/// choice, and so must not be recorded as Portal's to regenerate.
pub fn is_helper_derived(entry: &str, source: &str) -> bool {
    only_adds(entry, source, &MANAGED[..3])
}

/// Retire forced Electron backend/stdio options only from an unchanged
/// Portal-derived entry. A marker alone is not ownership proof after a user
/// edits an entry, and options shipped by the app itself must remain intact.
pub fn retire_electron_launch_options(entry: &str, source: &str) -> Option<String> {
    if !entry.lines().any(|line| line == ENTRY_MARKER) || !only_adds(entry, source, MANAGED) {
        return None;
    }
    // Older Electron helpers added these as a pair. A single matching
    // option could be an explicit user choice on another Chromium launcher.
    if !entry.lines().any(|line| {
        line.starts_with("Exec=")
            && line.contains(" --no-stdio-init")
            && line.contains(" --ozone-platform=wayland")
    }) {
        return Some(entry.to_owned());
    }
    let mut vendor_exec = source.lines().filter(|line| line.starts_with("Exec="));
    Some(
        entry
            .split_inclusive('\n')
            .map(|line| {
                if line.starts_with("Exec=") {
                    let vendor = vendor_exec.next().unwrap_or_default();
                    [" --no-stdio-init", " --ozone-platform=wayland"]
                        .iter()
                        .filter(|flag| !vendor.contains(**flag))
                        .fold(line.to_owned(), |text, flag| text.replace(flag, ""))
                } else {
                    line.to_owned()
                }
            })
            .collect(),
    )
}

pub fn install_command() -> String {
    format!(
        "apt-get update && apt-get install -y --no-remove --no-install-recommends {}",
        PACKAGES.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDED: &str = "--disable-features=GlobalShortcutsPortal --enable-wayland-ime --wayland-text-input-version=3";

    fn derived(source: &str) -> String {
        let mut entry = source
            .replace(
                "[Desktop Entry]\n",
                "[Desktop Entry]\nX-LocalDesktop-NoSandbox=true\n",
            )
            .replace("Exec=example", &format!("Exec=example {ADDED}"));
        if !entry.ends_with('\n') {
            entry.push('\n');
        }
        entry
    }

    #[test]
    fn vendor_flags_prove_ownership_and_survive_the_pair_removal() {
        let source = "[Desktop Entry]\nExec=example --ozone-platform=wayland --no-sandbox %U\n";
        let current = derived(source);
        assert_eq!(
            retire_electron_launch_options(&current, source),
            Some(current.clone())
        );
        let legacy = current.replace(" %U", " --no-stdio-init %U");
        let retired = retire_electron_launch_options(&legacy, source).unwrap();
        assert!(!retired.contains("--no-stdio-init"));
        assert!(retired.contains(" --ozone-platform=wayland"));
        assert!(retired.contains(" --no-sandbox"));
    }

    #[test]
    fn source_without_a_trailing_newline_still_proves_ownership() {
        let source = "[Desktop Entry]\nName=Example\nExec=example %U";
        let entry = derived(source);
        assert!(entry.ends_with('\n'));
        assert_eq!(
            retire_electron_launch_options(&entry, source),
            Some(entry.clone())
        );
    }

    #[test]
    fn user_edits_stay_unproven() {
        let source = "[Desktop Entry]\nName=Example\nExec=example --no-sandbox %U\n";
        let entry = derived(source);
        for edited in [
            entry.replace("Name=Example", "Name=Mine"),
            entry.replace(" %U", " --my-flag %U"),
            format!("{entry}# User customization\n"),
            entry.replace("Name=Example\n", ""),
            // Took back the sandbox the vendor's entry turns off.
            entry.replace(" --no-sandbox", ""),
            // Repeated a flag the vendor ships.
            entry.replace(" --no-sandbox", " --no-sandbox --no-sandbox"),
        ] {
            assert_eq!(retire_electron_launch_options(&edited, source), None);
        }
    }

    #[test]
    fn only_what_the_helper_derives_is_recordable() {
        let source = "[Desktop Entry]\nName=Example\nExec=example --no-sandbox %U";
        let entry = derived(source);
        assert!(is_helper_derived(&entry, source));
        for choice in [" --ozone-platform=wayland", " --no-stdio-init"] {
            let kept = entry.replace(" %U", &format!("{choice} %U"));
            assert_eq!(
                retire_electron_launch_options(&kept, source),
                Some(kept.clone())
            );
            assert!(!is_helper_derived(&kept, source));
        }
        assert!(!is_helper_derived(
            &entry.replace(" --no-sandbox", ""),
            source
        ));
    }
}
