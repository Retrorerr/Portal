//! Install, removal and availability policy for Portal's optional apps.
//!
//! The first-run installer and the Return screen's app manager share these
//! commands. Every shell fragment is fixed text chosen by the
//! [`OptionalApp`] allowlist; nothing from the UI is interpolated.

use crate::core::install_plan::{AppRequirement, OptionalApp};
use std::collections::HashSet;

/// Guest log for every optional-app apt run.
pub const OPTIONAL_APPS_LOG: &str = "/tmp/portal-optional-apps-v1.log";
/// Present while Steam is installed; Portal keeps its launcher, compat tool
/// and lsof shim in sync only then.
pub const STEAM_MARKER_REL: &str = "var/lib/localdesktop/steam-installed-v1";
/// Steam's data root for the desktop user, relative to their home.
pub const STEAM_ROOT_HOME_REL: &str = ".local/share/Steam";

/// apt with machine-readable progress on fd 3.
const APT_INSTALL: &str =
    "apt-get install -y --no-install-recommends -o APT::Status-Fd=3";
const APT_PURGE: &str = "apt-get purge -y -o APT::Status-Fd=3";

/// What the device offers, read once per catalog request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceCapabilities {
    /// The Anland renderer (KWin on the Adreno GPU) is selected.
    pub anland: bool,
    /// `/dev/kgsl-3d0` exists, so Turnip has a GPU to drive.
    pub kgsl: bool,
    pub page_size: usize,
}

/// Why `app` cannot run on this device, or `None` when it can.
pub fn unavailable_reason(app: OptionalApp, caps: DeviceCapabilities) -> Option<&'static str> {
    match app.requirement() {
        AppRequirement::None => None,
        // Proton, DXVK and VKD3D need hardware Vulkan, which Portal has only
        // through Turnip on KGSL. FEX supports only 4 KiB pages.
        AppRequirement::GpuGaming if !caps.kgsl => Some("Needs an Adreno GPU"),
        AppRequirement::GpuGaming if caps.page_size != 4096 => {
            Some("Needs a 4 KB page kernel")
        }
        AppRequirement::GpuGaming if !caps.anland => Some("Needs GPU graphics mode"),
        AppRequirement::GpuGaming => None,
    }
}

/// Bring dpkg/apt to a usable state before installing anything. The image
/// is extracted without maintainer scripts, so gawk's awk alternative is
/// missing until restored here.
pub fn prepare_apt_command() -> String {
    String::from(
        "(test -x /usr/bin/awk || \
         update-alternatives --quiet --install /usr/bin/awk awk /usr/bin/gawk 10) && \
         dpkg --configure -a && \
         apt-get -q update && \
         apt-get install -y --no-remove --no-install-recommends --fix-broken"
    )
}

/// Guest shell that installs `app`'s Debian side. apt status goes to fd 3,
/// which the caller must have opened.
pub fn install_command(app: OptionalApp) -> String {
    match app {
        OptionalApp::Chatgpt => format!(
            r#"({APT_INSTALL} curl &&
            echo 'dlstatus:0:0:Downloading chatgpt_arm64.deb' >&3 &&
            curl --fail --location --retry 3 --proto '=https' --proto-redir '=https' --tlsv1.2 \
                --output /tmp/portal-chatgpt_arm64.deb.part \
                https://persistent.oaistatic.com/codex-app-prod/linux/deb/latest/chatgpt_arm64.deb &&
            test "$(dpkg-deb --field /tmp/portal-chatgpt_arm64.deb.part Package)" = chatgpt &&
            test "$(dpkg-deb --field /tmp/portal-chatgpt_arm64.deb.part Architecture)" = arm64 &&
            mv -f /tmp/portal-chatgpt_arm64.deb.part /tmp/portal-chatgpt_arm64.deb &&
            {APT_INSTALL} /tmp/portal-chatgpt_arm64.deb git ripgrep python3-venv &&
            rm -f /tmp/portal-chatgpt_arm64.deb)"#
        ),
        // Anthropic's signed apt repository, as its Linux install guide
        // describes, so apt resolves the newest release now and delivers
        // later ones with ordinary package updates. The whole key file
        // becomes apt's keyring for this source, so it is accepted only if
        // it holds exactly one key and that key has Anthropic's published
        // fingerprint.
        OptionalApp::Claude => format!(
            r#"({APT_INSTALL} curl gnupg ca-certificates &&
            curl --fail --location --retry 3 --proto '=https' --proto-redir '=https' --tlsv1.2 \
                --output /tmp/portal-claude-desktop-key.asc.part \
                https://downloads.claude.ai/claude-desktop/key.asc &&
            test "$(gpg --batch --with-colons --show-keys /tmp/portal-claude-desktop-key.asc.part |
                awk -F: '$1 == "pub" {{ keys++ }} $1 == "fpr" && primary == "" {{ primary = $10 }}
                    END {{ if (keys == 1) print primary }}')" = 31DDDE24DDFAB679F42D7BD2BAA929FF1A7ECACE &&
            install -m 0644 /tmp/portal-claude-desktop-key.asc.part \
                /usr/share/keyrings/claude-desktop-archive-keyring.asc &&
            rm -f /tmp/portal-claude-desktop-key.asc.part &&
            printf '%s\n' 'deb [arch=arm64 signed-by=/usr/share/keyrings/claude-desktop-archive-keyring.asc] https://downloads.claude.ai/claude-desktop/apt/stable stable main' \
                >/etc/apt/sources.list.d/claude-desktop.list &&
            apt-get update &&
            {APT_INSTALL} claude-desktop git ripgrep python3-venv)"#
        ),
        _ => format!("{APT_INSTALL} {}", app.required_packages().join(" ")),
    }
}

/// Guest shell that removes `app`'s own packages, then whatever apt pulled
/// in only for them. Shared helpers listed in `required_packages` (git,
/// fonts, Python modules) stay: another app or the user may rely on them.
/// Steam has no Debian side to remove; its files are Portal's.
pub fn remove_command(app: OptionalApp) -> Option<String> {
    let packages = match app {
        OptionalApp::Steam => return None,
        // Every installed LibreOffice/VLC part, not just the metapackage.
        OptionalApp::Libreoffice => {
            "$(dpkg-query -W -f '${db:Status-Status} ${Package}\\n' 'libreoffice*' 2>/dev/null | awk '$1 == \"installed\" { print $2 }')"
        }
        OptionalApp::Vlc => {
            "$(dpkg-query -W -f '${db:Status-Status} ${Package}\\n' 'vlc*' 'libvlc*' 2>/dev/null | awk '$1 == \"installed\" { print $2 }')"
        }
        _ => app.package_name()?,
    };
    let cleanup = match app {
        OptionalApp::Claude => {
            " && rm -f /etc/apt/sources.list.d/claude-desktop.list \
             /usr/share/keyrings/claude-desktop-archive-keyring.asc"
        }
        _ => "",
    };
    Some(format!(
        "{APT_PURGE} {packages} && apt-get autoremove --purge -y -o APT::Status-Fd=3{cleanup}"
    ))
}

/// Whether `app` counts as installed. Debian apps need their own package;
/// Steam needs Portal's marker.
pub fn is_installed(app: OptionalApp, dpkg_installed: &HashSet<String>, steam_marker: bool) -> bool {
    match app.package_name() {
        Some(package) => dpkg_installed.contains(package),
        None => steam_marker,
    }
}

/// Parse `/var/lib/dpkg/status` into the set of installed packages.
pub fn installed_packages(dpkg_status: &str) -> HashSet<String> {
    let mut installed = HashSet::new();
    for stanza in dpkg_status.split("\n\n") {
        let mut package = None;
        let mut state = None;
        for line in stanza.lines() {
            if let Some(value) = line.strip_prefix("Package: ") {
                package = Some(value.trim());
            } else if let Some(value) = line.strip_prefix("Status: ") {
                state = Some(value.trim());
            }
        }
        if state == Some("install ok installed") {
            if let Some(package) = package {
                installed.insert(package.to_owned());
            }
        }
    }
    installed
}

/// Progress line printed by `portal-steam-bootstrap.py`: `PROGRESS done total`.
pub fn parse_bootstrap_progress(line: &str) -> Option<(u64, u64)> {
    let mut fields = line.strip_prefix("PROGRESS ")?.split_whitespace();
    let done = fields.next()?.parse().ok()?;
    let total: u64 = fields.next()?.parse().ok()?;
    (total > 0 && done <= total).then_some((done, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: DeviceCapabilities = DeviceCapabilities {
        anland: true,
        kgsl: true,
        page_size: 4096,
    };

    #[test]
    fn steam_needs_the_gpu_path_and_4k_pages() {
        assert_eq!(unavailable_reason(OptionalApp::Steam, GOOD), None);
        assert_eq!(
            unavailable_reason(OptionalApp::Steam, DeviceCapabilities { kgsl: false, ..GOOD }),
            Some("Needs an Adreno GPU")
        );
        assert_eq!(
            unavailable_reason(OptionalApp::Steam, DeviceCapabilities { page_size: 16384, ..GOOD }),
            Some("Needs a 4 KB page kernel")
        );
        assert_eq!(
            unavailable_reason(OptionalApp::Steam, DeviceCapabilities { anland: false, ..GOOD }),
            Some("Needs GPU graphics mode")
        );
        let nothing = DeviceCapabilities { anland: false, kgsl: false, page_size: 16384 };
        for app in OptionalApp::ALL {
            if app != OptionalApp::Steam {
                assert_eq!(unavailable_reason(app, nothing), None);
            }
        }
    }

    #[test]
    fn install_commands_use_only_allowlisted_packages_and_report_progress() {
        for app in OptionalApp::ALL {
            let command = install_command(app);
            assert!(command.contains("APT::Status-Fd=3"), "{command}");
            for package in app.required_packages() {
                assert!(command.contains(package), "{app:?} misses {package}");
            }
        }
        assert_eq!(
            install_command(OptionalApp::Vlc),
            "apt-get install -y --no-install-recommends -o APT::Status-Fd=3 vlc"
        );
        assert!(install_command(OptionalApp::Steam).contains("libnm0 libgtk2.0-0t64 lsof lsb-release"));
        assert!(install_command(OptionalApp::Claude)
            .contains("31DDDE24DDFAB679F42D7BD2BAA929FF1A7ECACE"));
    }

    #[test]
    fn removal_keeps_shared_helpers_and_never_touches_steam_games() {
        assert_eq!(remove_command(OptionalApp::Steam), None);
        let chatgpt = remove_command(OptionalApp::Chatgpt).unwrap();
        assert!(chatgpt.starts_with("apt-get purge -y -o APT::Status-Fd=3 chatgpt && "));
        assert!(!chatgpt.contains("git"));
        assert!(remove_command(OptionalApp::Claude).unwrap().contains("claude-desktop.list"));
        assert!(remove_command(OptionalApp::Libreoffice).unwrap().contains("'libreoffice*'"));
        for app in OptionalApp::ALL {
            if let Some(command) = remove_command(app) {
                assert!(command.contains("autoremove --purge"));
                assert!(!command.contains("rm -rf"));
            }
        }
    }

    #[test]
    fn installed_state_comes_from_dpkg_or_the_steam_marker() {
        let status = "Package: vlc\nStatus: install ok installed\n\n\
                      Package: gimp\nStatus: deinstall ok config-files\n";
        let installed = installed_packages(status);
        assert!(is_installed(OptionalApp::Vlc, &installed, false));
        assert!(!is_installed(OptionalApp::Gimp, &installed, false));
        assert!(!is_installed(OptionalApp::Steam, &installed, false));
        assert!(is_installed(OptionalApp::Steam, &installed, true));
    }

    #[test]
    fn bootstrap_progress_lines_parse() {
        assert_eq!(parse_bootstrap_progress("PROGRESS 5 10"), Some((5, 10)));
        assert_eq!(parse_bootstrap_progress("PROGRESS 11 10"), None);
        assert_eq!(parse_bootstrap_progress("PROGRESS 1 0"), None);
        assert_eq!(parse_bootstrap_progress("DONE 1"), None);
    }
}
