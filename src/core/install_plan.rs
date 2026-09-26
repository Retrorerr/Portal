//! Immutable, validated first-run choices committed by Portal's installer.
//!
//! The Android UI is allowed to *propose* a plan exactly once.  Native code
//! validates and persists the canonical form before any Debian work begins,
//! then all retries use that durable plan rather than reconstructing a choice
//! from a recreated UI.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{self, Read, Write},
    path::Path,
    process,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

pub const INSTALL_PLAN_VERSION: u32 = 1;
pub const INSTALL_PLAN_FILE: &str = "portal-install-plan-v1.json";
const MAX_INSTALL_PLAN_BYTES: u64 = 8 * 1024;
static INITIAL_PLAN_COMMIT_LOCK: Mutex<()> = Mutex::new(());
/// KWin 6.3.6 accepts output scales up to (and including) 5.0. Reject a
/// choice whose fixed `Large` multiplier would exceed that limit; silently
/// clamping would make the persisted user choice differ from what was applied.
pub const MAX_INSTALL_DENSITY_DPI: u32 = 695;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AppearanceChoice {
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AppliedAppearance {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InterfaceSize {
    Compact,
    Balanced,
    Large,
}

/// Applications which can be selected on Portal's first-run screen.
///
/// This enum deliberately doubles as the native allowlist.  Package names
/// are never received from Kotlin or interpolated from an untrusted string.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OptionalApp {
    Chatgpt,
    Gimp,
    Inkscape,
    Krita,
    Libreoffice,
    Thunderbird,
    Vlc,
}

impl OptionalApp {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Chatgpt => "chatgpt",
            Self::Gimp => "gimp",
            Self::Inkscape => "inkscape",
            Self::Krita => "krita",
            Self::Libreoffice => "libreoffice",
            Self::Thunderbird => "thunderbird",
            Self::Vlc => "vlc",
        }
    }

    pub const fn package_name(self) -> &'static str {
        match self {
            Self::Chatgpt => "chatgpt",
            Self::Gimp => "gimp",
            Self::Inkscape => "inkscape",
            Self::Krita => "krita",
            Self::Libreoffice => "libreoffice",
            Self::Thunderbird => "thunderbird",
            Self::Vlc => "vlc",
        }
    }

    /// Packages required for the selected app to run in Portal's Plasma
    /// Wayland session, not merely for Debian to mark its metapackage installed.
    pub const fn required_packages(self) -> &'static [&'static str] {
        match self {
            Self::Chatgpt => &["chatgpt"],
            Self::Gimp => &["gimp"],
            Self::Inkscape => &["inkscape"],
            Self::Krita => &["krita"],
            Self::Libreoffice => &["libreoffice", "libreoffice-kf6"],
            Self::Thunderbird => &["thunderbird"],
            Self::Vlc => &["vlc"],
        }
    }

    /// Desktop-entry ID shipped by the corresponding Debian Trixie package.
    /// These IDs are fixed native metadata, never supplied by the UI.
    pub const fn desktop_file_id(self) -> &'static str {
        match self {
            Self::Chatgpt => "chatgpt.desktop",
            Self::Gimp => "gimp.desktop",
            Self::Inkscape => "org.inkscape.Inkscape.desktop",
            Self::Krita => "org.kde.krita.desktop",
            Self::Libreoffice => "libreoffice-startcenter.desktop",
            Self::Thunderbird => "thunderbird.desktop",
            Self::Vlc => "vlc.desktop",
        }
    }

    /// URL format consumed by Plasma's Task Manager launcher list.
    pub fn plasma_launcher_url(self) -> String {
        format!("applications:{}", self.desktop_file_id())
    }
}

/// A validated snapshot of every first-run choice that affects the guest.
///
/// All fields are private so callers can inspect a plan but cannot mutate a
/// plan after it has crossed the JNI boundary.  Serde creates it only through
/// [`InstallPlan::from_json`], which calls [`InstallPlan::validate`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallPlan {
    version: u32,
    appearance: AppearanceChoice,
    #[serde(rename = "committedAppearance")]
    committed_appearance: AppliedAppearance,
    #[serde(rename = "interfaceSize")]
    interface_size: InterfaceSize,
    #[serde(rename = "densityDpi")]
    density_dpi: u32,
    #[serde(rename = "logicalWidthPx")]
    logical_width_px: u32,
    #[serde(rename = "logicalHeightPx")]
    logical_height_px: u32,
    #[serde(rename = "selectedAppIds")]
    selected_app_ids: Vec<OptionalApp>,
}

impl InstallPlan {
    pub fn from_json(json: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            json.len() as u64 <= MAX_INSTALL_PLAN_BYTES,
            "Install plan JSON exceeds the supported size"
        );
        let plan: Self = serde_json::from_str(json).context("Install plan JSON is invalid")?;
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.version == INSTALL_PLAN_VERSION,
            "Unsupported install plan version {}",
            self.version
        );
        anyhow::ensure!(
            (72..=MAX_INSTALL_DENSITY_DPI).contains(&self.density_dpi),
            "Install plan densityDpi must be between 72 and 695 so the selected Plasma output scale stays within KWin's supported range"
        );
        anyhow::ensure!(
            (1..=32_768).contains(&self.logical_width_px)
                && (1..=32_768).contains(&self.logical_height_px),
            "Install plan logical display extent is outside supported bounds"
        );
        match self.appearance {
            AppearanceChoice::System => {}
            AppearanceChoice::Dark => anyhow::ensure!(
                self.committed_appearance == AppliedAppearance::Dark,
                "Dark appearance must commit a dark Plasma appearance"
            ),
            AppearanceChoice::Light => anyhow::ensure!(
                self.committed_appearance == AppliedAppearance::Light,
                "Light appearance must commit a light Plasma appearance"
            ),
        }
        anyhow::ensure!(
            self.selected_app_ids
                .windows(2)
                .all(|ids| ids[0] < ids[1]),
            "Install plan app IDs must be strictly sorted and unique"
        );
        Ok(())
    }

    /// Canonical JSON is stable because the struct field order and optional
    /// app order are fixed by validation.  It is used for durable plan records
    /// and guest-side proof binding, never as a UI source of truth.
    pub fn canonical_json(&self) -> anyhow::Result<String> {
        self.validate()?;
        serde_json::to_string(self).context("Could not serialize install plan")
    }

    /// Stable digest used to prove that a delayed live-Plasma application
    /// belongs to this exact native plan.
    pub fn fingerprint(&self) -> anyhow::Result<String> {
        let canonical = self.canonical_json()?;
        let mut hash = Sha256::new();
        hash.update(canonical.as_bytes());
        Ok(format!("{:x}", hash.finalize()))
    }

    pub const fn appearance(&self) -> AppearanceChoice {
        self.appearance
    }

    pub const fn committed_appearance(&self) -> AppliedAppearance {
        self.committed_appearance
    }

    pub const fn interface_size(&self) -> InterfaceSize {
        self.interface_size
    }

    pub const fn density_dpi(&self) -> u32 {
        self.density_dpi
    }

    pub const fn logical_width_px(&self) -> u32 {
        self.logical_width_px
    }

    pub const fn logical_height_px(&self) -> u32 {
        self.logical_height_px
    }

    /// The device-derived baseline required by Portal's presentation path.
    /// Size-choice multipliers belong to the Plasma/KScreen applicator.
    pub fn device_baseline_scale(&self) -> f64 {
        f64::from(self.density_dpi) / 160.0
    }

    /// One initial KScreen output scale, derived from Android's actual display
    /// density rather than a tablet preset.  These are deliberately modest,
    /// bounded multipliers around the baseline: the selected output scale is
    /// still fractional and remains a one-time seed for user-owned KScreen
    /// configuration.
    pub fn initial_output_scale(&self) -> f64 {
        let multiplier = match self.interface_size {
            InterfaceSize::Compact => 0.85,
            InterfaceSize::Balanced => 1.00,
            InterfaceSize::Large => 1.15,
        };
        self.device_baseline_scale() * multiplier
    }

    /// Decimal form accepted by `kscreen-doctor output.<id>.scale.<qreal>`.
    /// It is generated in native code, never parsed or recalculated by a
    /// shell helper.
    pub fn initial_output_scale_string(&self) -> String {
        // All three fixed multipliers over integer dpi are exactly representable
        // within five decimal places (the common denominator is 3200), so this
        // preserves the selected fractional scale rather than rounding it to
        // an integer or collapsing adjacent choices.
        let mut value = format!("{:.5}", self.initial_output_scale());
        while value.contains('.') && value.ends_with('0') {
            value.pop();
        }
        if value.ends_with('.') {
            value.pop();
        }
        value
    }

    pub fn selected_apps(&self) -> &[OptionalApp] {
        &self.selected_app_ids
    }

    pub fn selected_packages(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.selected_app_ids
            .iter()
            .copied()
            .map(OptionalApp::package_name)
    }
}

/// Validate the guest-side readback which is allowed to close a first-run
/// install. `output_id` and `observed_scale` are returned for diagnostics; all
/// other fields must exactly match the immutable native plan and this Plasma
/// launch attempt.
pub fn validate_initial_setup_proof(
    contents: &str,
    plan: &InstallPlan,
    appearance: AppliedAppearance,
    attempt_id: &str,
) -> anyhow::Result<(u32, f64)> {
    anyhow::ensure!(contents.is_ascii(), "Initial setup proof is not ASCII");
    let allowed = [
        "version",
        "fingerprint",
        "attempt_id",
        "appearance",
        "scale",
        "app_ids",
        "launcher_urls",
        "widget_ids",
        "output_id",
        "observed_scale",
    ];
    let mut fields = HashMap::with_capacity(allowed.len());
    for line in contents.lines() {
        anyhow::ensure!(!line.is_empty(), "Initial setup proof contains a blank line");
        let (key, value) = line
            .split_once('=')
            .context("Initial setup proof contains a malformed field")?;
        anyhow::ensure!(
            allowed.contains(&key),
            "Initial setup proof contains an unknown field"
        );
        anyhow::ensure!(
            !value.is_empty() || matches!(key, "app_ids" | "launcher_urls" | "widget_ids"),
            "Initial setup proof contains an invalid empty or duplicate field"
        );
        anyhow::ensure!(
            fields.insert(key, value).is_none(),
            "Initial setup proof contains an invalid empty or duplicate field"
        );
    }
    anyhow::ensure!(
        fields.len() == allowed.len(),
        "Initial setup proof is incomplete"
    );
    anyhow::ensure!(fields["version"] == "2", "Initial setup proof version is unsupported");
    anyhow::ensure!(
        fields["fingerprint"] == plan.fingerprint()?,
        "Initial setup proof belongs to a different accepted plan"
    );
    anyhow::ensure!(
        fields["attempt_id"] == attempt_id,
        "Initial setup proof belongs to a different Plasma attempt"
    );
    let expected_appearance = match appearance {
        AppliedAppearance::Dark => "dark",
        AppliedAppearance::Light => "light",
    };
    anyhow::ensure!(
        fields["appearance"] == expected_appearance,
        "Initial setup proof does not match the committed appearance"
    );
    anyhow::ensure!(
        fields["scale"] == plan.initial_output_scale_string(),
        "Initial setup proof does not match the accepted output scale"
    );
    let expected_app_ids = plan
        .selected_apps()
        .iter()
        .map(|app| app.id())
        .collect::<Vec<_>>()
        .join(",");
    anyhow::ensure!(
        fields["app_ids"] == expected_app_ids,
        "Initial setup proof does not match the selected applications"
    );
    let expected_launcher_urls = plan
        .selected_apps()
        .iter()
        .copied()
        .map(OptionalApp::plasma_launcher_url)
        .collect::<Vec<_>>()
        .join(",");
    anyhow::ensure!(
        fields["launcher_urls"] == expected_launcher_urls,
        "Initial setup proof does not match the selected Plasma panel launchers"
    );
    let widget_ids = if fields["widget_ids"].is_empty() {
        Vec::new()
    } else {
        let mut unique = std::collections::HashSet::new();
        let parsed = fields["widget_ids"]
            .split(',')
            .map(|value| {
                anyhow::ensure!(
                    value.bytes().all(|byte| byte.is_ascii_digit()),
                    "Initial setup proof panel widget id is not decimal"
                );
                let id = value
                    .parse::<u32>()
                    .context("Initial setup proof panel widget id is not numeric")?;
                anyhow::ensure!(
                    id > 0 && unique.insert(id),
                    "Initial setup proof panel widget ids must be positive and unique"
                );
                Ok(id)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        parsed
    };
    anyhow::ensure!(
        !widget_ids.is_empty(),
        "Initial setup proof does not validate the Plasma taskbar launchers"
    );
    anyhow::ensure!(
        fields["output_id"].bytes().all(|byte| byte.is_ascii_digit()),
        "Initial setup proof output id is not decimal"
    );
    let output_id = fields["output_id"]
        .parse::<u32>()
        .context("Initial setup proof output id is not numeric")?;
    anyhow::ensure!(output_id > 0, "Initial setup proof output id must be positive");
    let observed_scale = fields["observed_scale"]
        .parse::<f64>()
        .context("Initial setup proof scale is not numeric")?;
    anyhow::ensure!(
        observed_scale.is_finite() && observed_scale > 0.0 && observed_scale <= 5.0,
        "Initial setup proof scale is outside KWin's supported range"
    );
    let expected_readback = (plan.initial_output_scale() * 120.0).round() / 120.0;
    anyhow::ensure!(
        (observed_scale - expected_readback).abs() <= 0.001,
        "KScreen's persisted output scale does not match the accepted scale"
    );
    Ok((output_id, observed_scale))
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallPlanState {
    InProgress,
    Failed,
    Complete,
}

/// The small app-private record retained across Android process death.  It is
/// intentionally outside `runtime-B`: runtime extraction/promotion must not
/// be able to lose a selection that was already accepted by the user.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedInstallPlan {
    state: InstallPlanState,
    plan: InstallPlan,
    /// `System` is resolved from Android uiMode immediately before its first
    /// Plasma application. Keeping that runtime resolution separate preserves
    /// the exact accepted plan and fingerprint across retries.
    #[serde(default, rename = "resolvedAppearance")]
    resolved_appearance: Option<AppliedAppearance>,
}

impl PersistedInstallPlan {
    pub fn new(state: InstallPlanState, plan: InstallPlan) -> anyhow::Result<Self> {
        plan.validate()?;
        let resolved_appearance = match plan.appearance {
            AppearanceChoice::System => None,
            AppearanceChoice::Dark | AppearanceChoice::Light => Some(plan.committed_appearance),
        };
        Ok(Self {
            state,
            plan,
            resolved_appearance,
        })
    }

    pub const fn state(&self) -> InstallPlanState {
        self.state
    }

    pub fn plan(&self) -> &InstallPlan {
        &self.plan
    }

    pub const fn resolved_appearance(&self) -> Option<AppliedAppearance> {
        self.resolved_appearance
    }

    pub fn with_resolved_appearance(
        &self,
        appearance: AppliedAppearance,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            self.plan.appearance == AppearanceChoice::System,
            "Only a System appearance plan can be resolved at apply time"
        );
        anyhow::ensure!(
            self.resolved_appearance.is_none() || self.resolved_appearance == Some(appearance),
            "The accepted System appearance was already resolved differently"
        );
        anyhow::ensure!(
            self.state == InstallPlanState::InProgress,
            "A failed or completed plan cannot resolve its appearance"
        );
        Ok(Self {
            state: self.state,
            plan: self.plan.clone(),
            resolved_appearance: Some(appearance),
        })
    }

    pub fn with_state(&self, state: InstallPlanState) -> Self {
        Self {
            state,
            plan: self.plan.clone(),
            resolved_appearance: self.resolved_appearance,
        }
    }

    pub fn read(path: &Path) -> anyhow::Result<Option<Self>> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("Could not inspect persisted install plan"),
        };
        anyhow::ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "Persisted install plan is not a regular file"
        );
        let file = fs::File::open(path).context("Could not read persisted install plan")?;
        let mut bytes = Vec::new();
        file.take(MAX_INSTALL_PLAN_BYTES + 1)
            .read_to_end(&mut bytes)
            .context("Could not read persisted install plan")?;
        anyhow::ensure!(
            bytes.len() as u64 <= MAX_INSTALL_PLAN_BYTES,
            "Persisted install plan exceeds the supported size"
        );
        let contents = String::from_utf8(bytes).context("Persisted install plan is not UTF-8")?;
        let mut record: Self = serde_json::from_str(&contents)
            .context("Persisted install plan JSON is invalid")?;
        record.plan.validate()?;
        // The accepted v1 format predates the separate System resolution
        // field. Explicit appearance plans already contain their final target,
        // so migrate those records in memory without changing the accepted
        // plan. System remains unresolved until the first application attempt.
        if record.resolved_appearance.is_none() {
            match record.plan.appearance {
                AppearanceChoice::System => {}
                AppearanceChoice::Dark | AppearanceChoice::Light => {
                    record.resolved_appearance = Some(record.plan.committed_appearance);
                }
            }
        }
        anyhow::ensure!(
            record.plan.appearance == AppearanceChoice::System
                || record.resolved_appearance == Some(record.plan.committed_appearance),
            "Persisted appearance resolution does not match the accepted plan"
        );
        anyhow::ensure!(
            record.state != InstallPlanState::Complete || record.resolved_appearance.is_some(),
            "A completed install plan must have a committed appearance"
        );
        Ok(Some(record))
    }

    /// Atomically replace the record and fsync it before any provisioning
    /// worker is allowed to run.  A torn or missing record is never treated as
    /// permission to invent a replacement plan.
    pub fn write_atomic(&self, path: &Path) -> anyhow::Result<()> {
        self.plan.validate()?;
        let existing = Self::read(path)?;
        match existing.as_ref() {
            Some(existing) => {
                anyhow::ensure!(
                    existing.plan == self.plan,
                    "An accepted install plan cannot be replaced"
                );
                anyhow::ensure!(
                    existing.resolved_appearance == self.resolved_appearance
                        || (existing.resolved_appearance.is_none()
                            && self.resolved_appearance.is_some()
                            && self.plan.appearance == AppearanceChoice::System
                            && existing.state == InstallPlanState::InProgress
                            && self.state == InstallPlanState::InProgress),
                    "The persisted appearance resolution cannot be changed or cleared"
                );
                anyhow::ensure!(
                    valid_state_transition(existing.state, self.state),
                    "Invalid persisted install-plan state transition: {:?} -> {:?}",
                    existing.state,
                    self.state
                );
            }
            None => anyhow::ensure!(
                self.state == InstallPlanState::InProgress,
                "A persisted install plan must begin in progress"
            ),
        }
        let parent = path
            .parent()
            .context("Persisted install plan has no parent directory")?;
        fs::create_dir_all(parent).context("Could not create install plan directory")?;
        let bytes = serde_json::to_vec(self).context("Could not serialize persisted install plan")?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary = parent.join(format!(
            ".{}-{}-{nonce}.tmp",
            path.file_name().and_then(|name| name.to_str()).unwrap_or("install-plan"),
            process::id()
        ));
        let write_result = (|| -> anyhow::Result<()> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .context("Could not create staged install plan")?;
            file.write_all(&bytes)
                .context("Could not write staged install plan")?;
            file.sync_all().context("Could not sync staged install plan")?;
            drop(file);
            if existing.is_some() {
                fs::rename(&temporary, path).context("Could not replace install plan")?;
            } else {
                // The first accepted plan must never be replaced by a racing
                // Begin request. Linux/Android commit it with renameat2's
                // atomic RENAME_NOREPLACE flag. Android app-data filesystems
                // may deny hard links even between files owned by the app.
                match commit_initial_plan_without_replacement(&temporary, path) {
                    Ok(()) => {
                        // renameat2 consumes the staged path. The portable
                        // hard-link fallback removes it inside the helper.
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let concurrent = Self::read(path)?.context(
                            "A competing first-run plan disappeared before it could be read",
                        )?;
                        anyhow::ensure!(
                            concurrent.plan == self.plan
                                && concurrent.state == InstallPlanState::InProgress
                                && self.state == InstallPlanState::InProgress,
                            "A different or non-active first-run plan won the acceptance race"
                        );
                        fs::remove_file(&temporary)
                            .context("Could not remove redundant staged install plan")?;
                    }
                    Err(error) => {
                        return Err(error)
                            .context("Could not commit initial install plan without replacement");
                    }
                }
            }
            sync_directory(parent).context("Could not sync install plan directory")?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        write_result
    }
}

fn commit_initial_plan_with_permission_fallback<F>(
    temporary: &Path,
    destination: &Path,
    attempt_no_replace: F,
) -> io::Result<()>
where
    F: FnOnce(&Path, &Path) -> io::Result<()>,
{
    // Android can deny renameat2(RENAME_NOREPLACE) in app-private storage even
    // though a same-directory rename is allowed. Serialize this process's plan
    // writers and recheck the target before the ordinary atomic rename fallback.
    let _commit_guard = INITIAL_PLAN_COMMIT_LOCK
        .lock()
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::Other,
                "initial plan commit lock is poisoned",
            )
        })?;
    match attempt_no_replace(temporary, destination) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            match fs::symlink_metadata(destination) {
                Ok(_) => Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "persisted install plan already exists",
                )),
                Err(metadata_error) if metadata_error.kind() == io::ErrorKind::NotFound => {
                    fs::rename(temporary, destination)
                }
                Err(metadata_error) => Err(metadata_error),
            }
        }
        result => result,
    }
}

#[cfg(any(target_os = "android", target_os = "linux"))]
fn commit_initial_plan_without_replacement(temporary: &Path, destination: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    commit_initial_plan_with_permission_fallback(
        temporary,
        destination,
        |temporary, destination| {
            let temporary = CString::new(temporary.as_os_str().as_bytes()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "staged path contains NUL")
            })?;
            let destination = CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "destination path contains NUL",
                )
            })?;
            // Use syscall rather than the API-30 bionic wrapper so the native library
            // remains loadable on older Android releases supported by the app. Linux
            // has exposed renameat2(RENAME_NOREPLACE) since kernel 3.15.
            let result = unsafe {
                libc::syscall(
                    libc::SYS_renameat2,
                    libc::AT_FDCWD,
                    temporary.as_ptr(),
                    libc::AT_FDCWD,
                    destination.as_ptr(),
                    libc::RENAME_NOREPLACE as libc::c_uint,
                )
            };
            if result == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        },
    )
}

#[cfg(not(any(target_os = "android", target_os = "linux")))]
fn commit_initial_plan_without_replacement(temporary: &Path, destination: &Path) -> io::Result<()> {
    // Platforms without renameat2 retain the previous atomic no-replace
    // behavior. The Linux/Android runtime path deliberately prefers renaming
    // because app-private filesystems may reject hard links.
    commit_initial_plan_with_permission_fallback(
        temporary,
        destination,
        |temporary, destination| {
            fs::hard_link(temporary, destination)?;
            fs::remove_file(temporary)
        },
    )
}

const fn valid_state_transition(from: InstallPlanState, to: InstallPlanState) -> bool {
    matches!(
        (from, to),
        (InstallPlanState::InProgress, InstallPlanState::InProgress)
            | (InstallPlanState::InProgress, InstallPlanState::Failed)
            | (InstallPlanState::InProgress, InstallPlanState::Complete)
            | (InstallPlanState::Failed, InstallPlanState::Failed)
            | (InstallPlanState::Failed, InstallPlanState::InProgress)
            | (InstallPlanState::Complete, InstallPlanState::Complete)
    )
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_PLAN: &str = r#"{
        "version":1,
        "appearance":"system",
        "committedAppearance":"dark",
        "interfaceSize":"balanced",
        "densityDpi":320,
        "logicalWidthPx":1280,
        "logicalHeightPx":800,
        "selectedAppIds":["gimp","libreoffice","vlc"]
    }"#;

    #[test]
    fn valid_plan_is_canonical_and_maps_only_allowlisted_packages() {
        let plan = InstallPlan::from_json(VALID_PLAN).unwrap();
        assert_eq!(plan.device_baseline_scale(), 2.0);
        assert_eq!(
            plan.selected_packages().collect::<Vec<_>>(),
            vec!["gimp", "libreoffice", "vlc"]
        );
        assert_eq!(
            plan.selected_apps()
                .iter()
                .map(|app| app.desktop_file_id())
                .collect::<Vec<_>>(),
            vec!["gimp.desktop", "libreoffice-startcenter.desktop", "vlc.desktop"]
        );
        assert_eq!(
            plan.selected_apps()
                .iter()
                .copied()
                .map(OptionalApp::plasma_launcher_url)
                .collect::<Vec<_>>(),
            vec![
                "applications:gimp.desktop",
                "applications:libreoffice-startcenter.desktop",
                "applications:vlc.desktop"
            ]
        );
        assert_eq!(
            plan.fingerprint().unwrap(),
            InstallPlan::from_json(&plan.canonical_json().unwrap())
                .unwrap()
                .fingerprint()
                .unwrap()
        );
        assert_eq!(plan.initial_output_scale_string(), "2");
    }

    #[test]
    fn optional_apps_have_fixed_debian_desktop_entries() {
        let apps = [
            (OptionalApp::Chatgpt, "chatgpt", "chatgpt.desktop"),
            (OptionalApp::Gimp, "gimp", "gimp.desktop"),
            (
                OptionalApp::Inkscape,
                "inkscape",
                "org.inkscape.Inkscape.desktop",
            ),
            (OptionalApp::Krita, "krita", "org.kde.krita.desktop"),
            (
                OptionalApp::Libreoffice,
                "libreoffice",
                "libreoffice-startcenter.desktop",
            ),
            (OptionalApp::Thunderbird, "thunderbird", "thunderbird.desktop"),
            (OptionalApp::Vlc, "vlc", "vlc.desktop"),
        ];
        for (app, id, desktop_file) in apps {
            assert_eq!(app.id(), id);
            assert_eq!(app.desktop_file_id(), desktop_file);
            assert_eq!(
                app.plasma_launcher_url(),
                format!("applications:{desktop_file}")
            );
        }
        assert_eq!(
            OptionalApp::Libreoffice.required_packages(),
            &["libreoffice", "libreoffice-kf6"]
        );
    }

    #[test]
    fn official_chatgpt_is_a_canonical_optional_choice() {
        let plan = InstallPlan::from_json(&VALID_PLAN.replace(
            "[\"gimp\",\"libreoffice\",\"vlc\"]",
            "[\"chatgpt\",\"gimp\",\"libreoffice\",\"vlc\"]",
        ))
        .unwrap();
        assert_eq!(plan.selected_apps()[0], OptionalApp::Chatgpt);
        assert_eq!(OptionalApp::Chatgpt.required_packages(), &["chatgpt"]);
        assert_eq!(
            plan.selected_packages().collect::<Vec<_>>(),
            vec!["chatgpt", "gimp", "libreoffice", "vlc"]
        );
    }

    fn valid_setup_proof(plan: &InstallPlan, attempt_id: &str, widget_ids: &str) -> String {
        let app_ids = plan
            .selected_apps()
            .iter()
            .map(|app| app.id())
            .collect::<Vec<_>>()
            .join(",");
        let launchers = plan
            .selected_apps()
            .iter()
            .copied()
            .map(OptionalApp::plasma_launcher_url)
            .collect::<Vec<_>>()
            .join(",");
        let scale = plan.initial_output_scale_string();
        format!(
            "version=2\nfingerprint={}\nattempt_id={attempt_id}\nappearance=dark\nscale={scale}\napp_ids={app_ids}\nlauncher_urls={launchers}\nwidget_ids={widget_ids}\noutput_id=1\nobserved_scale={scale}\n",
            plan.fingerprint().unwrap()
        )
    }

    #[test]
    fn initial_setup_proof_accepts_only_this_attempt_and_selected_panel_launchers() {
        let plan = InstallPlan::from_json(VALID_PLAN).unwrap();
        let proof = valid_setup_proof(&plan, "12-34", "3,4");
        assert_eq!(
            validate_initial_setup_proof(&proof, &plan, AppliedAppearance::Dark, "12-34")
                .unwrap(),
            (1, 2.0)
        );

        for rejected in [
            proof.replace("attempt_id=12-34", "attempt_id=12-35"),
            proof.replace(&plan.fingerprint().unwrap(), &"0".repeat(64)),
            proof.replace("app_ids=gimp,libreoffice,vlc", "app_ids=gimp,vlc"),
            proof.replace(
                "applications:libreoffice-startcenter.desktop",
                "applications:thunderbird.desktop",
            ),
            proof.replace("widget_ids=3,4", "widget_ids=3,3"),
            proof.replace("widget_ids=3,4", "widget_ids=0"),
            proof.replace("widget_ids=3,4", "widget_ids=bad"),
            proof.replace("widget_ids=3,4", "widget_ids=+3"),
            proof.replace("widget_ids=3,4", "widget_ids=3, 4"),
            proof.replace("observed_scale=2", "observed_scale=2.5"),
            proof.replace("output_id=1", "output_id=0"),
            proof.replace("output_id=1", "output_id=+1"),
            proof.replace("version=2\n", ""),
            format!("{proof}unexpected=true\n"),
            format!("{proof}attempt_id=12-34\n"),
        ] {
            assert!(
                validate_initial_setup_proof(
                    &rejected,
                    &plan,
                    AppliedAppearance::Dark,
                    "12-34"
                )
                .is_err(),
                "unexpectedly accepted proof:\n{rejected}"
            );
        }
    }

    #[test]
    fn empty_app_plan_still_requires_a_baseline_panel_launcher_proof() {
        let plan_json = VALID_PLAN.replace(
            "[\"gimp\",\"libreoffice\",\"vlc\"]",
            "[]",
        );
        let plan = InstallPlan::from_json(&plan_json).unwrap();
        let proof = valid_setup_proof(&plan, "55-66", "3");
        assert!(validate_initial_setup_proof(&proof, &plan, AppliedAppearance::Dark, "55-66").is_ok());
        let missing_widget = proof.replace("widget_ids=3\n", "widget_ids=\n");
        assert!(
            validate_initial_setup_proof(
                &missing_widget,
                &plan,
                AppliedAppearance::Dark,
                "55-66"
            )
            .is_err()
        );
    }

    #[test]
    fn interface_scales_are_device_relative_and_strictly_ordered() {
        for density_dpi in [120, 320, 640] {
            let compact = InstallPlan::from_json(
                &VALID_PLAN
                    .replace("\"densityDpi\":320", &format!("\"densityDpi\":{density_dpi}"))
                    .replace("\"interfaceSize\":\"balanced\"", "\"interfaceSize\":\"compact\""),
            )
            .unwrap();
            let balanced = InstallPlan::from_json(
                &VALID_PLAN.replace("\"densityDpi\":320", &format!("\"densityDpi\":{density_dpi}")),
            )
            .unwrap();
            let large = InstallPlan::from_json(
                &VALID_PLAN
                    .replace("\"densityDpi\":320", &format!("\"densityDpi\":{density_dpi}"))
                    .replace("\"interfaceSize\":\"balanced\"", "\"interfaceSize\":\"large\""),
            )
            .unwrap();
            assert!(compact.initial_output_scale() < balanced.initial_output_scale());
            assert!(balanced.initial_output_scale() < large.initial_output_scale());
            assert_eq!(balanced.initial_output_scale(), f64::from(density_dpi) / 160.0);
        }
    }

    #[test]
    fn rejects_unknown_unsorted_and_inconsistent_choices() {
        assert!(InstallPlan::from_json(&VALID_PLAN.replace("\"vlc\"]", "\"vlc\",\"kate\"]")).is_err());
        assert!(InstallPlan::from_json(&VALID_PLAN.replace(
            "[\"gimp\",\"libreoffice\",\"vlc\"]",
            "[\"vlc\",\"gimp\"]"
        ))
        .is_err());
        assert!(InstallPlan::from_json(&VALID_PLAN.replace(
            "\"appearance\":\"system\"",
            "\"appearance\":\"dark\""
        )
        .replace("\"committedAppearance\":\"dark\"", "\"committedAppearance\":\"light\""))
        .is_err());
        assert!(InstallPlan::from_json(
            &VALID_PLAN.replace("\"densityDpi\":320", "\"densityDpi\":696")
        )
        .is_err());
    }

    #[test]
    fn durable_record_round_trips_state_and_plan() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(INSTALL_PLAN_FILE);
        let plan = InstallPlan::from_json(VALID_PLAN).unwrap();
        let pending = PersistedInstallPlan::new(InstallPlanState::InProgress, plan).unwrap();
        pending.write_atomic(&path).unwrap();
        assert_eq!(PersistedInstallPlan::read(&path).unwrap(), Some(pending.clone()));
        let resolved = pending
            .with_resolved_appearance(AppliedAppearance::Light)
            .unwrap();
        assert_eq!(resolved.plan(), pending.plan());
        assert_eq!(
            resolved.plan().fingerprint().unwrap(),
            pending.plan().fingerprint().unwrap()
        );
        resolved.write_atomic(&path).unwrap();
        assert_eq!(PersistedInstallPlan::read(&path).unwrap(), Some(resolved.clone()));
        let failed = resolved.with_state(InstallPlanState::Failed);
        failed.write_atomic(&path).unwrap();
        assert_eq!(PersistedInstallPlan::read(&path).unwrap(), Some(failed));
    }

    #[test]
    fn initial_atomic_commit_never_replaces_an_existing_destination() {
        let directory = tempfile::tempdir().unwrap();
        let temporary = directory.path().join("staged-plan.tmp");
        let destination = directory.path().join(INSTALL_PLAN_FILE);
        fs::write(&temporary, b"new-plan").unwrap();
        fs::write(&destination, b"accepted-plan").unwrap();

        let error = commit_initial_plan_without_replacement(&temporary, &destination).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&destination).unwrap(), b"accepted-plan");
        assert_eq!(fs::read(&temporary).unwrap(), b"new-plan");
    }

    #[test]
    fn initial_plan_commit_falls_back_to_atomic_rename_when_no_replace_is_denied() {
        let directory = tempfile::tempdir().unwrap();
        let temporary = directory.path().join("staged-plan.tmp");
        let destination = directory.path().join(INSTALL_PLAN_FILE);
        fs::write(&temporary, b"accepted-plan").unwrap();

        commit_initial_plan_with_permission_fallback(&temporary, &destination, |_, _| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "simulated Android renameat2 denial",
            ))
        })
        .unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"accepted-plan");
        assert!(!temporary.exists());
    }

    #[test]
    fn permission_fallback_never_replaces_an_existing_plan() {
        let directory = tempfile::tempdir().unwrap();
        let temporary = directory.path().join("staged-plan.tmp");
        let destination = directory.path().join(INSTALL_PLAN_FILE);
        fs::write(&temporary, b"new-plan").unwrap();
        fs::write(&destination, b"accepted-plan").unwrap();

        let error = commit_initial_plan_with_permission_fallback(
            &temporary,
            &destination,
            |_, _| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "simulated Android renameat2 denial",
                ))
            },
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&destination).unwrap(), b"accepted-plan");
        assert_eq!(fs::read(&temporary).unwrap(), b"new-plan");
    }

    #[test]
    fn durable_plan_cannot_be_replaced_or_resurrected_after_completion() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(INSTALL_PLAN_FILE);
        let plan = InstallPlan::from_json(VALID_PLAN).unwrap();
        PersistedInstallPlan::new(InstallPlanState::InProgress, plan.clone())
            .unwrap()
            .write_atomic(&path)
            .unwrap();
        PersistedInstallPlan::new(InstallPlanState::Complete, plan.clone())
            .unwrap()
            .write_atomic(&path)
            .unwrap();

        let replacement = InstallPlan::from_json(
            &VALID_PLAN.replace("\"densityDpi\":320", "\"densityDpi\":321"),
        )
        .unwrap();
        assert!(PersistedInstallPlan::new(InstallPlanState::InProgress, replacement)
            .unwrap()
            .write_atomic(&path)
            .is_err());
        assert!(PersistedInstallPlan::new(InstallPlanState::InProgress, plan)
            .unwrap()
            .write_atomic(&path)
            .is_err());
    }
}
