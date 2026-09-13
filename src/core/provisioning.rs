//! One release image, verified before extraction and atomically promoted.
//!
//! The archive and runtime are deliberately separate transactions.  A
//! `.part` file is only a resumable byte stream, `portal-runtime.tar.xz` is
//! created only after a complete size/hash check, and a staged tree is only
//! an image candidate until it carries `IMAGE_READY_MARKER`.  The final
//! `READY_MARKER` is written by the setup owner after all required guest
//! configuration (including the pinned Mesa layer and durable renderer
//! selection) has succeeded.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const IMAGE_MARKER: &str = "etc/portal-runtime-version";
pub const READY_MARKER: &str = ".portal-runtime-complete";
/// Marker for an extracted, validated image that is not yet a usable Portal
/// installation.  It is intentionally different from `READY_MARKER` so a
/// process death after extraction cannot make Plasma boot early.
pub const IMAGE_READY_MARKER: &str = ".portal-runtime-image-ready";
/// Name of the resumable download.  The final archive is never used as a
/// partial download.
pub const PARTIAL_ARCHIVE: &str = "portal-runtime.tar.xz.part";
/// Durable checkpoint for the last synced prefix in `PARTIAL_ARCHIVE`. A
/// partial file may have a torn tail after process death; only bytes at or
/// before this checkpoint are eligible for a Range resume.
const PARTIAL_CHECKPOINT: &str = "portal-runtime.tar.xz.part.offset";
const INSTALLATION_MARKER: &str = "portal-installation-v1";
const PREVIOUS_RUNTIME: &str = "runtime-B.previous";
const PREVIOUS_PENDING_PREFIX: &str = "runtime-B.previous.pending";
const INVALID_RUNTIME_PREFIX: &str = "runtime-B.invalid";
const INVALID_PREVIOUS_PREFIX: &str = "runtime-B.previous.invalid";
const IMAGE_ONLY_RUNTIME_PREFIX: &str = "runtime-B.image-only";
const IMAGE_ONLY_PREVIOUS_PREFIX: &str = "runtime-B.previous.image-only";
const UNKNOWN_RUNTIME_PREFIX: &str = "runtime-B.unknown";
const UNKNOWN_PREVIOUS_PREFIX: &str = "runtime-B.previous.unknown";
const PRESERVED_LEGACY_RUNTIME_PREFIX: &str = "runtime-B.legacy-preserved";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProvisioningPhase {
    Idle,
    Preparing,
    Downloading,
    Verifying,
    Extracting,
    Promoting,
    Configuring,
    Finalising,
    Complete,
    Failed,
}

impl ProvisioningPhase {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Preparing => "Preparing",
            Self::Downloading => "Downloading Debian",
            Self::Verifying => "Verifying",
            Self::Extracting => "Extracting",
            Self::Promoting => "Promoting",
            Self::Configuring => "Configuring Portal",
            Self::Finalising => "Finalising",
            Self::Complete => "Complete",
            Self::Failed => "Failed",
        }
    }
}

/// The only progress payload shared by native provisioning and the UI bridge.
/// `progress` is never 100 until the durable installation marker has been
/// written and revalidated by the setup owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvisioningSnapshot {
    pub phase: ProvisioningPhase,
    pub progress: u16,
    pub message: String,
    pub error: Option<String>,
}

impl ProvisioningSnapshot {
    pub fn update(phase: ProvisioningPhase, progress: u16, message: impl Into<String>) -> Self {
        Self {
            phase,
            progress: progress.min(99),
            message: message.into(),
            error: None,
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            phase: ProvisioningPhase::Failed,
            progress: 0,
            error: Some(message.clone()),
            message,
        }
    }

    pub fn complete(message: impl Into<String>) -> Self {
        Self {
            phase: ProvisioningPhase::Complete,
            progress: 100,
            message: message.into(),
            error: None,
        }
    }
}

/// Process-local operation policy used by the Android coordinator and host
/// tests. It makes duplicate Begin taps attach to the existing operation and
/// makes retry the only transition out of a failed operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallOperationState {
    Idle,
    Running,
    Failed,
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallStart {
    Start,
    Attach,
    Noop,
}

pub fn begin_installation(state: &mut InstallOperationState) -> InstallStart {
    match *state {
        InstallOperationState::Idle | InstallOperationState::Failed => {
            *state = InstallOperationState::Running;
            InstallStart::Start
        }
        InstallOperationState::Running => InstallStart::Attach,
        InstallOperationState::Complete => InstallStart::Noop,
    }
}

#[derive(Debug, Deserialize)]
pub struct RuntimeArtifact {
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub compressed_bytes: u64,
    /// Exact Portal source commit the runtime was published from (newer
    /// manifests). Optional with a default so older manifests without
    /// provenance still parse; unknown future fields are likewise ignored.
    #[serde(default)]
    pub source_commit: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompletionMarkerKind {
    Legacy,
    Installation,
}

/// Runtime ownership and readiness classification used by promotion and
/// recovery.  A valid Debian layout alone is deliberately not enough to
/// enter a recovery slot: it has to carry Portal provenance as well.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeClassification {
    /// The named runtime path is not present.
    Absent,
    /// A compatible Debian runtime with the final three-line Portal marker.
    BootablePortal,
    /// A compatible Debian runtime with Portal's older two-line marker.
    LegacyPortal,
    /// A layout validated against the exact pinned image identity, but with
    /// no final installation completion marker yet.
    ValidatedImageOnly,
    /// A structurally valid Debian tree without accepted Portal provenance.
    Unknown,
    /// Missing required layout or otherwise invalid runtime contents.
    Invalid,
}

impl RuntimeClassification {
    /// A runtime in this set is proven Portal-owned and is safe to use as a
    /// recovery point for replacing another proven recovery runtime.
    pub const fn is_trusted_recovery(self) -> bool {
        matches!(self, Self::BootablePortal | Self::LegacyPortal)
    }

    /// An exact image marker or a completion marker proves that the tree is
    /// Portal-owned installation state.  Only the completion-marker variants
    /// are strong enough to replace a user-runtime recovery backup.
    pub const fn is_portal_owned(self) -> bool {
        matches!(
            self,
            Self::BootablePortal | Self::LegacyPortal | Self::ValidatedImageOnly
        )
    }

    pub const fn is_recoverable_installation_state(self) -> bool {
        self.is_portal_owned()
    }
}

impl RuntimeArtifact {
    pub fn production() -> Self {
        serde_json::from_str(include_str!("../../assets/debian-runtime.json"))
            .expect("Invalid release runtime manifest")
    }

    fn identity(&self) -> String {
        format!("{}\n{}\n", self.version, self.sha256)
    }

    fn validate_debian_layout(root: &Path) -> anyhow::Result<()> {
        let os = fs::read_to_string(root.join("usr/lib/os-release"))?;
        anyhow::ensure!(
            os.lines().any(|l| l == "ID=debian")
                && os
                    .lines()
                    .any(|l| l == "VERSION_ID=\"13\"" || l == "VERSION_ID=13"),
            "Expected Debian 13"
        );
        for path in [
            "usr/bin/dpkg",
            "usr/bin/apt",
            "usr/bin/bash",
            "usr/bin/kwin_wayland",
            "usr/bin/plasmashell",
            "usr/bin/python3",
            "var/lib/dpkg/status",
        ] {
            anyhow::ensure!(root.join(path).is_file(), "Runtime missing {path}");
        }
        anyhow::ensure!(
            !root.join("usr/bin/pacman").exists(),
            "Unexpected package manager in runtime"
        );
        Ok(())
    }

    /// Validate an extracted archive against this exact release artifact.
    pub fn validate_image(&self, root: &Path) -> anyhow::Result<()> {
        anyhow::ensure!(
            fs::read_to_string(root.join(IMAGE_MARKER))?.trim() == self.version,
            "Runtime artifact version mismatch"
        );
        Self::validate_debian_layout(root)
    }

    fn read_completion_marker(
        root: &Path,
    ) -> anyhow::Result<(String, String, CompletionMarkerKind)> {
        let marker = fs::read_to_string(root.join(READY_MARKER))?;
        let lines: Vec<_> = marker.lines().map(str::trim).collect();
        anyhow::ensure!(
            lines.len() == 2 || lines.len() == 3,
            "Runtime completion marker has an invalid shape"
        );
        let version = lines.first().copied().unwrap_or_default();
        let digest = lines.get(1).copied().unwrap_or_default();
        anyhow::ensure!(
            !version.is_empty(),
            "Runtime completion marker has no version"
        );
        anyhow::ensure!(
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Runtime completion marker has no SHA-256 identity"
        );
        let kind = match lines.get(2).copied() {
            None => CompletionMarkerKind::Legacy,
            Some(INSTALLATION_MARKER) => CompletionMarkerKind::Installation,
            Some(_) => anyhow::bail!("Runtime completion marker has an unknown format"),
        };
        Ok((version.to_string(), digest.to_string(), kind))
    }

    fn validate_installation_marker(root: &Path) -> anyhow::Result<()> {
        let (_, _, kind) = Self::read_completion_marker(root)?;
        anyhow::ensure!(
            kind == CompletionMarkerKind::Installation,
            "Runtime installation marker is from an older incomplete setup"
        );
        Ok(())
    }

    fn is_legacy_runtime(&self, root: &Path) -> bool {
        matches!(
            Self::read_completion_marker(root),
            Ok((_, _, CompletionMarkerKind::Legacy))
        ) && Self::validate_debian_layout(root).is_ok()
    }

    /// Older Portal releases used a two-line completion marker. Treat a
    /// structurally valid one as an already-owned runtime so the setup owner
    /// can migrate it in place without downloading or replacing user data.
    pub fn is_legacy_complete(&self, root: &Path) -> bool {
        self.is_legacy_runtime(root)
    }

    /// Validate a persistent Debian 13 runtime whose required setup stages
    /// have completed. The marker's third line is the durable distinction
    /// between a usable Portal installation and an old/image-only marker.
    pub fn validate_compatible(&self, root: &Path) -> anyhow::Result<()> {
        Self::validate_installation_marker(root)?;
        Self::validate_debian_layout(root)
    }

    pub fn is_bootable(&self, root: &Path) -> bool {
        self.validate_compatible(root).is_ok()
    }

    /// Whether this exact artifact is fully extracted and safe to promote.
    /// This does not mean that Portal setup is complete.
    pub fn is_image_ready(&self, root: &Path) -> bool {
        fs::read_to_string(root.join(IMAGE_READY_MARKER)).ok().as_deref()
            == Some(self.identity().as_str())
            && self.validate_image(root).is_ok()
    }

    /// Classify a runtime without inferring Portal ownership from Debian
    /// binaries alone.  The exact image marker is sufficient for disposable
    /// installation state; a completion marker is required for a trusted
    /// recovery root.
    pub fn classify_runtime(&self, root: &Path) -> RuntimeClassification {
        if !path_exists(root) {
            return RuntimeClassification::Absent;
        }
        if self.is_bootable(root) {
            return RuntimeClassification::BootablePortal;
        }
        if self.is_legacy_runtime(root) {
            return RuntimeClassification::LegacyPortal;
        }
        if self.is_image_ready(root) {
            return RuntimeClassification::ValidatedImageOnly;
        }
        if Self::validate_debian_layout(root).is_ok() {
            RuntimeClassification::Unknown
        } else {
            RuntimeClassification::Invalid
        }
    }

    /// Kept as the image-stage compatibility name used by existing callers.
    /// It intentionally does not imply a bootable Portal installation.
    pub fn is_ready(&self, root: &Path) -> bool {
        self.is_image_ready(root)
    }

    pub fn verify(&self, archive: &Path) -> anyhow::Result<()> {
        let metadata = fs::symlink_metadata(archive)?;
        anyhow::ensure!(
            metadata.file_type().is_file(),
            "Runtime archive is not a regular file"
        );
        anyhow::ensure!(
            metadata.len() == self.compressed_bytes,
            "Runtime download size mismatch"
        );
        let mut file = fs::File::open(archive)?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 256 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        anyhow::ensure!(
            format!("{:x}", hash.finalize()) == self.sha256,
            "Runtime SHA-256 mismatch"
        );
        Ok(())
    }

    pub fn extract(
        &self,
        archive: &Path,
        staging: &Path,
        report: &impl Fn(String),
    ) -> anyhow::Result<()> {
        self.extract_inner(archive, staging, &|message| report(message))
    }

    fn extract_inner(
        &self,
        archive: &Path,
        staging: &Path,
        report: &impl Fn(String),
    ) -> anyhow::Result<()> {
        self.verify(archive)?;
        if path_exists(staging) {
            remove_path_synced(staging)?;
        }
        fs::create_dir_all(staging)?;
        sync_parent_directory(staging)?;
        sync_directory(staging)?;
        let decoder = xz2::read::XzDecoder::new(fs::File::open(archive)?);
        let mut tar = tar::Archive::new(decoder);
        let mut count = 0u64;
        let mut last = Instant::now();
        for entry in tar.entries()? {
            anyhow::ensure!(entry?.unpack_in(staging)?, "Unsafe runtime archive path");
            count += 1;
            if last.elapsed() >= Duration::from_secs(1) {
                report(format!("Extracting Debian runtime: {count} entries"));
                last = Instant::now();
            }
        }
        self.validate_image(staging)?;
        write_atomic(
            &staging.join(IMAGE_READY_MARKER),
            self.identity().as_bytes(),
        )?;
        // The marker rename is durable in the staging directory. Sync the
        // directory entry which makes the staging tree discoverable after a
        // sudden stop as well.
        sync_parent_directory(staging)?;
        Ok(())
    }

    /// Write the final installation truth only after all required setup work
    /// has completed. It is idempotent and upgrades the old two-line marker
    /// without changing the mutable runtime tree.
    pub fn mark_installation_complete(&self, root: &Path) -> anyhow::Result<()> {
        if self.is_bootable(root) {
            if let Some(base) = root.parent() {
                if let Err(error) = self.recover_promotion_state(base, root) {
                    // The marker is already the authoritative committed
                    // truth. Cleanup of an extra parked backup is recoverable
                    // on the next provisioning pass and must not turn a
                    // successful installation into a false setup failure.
                    log::warn!(
                        "Could not finish Portal promotion cleanup after committed marker: {error:#}"
                    );
                }
            }
            return Ok(());
        }

        let identity = if self.is_image_ready(root) {
            self.identity()
        } else if self.is_legacy_runtime(root) {
            let (version, digest, _) = Self::read_completion_marker(root)?;
            format!("{version}\n{digest}\n")
        } else {
            anyhow::bail!(
                "Cannot mark Portal installed: the validated Debian image is unavailable"
            );
        };

        let mut marker = identity;
        marker.push_str(INSTALLATION_MARKER);
        marker.push('\n');
        write_atomic(&root.join(READY_MARKER), marker.as_bytes())?;
        self.validate_compatible(root)?;
        if let Some(base) = root.parent() {
            if let Err(error) = self.recover_promotion_state(base, root) {
                // See the already-bootable branch above: marker durability is
                // the commit point, while parked-backup cleanup is retryable.
                log::warn!(
                    "Could not finish Portal promotion cleanup after committed marker: {error:#}"
                );
            }
        }
        Ok(())
    }

    /// Complete the Debian image transaction. All paths before this method
    /// are safe to repeat after process death; the final marker is deliberately
    /// left for `mark_installation_complete` in the setup coordinator.
    pub fn provision(&self, base: &Path, report: impl Fn(String)) -> anyhow::Result<()> {
        self.provision_with_progress(base, |snapshot| report(snapshot.message))
    }

    pub fn provision_with_progress(
        &self,
        base: &Path,
        report: impl Fn(ProvisioningSnapshot),
    ) -> anyhow::Result<()> {
        let root = base.join("runtime-B");
        fs::create_dir_all(base)?;
        // A previous promotion may have stopped after parking a backup but
        // before the next rename. Resolve only states whose ownership is
        // proven by the runtime contents; unknown trees are quarantined, not
        // overwritten.
        self.recover_promotion_state(base, &root)?;
        // runtime-B is mutable user state. Once it is a complete Portal
        // installation, keep it across APK/image revisions.
        if self.is_bootable(&root) {
            return Ok(());
        }

        // An image that was promoted before setup finished is safe to reuse;
        // the setup coordinator will rerun its idempotent stages and write the
        // final marker last.
        if self.is_image_ready(&root) || self.is_legacy_runtime(&root) {
            return Ok(());
        }

        // A marked-but-corrupt runtime may contain user packages/configuration.
        // Never silently replace it. An unmarked/invalid image can be safely
        // replaced only after a fresh staging tree validates.
        if fs::symlink_metadata(root.join(READY_MARKER)).is_ok() {
            anyhow::bail!(
                "Existing Debian runtime is marked complete but failed validation; refusing to replace user data"
            );
        }

        self.emit(
            &report,
            ProvisioningPhase::Preparing,
            0,
            "Preparing Portal's Debian runtime…",
        );

        let staging = base.join("runtime-B.staging");

        // A previous attempt may have crashed between extraction validation
        // and the IMAGE_READY marker write, or between `rename(root,
        // previous)` and `rename(staging, root)`. An exact image validation is
        // sufficient to recreate the disposable staging marker; promote a
        // fully validated image without downloading or extracting again.
        let staging_ready = if self.is_staged_image(&staging) {
            true
        } else if self.validate_image(&staging).is_ok() {
            write_atomic(
                &staging.join(IMAGE_READY_MARKER),
                self.identity().as_bytes(),
            )?;
            true
        } else {
            false
        };
        if staging_ready {
            self.emit(
                &report,
                ProvisioningPhase::Promoting,
                68,
                "Recovering the validated Debian runtime…",
            );
            self.promote_staging(base, &root, &staging)?;
            if let Err(error) = remove_path_synced(&base.join("portal-runtime.tar.xz")) {
                log::warn!("Could not remove verified Portal runtime archive: {error}");
            }
            self.emit(
                &report,
                ProvisioningPhase::Configuring,
                70,
                "Debian runtime committed; configuring Portal…",
            );
            return Ok(());
        }

        // Incomplete staging is never launchable and extraction replaces it.
        if path_exists(&staging) {
            remove_path_synced(&staging)?;
        }

        let archive = base.join("portal-runtime.tar.xz");
        let partial = base.join(PARTIAL_ARCHIVE);
        let checkpoint = base.join(PARTIAL_CHECKPOINT);
        self.normalise_cached_archive(&archive, &partial, &checkpoint)?;

        // Allow payload allocation overhead as well as the compressed download.
        // A resumable prefix is already allocated and can be subtracted from
        // the conservative estimate. Extraction still gets the full margin.
        #[cfg(target_os = "android")]
        {
            use std::os::unix::ffi::OsStrExt;
            let path = std::ffi::CString::new(base.as_os_str().as_bytes())?;
            let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
            anyhow::ensure!(
                unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } == 0,
                "Cannot check available storage. Reopen Portal to retry."
            );
            let stats = unsafe { stats.assume_init() };
            let available = (stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64);
            let reusable = if self.verify(&archive).is_ok() {
                self.compressed_bytes
            } else {
                fs::metadata(&partial)
                    .map(|m| m.len().min(self.compressed_bytes))
                    .unwrap_or(0)
            };
            let needed = self
                .compressed_bytes
                .saturating_mul(5)
                .saturating_add(512 * 1024 * 1024)
                .saturating_sub(reusable);
            anyhow::ensure!(
                available >= needed,
                "Not enough storage: free {} MiB; Portal needs {} MiB available. Free space and retry setup.",
                available / 1048576,
                needed / 1048576
            );
        }

        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60 * 60))
            .build()?;
        let mut last_error = None;
        for attempt in 1..=3 {
            let result = (|| -> anyhow::Result<()> {
                if self.verify(&archive).is_err() {
                    self.download_archive(
                        &client,
                        &archive,
                        &partial,
                        &checkpoint,
                        attempt,
                        &report,
                    )?;
                    self.emit(
                        &report,
                        ProvisioningPhase::Verifying,
                        48,
                        "Verifying Debian runtime size and SHA-256…",
                    );
                    self.verify(&archive)?;
                }

                self.emit(
                    &report,
                    ProvisioningPhase::Extracting,
                    52,
                    "Extracting the validated Debian runtime…",
                );
                let extract_report = |message: String| {
                    self.emit(&report, ProvisioningPhase::Extracting, 55, message);
                };
                self.extract_inner(&archive, &staging, &extract_report)?;
                self.emit(
                    &report,
                    ProvisioningPhase::Promoting,
                    68,
                    "Validating and committing the Debian runtime…",
                );
                self.promote_staging(base, &root, &staging)?;
                // The archive is disposable only after the live image has
                // proved valid. Failure to remove it is harmless: next launch
                // verifies and reuses it.
                if let Err(error) = remove_path_synced(&archive) {
                    log::warn!("Could not remove verified Portal runtime archive: {error}");
                }
                self.emit(
                    &report,
                    ProvisioningPhase::Configuring,
                    70,
                    "Debian runtime committed; configuring Portal…",
                );
                Ok(())
            })();
            match result {
                Ok(()) => return Ok(()),
                Err(error) => {
                    log::warn!(
                        "Debian runtime attempt {attempt}/3 failed: {error:#}; resumable state preserved"
                    );
                    self.emit(
                        &report,
                        ProvisioningPhase::Preparing,
                        0,
                        format!("Debian runtime attempt {attempt}/3 failed; retrying safely…"),
                    );
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Debian runtime provisioning failed")))
    }

    fn emit(
        &self,
        report: &impl Fn(ProvisioningSnapshot),
        phase: ProvisioningPhase,
        progress: u16,
        message: impl Into<String>,
    ) {
        report(ProvisioningSnapshot::update(phase, progress, message));
    }

    fn is_staged_image(&self, staging: &Path) -> bool {
        self.is_image_ready(staging)
            || (self.validate_image(staging).is_ok() && self.is_legacy_runtime(staging))
    }

    fn pending_backups(&self, base: &Path) -> anyhow::Result<Vec<PathBuf>> {
        if !path_exists(base) {
            return Ok(Vec::new());
        }
        let mut paths = Vec::new();
        for entry in fs::read_dir(base)? {
            let entry = entry?;
            let name = entry.file_name();
            if name
                .to_str()
                .is_some_and(|name| name.starts_with(PREVIOUS_PENDING_PREFIX))
            {
                paths.push(entry.path());
            }
        }
        paths.sort();
        Ok(paths)
    }

    /// Return only disposable quarantine paths.  Unknown Debian-shaped trees
    /// and deliberately preserved legacy runtimes are not included: they may
    /// contain user data and must remain recoverable until an explicit policy
    /// can safely remove them.
    fn disposable_quarantine_paths(&self, base: &Path) -> anyhow::Result<Vec<PathBuf>> {
        if !path_exists(base) {
            return Ok(Vec::new());
        }
        let mut paths = Vec::new();
        for entry in fs::read_dir(base)? {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_str().is_some_and(|name| {
                name.starts_with(INVALID_RUNTIME_PREFIX)
                    || name.starts_with(INVALID_PREVIOUS_PREFIX)
                    || name.starts_with(IMAGE_ONLY_RUNTIME_PREFIX)
                    || name.starts_with(IMAGE_ONLY_PREVIOUS_PREFIX)
            }) {
                paths.push(entry.path());
            }
        }
        paths.sort();
        Ok(paths)
    }

    fn quarantine_prefix(
        classification: RuntimeClassification,
        previous: bool,
    ) -> Option<&'static str> {
        match (classification, previous) {
            (RuntimeClassification::ValidatedImageOnly, false) => Some(IMAGE_ONLY_RUNTIME_PREFIX),
            (RuntimeClassification::ValidatedImageOnly, true) => {
                Some(IMAGE_ONLY_PREVIOUS_PREFIX)
            }
            (RuntimeClassification::Unknown, false) => Some(UNKNOWN_RUNTIME_PREFIX),
            (RuntimeClassification::Unknown, true) => Some(UNKNOWN_PREVIOUS_PREFIX),
            (RuntimeClassification::Invalid, false) => Some(INVALID_RUNTIME_PREFIX),
            (RuntimeClassification::Invalid, true) => Some(INVALID_PREVIOUS_PREFIX),
            (RuntimeClassification::Absent, _)
            | (RuntimeClassification::BootablePortal, _)
            | (RuntimeClassification::LegacyPortal, _) => None,
        }
    }

    fn quarantine_untrusted_runtime(
        base: &Path,
        path: &Path,
        classification: RuntimeClassification,
        previous: bool,
    ) -> anyhow::Result<()> {
        if let Some(prefix) = Self::quarantine_prefix(classification, previous) {
            quarantine_path(base, path, prefix)?;
        }
        Ok(())
    }

    /// Remove only disposable Portal-owned debris after an exact image or a
    /// completed runtime is present. Unknown trees and preserved legacy roots
    /// are intentionally never swept by this helper.
    fn cleanup_disposable_quarantine_paths(
        &self,
        base: &Path,
        root: &Path,
        previous: &Path,
    ) -> anyhow::Result<()> {
        let root_classification = self.classify_runtime(root);
        let previous_classification = self.classify_runtime(previous);
        let safe_to_dispose = root_classification.is_trusted_recovery()
            || (root_classification == RuntimeClassification::ValidatedImageOnly
                && previous_classification.is_trusted_recovery());
        if !safe_to_dispose {
            return Ok(());
        }
        for path in self.disposable_quarantine_paths(base)? {
            remove_path_synced(&path)?;
        }
        Ok(())
    }

    /// Recover the only ambiguous promotion window: an old `.previous` was
    /// parked under a pending name, but the process stopped before the next
    /// rename. Never replace an existing trusted backup with an image-only or
    /// unknown tree. Pending proven-good roots remain until the new live root
    /// has its final completion marker.
    fn recover_promotion_state(&self, base: &Path, root: &Path) -> anyhow::Result<()> {
        let pending = self.pending_backups(base)?;
        let root_classification = self.classify_runtime(root);
        if !root_classification.is_recoverable_installation_state() {
            return Ok(());
        }
        let previous = base.join(PREVIOUS_RUNTIME);
        if pending.is_empty() {
            self.cleanup_disposable_quarantine_paths(base, root, &previous)?;
            return Ok(());
        }

        let mut previous_classification = self.classify_runtime(&previous);
        if path_exists(&previous) && !previous_classification.is_trusted_recovery() {
            Self::quarantine_untrusted_runtime(
                base,
                &previous,
                previous_classification,
                true,
            )?;
            previous_classification = RuntimeClassification::Absent;
        }

        let valid_pending = pending
            .iter()
            .find(|path| {
                self.classify_runtime(path.as_path())
                    .is_trusted_recovery()
            })
            .cloned();
        let mut restored_pending = false;
        if !previous_classification.is_trusted_recovery() {
            if let Some(path) = valid_pending.as_ref() {
                rename_synced(path, &previous)?;
                previous_classification = self.classify_runtime(&previous);
                restored_pending = true;
            }
        }

        // A pending trusted runtime is safe to discard only after the live
        // root itself is fully bootable and another trusted `.previous`
        // exists. In particular, an image-ready root is still an incomplete
        // replacement, so it keeps every proven recovery point until final
        // marker commit. Unknown pending trees are left visibly pending.
        for path in pending {
            if restored_pending && valid_pending.as_ref() == Some(&path) {
                continue;
            }
            let classification = self.classify_runtime(&path);
            if classification.is_trusted_recovery() {
                if root_classification.is_trusted_recovery()
                    && previous_classification.is_trusted_recovery()
                {
                    remove_path_synced(&path)?;
                }
            } else if classification == RuntimeClassification::Invalid
                && root_classification.is_trusted_recovery()
                && previous_classification.is_trusted_recovery()
            {
                remove_path_synced(&path)?;
            }
        }
        self.cleanup_disposable_quarantine_paths(base, root, &previous)?;
        Ok(())
    }

    fn cleanup_pending_backups(
        &self,
        base: &Path,
        root: &Path,
        previous: &Path,
    ) -> anyhow::Result<()> {
        if !self.classify_runtime(root).is_trusted_recovery()
            || !self.classify_runtime(previous).is_trusted_recovery()
        {
            return Ok(());
        }
        for path in self.pending_backups(base)? {
            let classification = self.classify_runtime(&path);
            if classification.is_trusted_recovery()
                || classification == RuntimeClassification::ValidatedImageOnly
                || classification == RuntimeClassification::Invalid
            {
                remove_path_synced(&path)?;
            }
        }
        self.cleanup_disposable_quarantine_paths(base, root, previous)?;
        Ok(())
    }

    fn promote_staging(&self, base: &Path, root: &Path, staging: &Path) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.is_staged_image(staging),
            "Refusing to promote an unvalidated runtime staging tree"
        );

        // First converge any earlier interrupted rotation. This can restore a
        // valid pending backup, but it never overwrites a valid `.previous`.
        self.recover_promotion_state(base, root)?;
        let previous = base.join(PREVIOUS_RUNTIME);

        // Only a completion-marker runtime is trusted for recovery rotation.
        // Image-only, structurally plausible, and invalid previous trees are
        // classified separately and quarantined without touching a proven
        // backup.
        let previous_classification = self.classify_runtime(&previous);
        if path_exists(&previous) && !previous_classification.is_trusted_recovery() {
            Self::quarantine_untrusted_runtime(
                base,
                &previous,
                previous_classification,
                true,
            )?;
        }

        let current_classification = self.classify_runtime(root);
        match current_classification {
            RuntimeClassification::BootablePortal => {
                if path_exists(&previous) {
                    // Park the old recovery root under a unique name first.
                    // A crash here leaves the valid current root untouched;
                    // the next launch can restore or clean it deterministically.
                    let pending = next_available_path(base, PREVIOUS_PENDING_PREFIX);
                    rename_synced(&previous, &pending)?;
                }
                // A bootable Portal root is proven user-owned state, so it is
                // safe to make it the recovery root after the old backup has
                // been parked.
                rename_synced(root, &previous)?;
            }
            RuntimeClassification::LegacyPortal => {
                if self.classify_runtime(&previous) == RuntimeClassification::BootablePortal {
                    // The modern completion marker is the stronger recovery
                    // identity. Keep both proven user runtimes instead of
                    // allowing a legacy tree to displace the stronger one.
                    quarantine_path(base, root, PRESERVED_LEGACY_RUNTIME_PREFIX)?;
                } else {
                    if path_exists(&previous) {
                        let pending = next_available_path(base, PREVIOUS_PENDING_PREFIX);
                        rename_synced(&previous, &pending)?;
                    }
                    rename_synced(root, &previous)?;
                }
            }
            RuntimeClassification::ValidatedImageOnly
            | RuntimeClassification::Unknown
            | RuntimeClassification::Invalid => {
                // Never let an image-only or merely plausible Debian tree
                // replace a trusted backup. Unknown trees are retained under
                // a non-bootable quarantine name because they may contain
                // user data; disposable image/invalid debris is cleaned only
                // after a safe replacement exists.
                Self::quarantine_untrusted_runtime(
                    base,
                    root,
                    current_classification,
                    false,
                )?;
            }
            RuntimeClassification::Absent => {}
        }

        anyhow::ensure!(
            !path_exists(root),
            "Runtime promotion destination is still occupied"
        );
        rename_synced(staging, root)?;
        anyhow::ensure!(
            self.is_staged_image(root),
            "Promoted runtime staging tree failed post-rename validation"
        );
        self.cleanup_pending_backups(base, root, &previous)?;
        self.cleanup_disposable_quarantine_paths(base, root, &previous)?;
        Ok(())
    }

    fn normalise_cached_archive(
        &self,
        archive: &Path,
        partial: &Path,
        checkpoint: &Path,
    ) -> anyhow::Result<()> {
        if self.verify(archive).is_ok() {
            remove_path_synced(partial)?;
            remove_path_synced(checkpoint)?;
            return Ok(());
        }
        if path_exists(archive) {
            let length = regular_file_length(archive).unwrap_or(0);
            if length > 0 && length < self.compressed_bytes && !path_exists(partial) {
                rename_synced(archive, partial)?;
                // Older Portal builds used the final archive name while
                // downloading and synced the file at the end of each
                // response. Carry that resumable prefix forward; current
                // writes add a stricter checkpoint after every synced window.
                write_atomic(checkpoint, length.to_string().as_bytes())?;
            } else {
                remove_path_synced(archive)?;
            }
        }
        if let Some(length) = regular_file_length(partial) {
            if length > self.compressed_bytes {
                remove_path_synced(partial)?;
                remove_path_synced(checkpoint)?;
            } else if length == self.compressed_bytes {
                // A full-size `.part` is still untrusted. Reuse it only when
                // the exact pinned digest proves it is the archive; otherwise
                // discard the ambiguous full-size prefix before requesting a
                // new range.
                if self.verify(partial).is_ok() {
                    rename_synced(partial, archive)?;
                    remove_path_synced(checkpoint)?;
                } else {
                    remove_path_synced(partial)?;
                    remove_path_synced(checkpoint)?;
                }
            }
        } else if path_exists(partial) {
            // Do not let a dangling symlink, directory, or other unexpected
            // node reach OpenOptions in the downloader.
            remove_path_synced(partial)?;
            remove_path_synced(checkpoint)?;
        } else {
            remove_path_synced(checkpoint)?;
        }
        Ok(())
    }

    fn download_archive(
        &self,
        client: &reqwest::blocking::Client,
        archive: &Path,
        partial: &Path,
        checkpoint: &Path,
        attempt: u32,
        report: &impl Fn(ProvisioningSnapshot),
    ) -> anyhow::Result<()> {
        self.emit(
            report,
            ProvisioningPhase::Downloading,
            5,
            format!("Downloading Debian runtime (attempt {attempt}/3)…"),
        );
        let mut actual_length = regular_file_length(partial).unwrap_or(0);
        if path_exists(partial) && regular_file_length(partial).is_none() {
            remove_path_synced(partial)?;
            remove_path_synced(checkpoint)?;
            actual_length = 0;
        } else if actual_length > self.compressed_bytes {
            // A full-size or overlong `.part` is never a valid resume prefix.
            // Discard only this disposable download state; the runtime roots
            // are not touched.
            remove_path_synced(partial)?;
            remove_path_synced(checkpoint)?;
            actual_length = 0;
        }
        let checkpoint_length = fs::read_to_string(checkpoint)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|length| *length <= actual_length && *length <= self.compressed_bytes)
            .unwrap_or(0);
        // A missing/invalid checkpoint means the tail cannot be trusted. It
        // is safer to lose that prefix than to ask the server for bytes after
        // a possibly torn write.
        if actual_length != checkpoint_length {
            let file = fs::OpenOptions::new()
                .create(true)
                .write(true)
                .open(partial)?;
            file.set_len(checkpoint_length)?;
            file.sync_all()?;
        }
        let offset = checkpoint_length;
        if offset == 0 {
            remove_path_synced(checkpoint)?;
        }
        let mut request = client.get(&self.url);
        if offset > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let response = request.send()?;
        let status = response.status();
        anyhow::ensure!(status.is_success(), "Runtime download HTTP {status}");
        if offset == 0 {
            anyhow::ensure!(
                status == reqwest::StatusCode::OK,
                "Server returned an unexpected partial response for a fresh download"
            );
        }

        let mut ranged_end = None;
        let resume = if offset == 0 {
            false
        } else if status == reqwest::StatusCode::PARTIAL_CONTENT {
            let (start, end, total) = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_content_range)
                .ok_or_else(|| anyhow::anyhow!("Server returned no valid Content-Range"))?;
            anyhow::ensure!(
                start == offset && total == self.compressed_bytes && end >= start,
                "Server returned an invalid download range"
            );
            anyhow::ensure!(
                end < self.compressed_bytes,
                "Server returned bytes beyond the pinned runtime size"
            );
            ranged_end = Some(end + 1);
            if let Some(length) = response.content_length() {
                anyhow::ensure!(
                    length == end - start + 1,
                    "Server Content-Length disagrees with Content-Range"
                );
            }
            true
        } else if status == reqwest::StatusCode::OK {
            // A server that ignores Range is safe only when the old prefix is
            // discarded and the response starts the archive from byte zero.
            false
        } else {
            anyhow::bail!("Server returned unsupported response status {status}");
        };

        if let Some(length) = response.content_length() {
            anyhow::ensure!(
                length <= self.compressed_bytes.saturating_sub(if resume { offset } else { 0 }),
                "Runtime response exceeds the pinned expected size"
            );
        }
        if let Some(parent) = partial.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resume)
            .truncate(!resume)
            .open(partial)?;
        let mut buffer = [0u8; 256 * 1024];
        let mut downloaded = if resume { offset } else { 0 };
        let mut checkpointed = if resume { offset } else { 0 };
        let mut last = Instant::now();
        let mut response = response;
        loop {
            let count = response.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            downloaded = downloaded.saturating_add(count as u64);
            anyhow::ensure!(
                downloaded <= self.compressed_bytes,
                "Runtime download exceeds expected size"
            );
            file.write_all(&buffer[..count])?;
            // Checkpoint only after the data file is synced. A process death
            // before the next checkpoint merely loses the current interval;
            // it can never cause a later Range request to skip unknown bytes.
            if downloaded.saturating_sub(checkpointed) >= 4 * 1024 * 1024 {
                file.sync_all()?;
                write_atomic(&checkpoint, downloaded.to_string().as_bytes())?;
                checkpointed = downloaded;
            }
            if last.elapsed() >= Duration::from_secs(1) {
                let progress = 5
                    + ((downloaded.saturating_mul(40) / self.compressed_bytes.max(1)) as u16)
                        .min(40);
                self.emit(
                    report,
                    ProvisioningPhase::Downloading,
                    progress,
                    format!(
                        "Downloading Debian runtime: {} / {} MiB",
                        downloaded / 1048576,
                        self.compressed_bytes / 1048576
                    ),
                );
                last = Instant::now();
            }
        }
        file.sync_all()?;
        write_atomic(&checkpoint, downloaded.to_string().as_bytes())?;
        if let Some(expected_end) = ranged_end {
            if downloaded > expected_end {
                let _ = remove_path_synced(partial);
                let _ = remove_path_synced(checkpoint);
                anyhow::bail!("Server returned more bytes than its Content-Range proved");
            }
            anyhow::ensure!(
                downloaded == expected_end,
                "Server ended before its Content-Range was complete"
            );
        }
        anyhow::ensure!(
            regular_file_length(partial).unwrap_or(0) <= self.compressed_bytes,
            "Runtime partial download exceeds expected size"
        );
        if regular_file_length(partial).unwrap_or(0) == self.compressed_bytes {
            self.emit(
                report,
                ProvisioningPhase::Downloading,
                45,
                "Debian runtime download complete; checking it…",
            );
            // A full-size prefix is still not a final archive until this verify
            // succeeds. Move it atomically only after the exact check.
            if let Err(error) = self.verify(partial) {
                let _ = remove_path_synced(partial);
                let _ = remove_path_synced(checkpoint);
                return Err(error);
            }
            if path_exists(archive) {
                remove_path_synced(archive)?;
            }
            rename_synced(partial, archive)?;
            remove_path_synced(checkpoint)?;
        }
        Ok(())
    }
}

fn parse_content_range(value: &str) -> Option<(u64, u64, u64)> {
    let value = value.trim();
    let value = value.strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?, total.parse().ok()?))
}

fn path_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn regular_file_length(path: &Path) -> Option<u64> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() {
        return None;
    }
    Some(metadata.len())
}

fn sync_directory(directory: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(directory)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
    }
    Ok(())
}

/// Persist the directory entry containing path. Android/Linux use a real
/// directory fsync; Windows has no portable equivalent for this host-side
/// test path, so the helper is deliberately a no-op there.
fn sync_parent_directory(path: &Path) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    sync_directory(parent)
}

fn rename_synced(from: &Path, to: &Path) -> anyhow::Result<()> {
    fs::rename(from, to)?;
    sync_parent_directory(from)?;
    sync_parent_directory(to)?;
    Ok(())
}

fn remove_path_synced(path: &Path) -> anyhow::Result<()> {
    if !path_exists(path) {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    sync_parent_directory(path)
}

fn next_available_path(base: &Path, prefix: &str) -> PathBuf {
    let first = base.join(prefix);
    if !path_exists(&first) {
        return first;
    }
    for index in 1u64.. {
        let candidate = base.join(format!("{prefix}.{index}"));
        if !path_exists(&candidate) {
            return candidate;
        }
    }
    unreachable!("exhausted Portal recovery path names")
}

fn quarantine_path(base: &Path, source: &Path, prefix: &str) -> anyhow::Result<()> {
    if !path_exists(source) {
        return Ok(());
    }
    let destination = next_available_path(base, prefix);
    rename_synced(source, &destination)
}

/// Shared durable file replacement for Portal-owned native state.
///
/// Renderer selection lives beside the app files rather than inside the
/// Debian tree, but it has the same crash-safety requirements as the runtime
/// markers. Keep both callers on this one implementation so Android/Linux
/// always get same-directory temp creation, atomic rename-over-existing, and
/// parent-directory durability.
pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    write_atomic_with(path, contents, replace_atomic)
}

fn write_atomic_with<F>(
    path: &Path,
    contents: &[u8],
    replace: F,
) -> anyhow::Result<()>
where
    F: FnOnce(&Path, &Path) -> io::Result<()>,
{
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Path has no filename: {}", path.display()))?
        .to_string_lossy();
    let temporary = parent.join(format!(".{name}.tmp"));
    let mut file = fs::File::create(&temporary)?;
    file.write_all(contents)?;
    file.sync_all()?;
    // Unix rename replaces the destination as one directory operation. The
    // old marker therefore remains visible if the process dies before this
    // point, and the new marker is the only visible value afterwards. The
    // Windows fallback is isolated because its rename contract differs.
    replace(&temporary, path)?;
    sync_parent_directory(path)?;
    Ok(())
}

#[cfg(not(windows))]
fn replace_atomic(temporary: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_atomic(temporary: &Path, destination: &Path) -> io::Result<()> {
    // Windows does not provide the same rename-over-existing behavior used by
    // Android/Linux. This branch is only for host tests and development; the
    // production Android path above never unlinks the old marker first.
    if fs::symlink_metadata(destination).is_ok() {
        fs::remove_file(destination)?;
    }
    fs::rename(temporary, destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(directory: &Path, version: &str) -> (RuntimeArtifact, PathBuf) {
        let archive = directory.join(format!("{version}.tar.xz"));
        let encoder = xz2::write::XzEncoder::new(fs::File::create(&archive).unwrap(), 1);
        let mut tar = tar::Builder::new(encoder);
        for (path, value) in [
            (IMAGE_MARKER, version),
            ("usr/lib/os-release", "ID=debian\nVERSION_ID=\"13\"\n"),
            ("usr/bin/dpkg", "binary"),
            ("usr/bin/apt", "binary"),
            ("usr/bin/bash", "binary"),
            ("usr/bin/kwin_wayland", "binary"),
            ("usr/bin/plasmashell", "binary"),
            ("usr/bin/python3", "binary"),
            ("var/lib/dpkg/status", "Package: dpkg\n"),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(value.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, path, value.as_bytes()).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        let bytes = fs::read(&archive).unwrap();
        (
            RuntimeArtifact {
                version: version.to_string(),
                url: "http://127.0.0.1:1/not-used".to_string(),
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                compressed_bytes: bytes.len() as u64,
                source_commit: None,
            },
            archive,
        )
    }

    fn extract_image(artifact: &RuntimeArtifact, archive: &Path, root: &Path) {
        artifact.extract(archive, root, &|_| {}).unwrap();
    }

    fn bootable_image(artifact: &RuntimeArtifact, archive: &Path, root: &Path) {
        extract_image(artifact, archive, root);
        artifact.mark_installation_complete(root).unwrap();
    }

    fn legacy_image(artifact: &RuntimeArtifact, archive: &Path, root: &Path) {
        extract_image(artifact, archive, root);
        fs::remove_file(root.join(IMAGE_READY_MARKER)).unwrap();
        fs::write(
            root.join(READY_MARKER),
            format!("{}\n{}\n", artifact.version, artifact.sha256),
        )
        .unwrap();
    }

    fn pending_name(base: &Path) -> PathBuf {
        base.join(PREVIOUS_PENDING_PREFIX)
    }

    #[test]
    fn atomic_legacy_marker_migration_keeps_old_marker_if_replace_is_interrupted() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join(READY_MARKER);
        let old = b"debian-v1\n0000000000000000000000000000000000000000000000000000000000000000\n";
        let new = b"debian-v1\n0000000000000000000000000000000000000000000000000000000000000000\nportal-installation-v1\n";
        fs::write(&marker, old).unwrap();

        let error = write_atomic_with(&marker, new, |_temporary, _destination| {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "fault injected before rename",
            ))
        });
        assert!(error.is_err());
        assert_eq!(fs::read(&marker).unwrap(), old);

        write_atomic(&marker, new).unwrap();
        assert_eq!(fs::read(&marker).unwrap(), new);
        assert!(!path_exists(&temp.path().join(".portal-runtime-complete.tmp")));
    }

    #[cfg(not(windows))]
    #[test]
    fn atomic_marker_is_either_old_or_new_when_post_rename_durability_fails() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join(READY_MARKER);
        let old = b"debian-v1\n0000000000000000000000000000000000000000000000000000000000000000\n";
        let new = b"debian-v1\n0000000000000000000000000000000000000000000000000000000000000000\nportal-installation-v1\n";
        fs::write(&marker, old).unwrap();

        let error = write_atomic_with(&marker, new, |temporary, destination| {
            fs::rename(temporary, destination)?;
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "fault injected after atomic replacement",
            ))
        });
        assert!(error.is_err());
        // Once Unix rename has happened, the destination is the complete new
        // marker; there is never a missing-marker interval to observe.
        assert_eq!(fs::read(&marker).unwrap(), new);
    }

    #[test]
    fn parent_directory_sync_is_host_testable() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("marker");
        fs::write(&path, b"durable").unwrap();
        sync_parent_directory(&path).unwrap();
        sync_directory(temp.path()).unwrap();
    }

    #[test]
    fn runtime_classification_requires_portal_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let (artifact, archive) = fixture(temp.path(), "classification-v1");
        let root = temp.path().join("runtime-B");

        extract_image(&artifact, &archive, &root);
        assert_eq!(
            artifact.classify_runtime(&root),
            RuntimeClassification::ValidatedImageOnly
        );

        fs::remove_file(root.join(IMAGE_READY_MARKER)).unwrap();
        assert_eq!(
            artifact.classify_runtime(&root),
            RuntimeClassification::Unknown,
            "a Debian-shaped tree without a Portal marker is not trusted"
        );

        fs::write(
            root.join(READY_MARKER),
            format!("{}\n{}\n", artifact.version, artifact.sha256),
        )
        .unwrap();
        assert_eq!(
            artifact.classify_runtime(&root),
            RuntimeClassification::LegacyPortal
        );

        fs::write(
            root.join(READY_MARKER),
            format!(
                "{}\n{}\n{}\n",
                artifact.version, artifact.sha256, INSTALLATION_MARKER
            ),
        )
        .unwrap();
        assert_eq!(
            artifact.classify_runtime(&root),
            RuntimeClassification::BootablePortal
        );

        fs::remove_file(root.join("usr/bin/apt")).unwrap();
        assert_eq!(
            artifact.classify_runtime(&root),
            RuntimeClassification::Invalid
        );
    }

    #[test]
    fn unmarked_structural_current_is_quarantined_without_replacing_previous() {
        let temp = tempfile::tempdir().unwrap();
        let (artifact, archive) = fixture(temp.path(), "unknown-current-v1");
        let root = temp.path().join("runtime-B");
        let previous = temp.path().join(PREVIOUS_RUNTIME);
        let staging = temp.path().join("runtime-B.staging");

        extract_image(&artifact, &archive, &root);
        fs::remove_file(root.join(IMAGE_READY_MARKER)).unwrap();
        fs::write(root.join("current-user-data"), "keep unknown tree").unwrap();
        assert_eq!(
            artifact.classify_runtime(&root),
            RuntimeClassification::Unknown
        );

        bootable_image(&artifact, &archive, &previous);
        fs::write(previous.join("previous-only"), "proven-good").unwrap();
        extract_image(&artifact, &archive, &staging);

        artifact.promote_staging(temp.path(), &root, &staging).unwrap();

        assert!(artifact.is_image_ready(&root));
        assert_eq!(
            artifact.classify_runtime(&previous),
            RuntimeClassification::BootablePortal
        );
        assert_eq!(
            fs::read_to_string(previous.join("previous-only")).unwrap(),
            "proven-good"
        );
        assert!(!root.join("current-user-data").exists());
        let unknown = fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(UNKNOWN_RUNTIME_PREFIX)
            })
            .expect("unknown current must be quarantined");
        assert_eq!(
            fs::read_to_string(unknown.join("current-user-data")).unwrap(),
            "keep unknown tree"
        );
    }

    #[test]
    fn legacy_current_preserves_stronger_bootable_previous() {
        let temp = tempfile::tempdir().unwrap();
        let (artifact, archive) = fixture(temp.path(), "legacy-current-v1");
        let root = temp.path().join("runtime-B");
        let previous = temp.path().join(PREVIOUS_RUNTIME);
        let staging = temp.path().join("runtime-B.staging");

        legacy_image(&artifact, &archive, &root);
        fs::write(root.join("legacy-user-data"), "keep legacy tree").unwrap();
        assert_eq!(
            artifact.classify_runtime(&root),
            RuntimeClassification::LegacyPortal
        );
        bootable_image(&artifact, &archive, &previous);
        fs::write(previous.join("modern-recovery"), "keep modern recovery").unwrap();
        extract_image(&artifact, &archive, &staging);

        artifact.promote_staging(temp.path(), &root, &staging).unwrap();

        assert!(artifact.is_image_ready(&root));
        assert!(artifact.is_bootable(&previous));
        assert_eq!(
            fs::read_to_string(previous.join("modern-recovery")).unwrap(),
            "keep modern recovery"
        );
        let preserved = fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(PRESERVED_LEGACY_RUNTIME_PREFIX)
            })
            .expect("legacy Portal runtime must be preserved separately");
        assert!(artifact.is_legacy_complete(&preserved));
        assert_eq!(
            fs::read_to_string(preserved.join("legacy-user-data")).unwrap(),
            "keep legacy tree"
        );
    }

    #[test]
    fn image_only_current_never_replaces_bootable_previous() {
        let temp = tempfile::tempdir().unwrap();
        let (artifact, archive) = fixture(temp.path(), "image-only-current-v1");
        let root = temp.path().join("runtime-B");
        let previous = temp.path().join(PREVIOUS_RUNTIME);
        let staging = temp.path().join("runtime-B.staging");

        extract_image(&artifact, &archive, &root);
        assert_eq!(
            artifact.classify_runtime(&root),
            RuntimeClassification::ValidatedImageOnly
        );
        bootable_image(&artifact, &archive, &previous);
        fs::write(previous.join("previous-only"), "proven-good").unwrap();
        extract_image(&artifact, &archive, &staging);

        artifact.promote_staging(temp.path(), &root, &staging).unwrap();

        assert!(artifact.is_image_ready(&root));
        assert!(artifact.is_bootable(&previous));
        assert_eq!(
            fs::read_to_string(previous.join("previous-only")).unwrap(),
            "proven-good"
        );
    }

    #[test]
    fn pending_recovery_with_unknown_current_preserves_previous_and_pending() {
        let temp = tempfile::tempdir().unwrap();
        let (artifact, archive) = fixture(temp.path(), "unknown-pending-v1");
        let root = temp.path().join("runtime-B");
        let previous = temp.path().join(PREVIOUS_RUNTIME);
        let pending = pending_name(temp.path());
        let staging = temp.path().join("runtime-B.staging");

        extract_image(&artifact, &archive, &root);
        fs::remove_file(root.join(IMAGE_READY_MARKER)).unwrap();
        fs::write(root.join("unknown-current"), "preserve me").unwrap();
        bootable_image(&artifact, &archive, &previous);
        fs::write(previous.join("previous-only"), "proven-good").unwrap();
        let pending_source = temp.path().join("pending-source");
        bootable_image(&artifact, &archive, &pending_source);
        fs::rename(&pending_source, &pending).unwrap();
        fs::write(pending.join("pending-only"), "proven-pending").unwrap();
        extract_image(&artifact, &archive, &staging);

        artifact.promote_staging(temp.path(), &root, &staging).unwrap();

        assert!(artifact.is_image_ready(&root));
        assert!(artifact.is_bootable(&previous));
        assert!(artifact.is_bootable(&pending));
        assert_eq!(
            fs::read_to_string(previous.join("previous-only")).unwrap(),
            "proven-good"
        );
        assert_eq!(
            fs::read_to_string(pending.join("pending-only")).unwrap(),
            "proven-pending"
        );
    }

    #[test]
    fn valid_previous_survives_invalid_current_promotion() {
        let temp = tempfile::tempdir().unwrap();
        let (artifact, archive) = fixture(temp.path(), "promotion-v1");
        let root = temp.path().join("runtime-B");
        let previous = temp.path().join(PREVIOUS_RUNTIME);
        let staging = temp.path().join("runtime-B.staging");

        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("untrusted-current"), "keep separate").unwrap();
        bootable_image(&artifact, &archive, &previous);
        fs::write(previous.join("previous-only"), "known-good").unwrap();
        extract_image(&artifact, &archive, &staging);

        artifact.promote_staging(temp.path(), &root, &staging).unwrap();

        assert!(artifact.is_image_ready(&root));
        assert!(artifact.is_bootable(&previous));
        assert_eq!(
            fs::read_to_string(previous.join("previous-only")).unwrap(),
            "known-good"
        );
        assert!(!root.join("untrusted-current").exists());
        assert!(!fs::read_dir(temp.path())
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(INVALID_RUNTIME_PREFIX)));
    }

    #[test]
    fn valid_current_and_previous_rotate_only_after_staging_is_validated() {
        let temp = tempfile::tempdir().unwrap();
        let (artifact, archive) = fixture(temp.path(), "promotion-v2");
        let root = temp.path().join("runtime-B");
        let previous = temp.path().join(PREVIOUS_RUNTIME);
        let staging = temp.path().join("runtime-B.staging");

        bootable_image(&artifact, &archive, &root);
        fs::write(root.join("current-only"), "current").unwrap();
        bootable_image(&artifact, &archive, &previous);
        fs::write(previous.join("previous-only"), "previous").unwrap();
        extract_image(&artifact, &archive, &staging);

        artifact.promote_staging(temp.path(), &root, &staging).unwrap();

        assert!(artifact.is_image_ready(&root));
        assert!(artifact.is_bootable(&previous));
        assert_eq!(
            fs::read_to_string(previous.join("current-only")).unwrap(),
            "current"
        );
        assert!(!previous.join("previous-only").exists());
        assert!(path_exists(&pending_name(temp.path())));
        artifact.mark_installation_complete(&root).unwrap();
        assert!(!path_exists(&pending_name(temp.path())));
    }

    #[test]
    fn interrupted_promotion_boundaries_converge_without_losing_valid_roots() {
        // Process death after parking the old previous root.
        {
            let temp = tempfile::tempdir().unwrap();
            let (artifact, archive) = fixture(temp.path(), "boundary-park");
            let root = temp.path().join("runtime-B");
            let previous = temp.path().join(PREVIOUS_RUNTIME);
            let staging = temp.path().join("runtime-B.staging");
            bootable_image(&artifact, &archive, &root);
            bootable_image(&artifact, &archive, &previous);
            fs::rename(&previous, pending_name(temp.path())).unwrap();
            extract_image(&artifact, &archive, &staging);

            artifact.promote_staging(temp.path(), &root, &staging).unwrap();
            assert!(artifact.is_image_ready(&root));
            assert!(artifact.is_bootable(&previous));
            assert!(path_exists(&pending_name(temp.path())));
            artifact.mark_installation_complete(&root).unwrap();
            assert!(!path_exists(&pending_name(temp.path())));
        }

        // Process death after moving the valid current root to previous.
        {
            let temp = tempfile::tempdir().unwrap();
            let (artifact, archive) = fixture(temp.path(), "boundary-rotate");
            let root = temp.path().join("runtime-B");
            let previous = temp.path().join(PREVIOUS_RUNTIME);
            let pending = pending_name(temp.path());
            let staging = temp.path().join("runtime-B.staging");
            bootable_image(&artifact, &archive, &previous);
            fs::rename(&previous, &pending).unwrap();
            bootable_image(&artifact, &archive, &root);
            extract_image(&artifact, &archive, &staging);
            // The current root is valid, the old valid backup is pending, and
            // the next promotion must keep both until the staged image lands.
            artifact.promote_staging(temp.path(), &root, &staging).unwrap();
            assert!(artifact.is_image_ready(&root));
            assert!(artifact.is_bootable(&previous));
            assert!(path_exists(&pending));
            artifact.mark_installation_complete(&root).unwrap();
            assert!(!path_exists(&pending));
        }

        // Process death immediately after staging became the live image.
        {
            let temp = tempfile::tempdir().unwrap();
            let (artifact, archive) = fixture(temp.path(), "boundary-commit");
            let root = temp.path().join("runtime-B");
            let previous = temp.path().join(PREVIOUS_RUNTIME);
            let pending = pending_name(temp.path());
            let staging = temp.path().join("runtime-B.staging");
            let old_previous = temp.path().join("runtime-old-previous");
            bootable_image(&artifact, &archive, &root);
            bootable_image(&artifact, &archive, &previous);
            bootable_image(&artifact, &archive, &old_previous);
            fs::rename(&old_previous, &pending).unwrap();
            fs::remove_dir_all(&root).unwrap();
            extract_image(&artifact, &archive, &staging);
            fs::rename(&staging, &root).unwrap();
            // This is the next-launch recovery path: provision checks the
            // pending rotation before accepting the image-ready live root.
            artifact
                .provision(temp.path(), |_| panic!("committed staging must not download"))
                .unwrap();
            assert!(artifact.is_image_ready(&root));
            assert!(artifact.is_bootable(&previous));
            assert!(path_exists(&pending));
            artifact.mark_installation_complete(&root).unwrap();
            assert!(!path_exists(&pending));
        }
    }
}
