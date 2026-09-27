//! Host-testable policy for Debian package updates inside the guest.
//!
//! Portal owns a small platform layer inside Debian: the lfdevs Anland
//! KWin/XWayland builds replace Debian's own packages, which have no Anland
//! backend. Everything else in the guest is ordinary Debian and is updated
//! in place with apt. Two rules keep the two layers apart:
//!
//! * An apt preference pins the platform packages away from Debian's
//!   archives, so neither an upgrade nor a user's `apt install` can replace
//!   them with stock builds.
//! * Portal only ever runs `apt-get upgrade --with-new-pkgs`, which never
//!   removes a package. When Debian moves a library the pinned KWin depends
//!   on (for example Qt's private ABI), apt keeps that library back instead
//!   of removing KWin.
//!
//! The Android wrapper (`src/android/proot/system_updates.rs`) runs apt and
//! owns the UI state; this module only parses and decides.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Debian packages whose installed payload is Portal's, not Debian's.
/// Mirrors `LFDEVS_OVERLAY_ORDER` in `scripts/build_debian_rootfs.py`.
pub const PROTECTED_PACKAGES: &[&str] = &[
    "kwin-common",
    "kwin-data",
    "kwin-wayland",
    "kwin-x11",
    "libkwin6",
    "xwayland",
];

/// Guest-relative apt preference that pins [`PROTECTED_PACKAGES`].
pub const APT_PIN_REL: &str = "etc/apt/preferences.d/portal-platform";
/// Guest-relative result of the last background check.
pub const CACHE_REL: &str = "var/lib/localdesktop/system-updates.json";
/// Present from the start of an upgrade until it completes successfully.
pub const INTERRUPTED_REL: &str = "var/lib/localdesktop/system-update-in-progress";
/// Guest path of the apt transcript for checks and upgrades.
pub const LOG_GUEST_PATH: &str = "/var/log/portal-updates.log";
/// Minimum time between background `apt-get update` runs.
pub const CHECK_INTERVAL_SECS: u64 = 12 * 60 * 60;

/// The pin file. `o=Debian` matches the main, updates and security
/// archives alike; the installed lfdevs version stays the only candidate.
pub fn apt_pin_content() -> String {
    format!(
        "# Managed by Portal and rewritten on every launch.\n\
         # Portal ships these packages from its Anland build. Debian's builds\n\
         # have no Anland backend and would leave the desktop black.\n\
         Package: {}\n\
         Pin: release o=Debian\n\
         Pin-Priority: -1\n",
        PROTECTED_PACKAGES.join(" ")
    )
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PendingUpdate {
    pub package: String,
    /// Installed version, empty when the upgrade pulls in a new package.
    pub from: String,
    pub to: String,
    pub security: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateCache {
    /// Unix seconds of the last successful check.
    pub checked_at: u64,
    pub updates: Vec<PendingUpdate>,
}

impl UpdateCache {
    pub fn parse(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }
}

/// Whether a background check should run. A clock that moved backwards
/// counts as due rather than suppressing checks until it catches up.
pub fn check_due(cache: Option<&UpdateCache>, now: u64) -> bool {
    match cache {
        None => true,
        Some(cache) => {
            cache.checked_at > now || now - cache.checked_at >= CHECK_INTERVAL_SECS
        }
    }
}

fn strip_arch(package: &str) -> &str {
    package.split_once(':').map_or(package, |(name, _)| name)
}

/// Parse `apt-get -s upgrade` output. Only `Inst` lines are upgrades; kept
/// back and pinned packages never appear as `Inst`, so the result is exactly
/// what a real `apt-get upgrade --with-new-pkgs` would install.
///
/// `Inst libfoo [1.0-1] (1.0-2 Debian-Security:13/stable-security [arm64])`
/// `Inst libnew (2.0-1 Debian:13.1/stable [arm64])`
pub fn parse_upgrade_simulation(stdout: &str) -> Vec<PendingUpdate> {
    let mut updates = Vec::new();
    for line in stdout.lines() {
        let Some(rest) = line.strip_prefix("Inst ") else {
            continue;
        };
        let Some((package, rest)) = rest.split_once(' ') else {
            continue;
        };
        let (from, rest) = match rest.strip_prefix('[') {
            Some(bracketed) => match bracketed.split_once("] ") {
                Some((from, rest)) => (from, rest),
                None => continue,
            },
            None => ("", rest),
        };
        let Some(candidate) = rest.strip_prefix('(') else {
            continue;
        };
        let mut fields = candidate.split_whitespace();
        let Some(to) = fields.next() else {
            continue;
        };
        let security = fields.any(|field| field.starts_with("Debian-Security:"));
        updates.push(PendingUpdate {
            package: strip_arch(package).to_owned(),
            from: from.to_owned(),
            to: to.to_owned(),
            security,
        });
    }
    updates
}

/// Installed package versions from `/var/lib/dpkg/status`.
pub fn installed_versions(status: &str) -> HashMap<String, String> {
    let mut installed = HashMap::new();
    for stanza in status.split("\n\n") {
        let mut package = None;
        let mut version = None;
        let mut state = None;
        for line in stanza.lines() {
            if let Some(value) = line.strip_prefix("Package: ") {
                package = Some(value.trim());
            } else if let Some(value) = line.strip_prefix("Version: ") {
                version = Some(value.trim());
            } else if let Some(value) = line.strip_prefix("Status: ") {
                state = Some(value.trim());
            }
        }
        if state == Some("install ok installed") {
            if let (Some(package), Some(version)) = (package, version) {
                installed.insert(package.to_owned(), version.to_owned());
            }
        }
    }
    installed
}

/// Drop cached updates the user already installed some other way, such as
/// `apt upgrade` in Konsole, so a stale cache never offers finished work.
pub fn still_pending(
    updates: &[PendingUpdate],
    installed: &HashMap<String, String>,
) -> Vec<PendingUpdate> {
    updates
        .iter()
        .filter(|update| match installed.get(&update.package) {
            Some(version) => update.from == *version,
            None => update.from.is_empty(),
        })
        .cloned()
        .collect()
}

/// Protected packages whose installed version differs between two status
/// snapshots. Any entry means the pin failed and the platform was touched.
pub fn changed_protected_packages(
    before: &HashMap<String, String>,
    after: &HashMap<String, String>,
) -> Vec<String> {
    PROTECTED_PACKAGES
        .iter()
        .filter(|package| before.get(**package) != after.get(**package))
        .map(|package| (*package).to_owned())
        .collect()
}

/// Security fixes first, then alphabetical: the order the UI lists them in.
pub fn sort_for_display(updates: &mut [PendingUpdate]) {
    updates.sort_by(|a, b| {
        b.security
            .cmp(&a.security)
            .then_with(|| a.package.cmp(&b.package))
    });
}

#[derive(Clone, Debug, PartialEq)]
pub enum AptStatus {
    /// `dlstatus`: download percent.
    Download(f32),
    /// `pmstatus`: dpkg percent and its action message.
    Install(f32, String),
    /// `pmerror`: the package and dpkg's error.
    Error(String),
}

/// Parse one `APT::Status-Fd` line. The package field may itself contain
/// colons (`libfoo:arm64`), so the percent is the first numeric field.
///
/// `dlstatus:3:25.0000:Retrieving file 3 of 12`
/// `pmstatus:libfoo:arm64:12.5:Unpacking libfoo:arm64 (1.0-2)`
pub fn parse_apt_status(line: &str) -> Option<AptStatus> {
    let (kind, rest) = line.trim_end().split_once(':')?;
    let fields: Vec<&str> = rest.split(':').collect();
    let percent_at = (1..fields.len()).find(|&i| fields[i].parse::<f32>().is_ok())?;
    let percent = fields[percent_at].parse::<f32>().ok()?.clamp(0.0, 100.0);
    let message = fields[percent_at + 1..].join(":");
    match kind {
        "dlstatus" => Some(AptStatus::Download(percent)),
        "pmstatus" => Some(AptStatus::Install(percent, message)),
        "pmerror" => Some(AptStatus::Error(format!(
            "{}: {message}",
            fields[..percent_at].join(":")
        ))),
        _ => None,
    }
}

/// Overall progress for one upgrade: package lists end at 10%, downloads
/// run to 50% and dpkg to 96%. The last points belong to verification.
pub fn overall_progress(status: &AptStatus) -> Option<u16> {
    match status {
        AptStatus::Download(percent) => Some(10 + (percent * 0.40) as u16),
        AptStatus::Install(percent, _) => Some(50 + (percent * 0.46) as u16),
        AptStatus::Error(_) => None,
    }
}

/// Shorten dpkg's action message for the launch screen:
/// `Unpacking libfoo:arm64 (1.0-2)` becomes `Unpacking libfoo`.
pub fn friendly_status(message: &str) -> String {
    let message = message.split(" (").next().unwrap_or(message);
    message
        .split(' ')
        .map(strip_arch)
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}
