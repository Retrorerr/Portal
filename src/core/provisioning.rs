//! One release image, verified before extraction and atomically promoted.
//!
//! The archive and runtime are deliberately separate transactions.  A
//! `.part` file is only a resumable byte stream, `portal-runtime.tar.xz` is
//! created only after a complete size/hash check, and a staged tree is only
//! an image candidate until it carries `IMAGE_READY_MARKER`.  The final
//! `READY_MARKER` is written by the setup owner after all required guest
//! configuration (including the pinned Mesa layer) has succeeded.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
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

    /// Kept as the image-stage compatibility name used by existing callers.
    /// It intentionally does not imply a bootable Portal installation.
    pub fn is_ready(&self, root: &Path) -> bool {
        self.is_image_ready(root)
    }

    pub fn verify(&self, archive: &Path) -> anyhow::Result<()> {
        anyhow::ensure!(
            fs::metadata(archive)?.len() == self.compressed_bytes,
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
        if staging.exists() {
            fs::remove_dir_all(staging)?;
        }
        fs::create_dir_all(staging)?;
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
        Ok(())
    }

    /// Write the final installation truth only after all required setup work
    /// has completed. It is idempotent and upgrades the old two-line marker
    /// without changing the mutable runtime tree.
    pub fn mark_installation_complete(&self, root: &Path) -> anyhow::Result<()> {
        if self.is_bootable(root) {
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
        self.validate_compatible(root)
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

        fs::create_dir_all(base)?;
        let staging = base.join("runtime-B.staging");

        // A previous attempt may have crashed between `rename(root, previous)`
        // and `rename(staging, root)`. Promote a fully validated image without
        // downloading again. Legacy two-line staging markers are accepted only
        // when the exact image itself still validates.
        if self.is_staged_image(&staging) {
            self.emit(
                &report,
                ProvisioningPhase::Promoting,
                68,
                "Recovering the validated Debian runtime…",
            );
            self.promote_staging(base, &root, &staging)?;
            let _ = fs::remove_file(base.join("portal-runtime.tar.xz"));
            self.emit(
                &report,
                ProvisioningPhase::Configuring,
                70,
                "Debian runtime committed; configuring Portal…",
            );
            return Ok(());
        }

        // Incomplete staging is never launchable and extraction replaces it.
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
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
                if let Err(error) = fs::remove_file(&archive) {
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

    fn promote_staging(&self, base: &Path, root: &Path, staging: &Path) -> anyhow::Result<()> {
        // The staging tree has already passed exact image validation. Keep the
        // old root in `.previous` until the new rename succeeds; never delete
        // the known-good root before its replacement proves out.
        if root.exists() {
            let previous = base.join("runtime-B.previous");
            if previous.exists() {
                fs::remove_dir_all(&previous)?;
            }
            fs::rename(root, &previous)?;
        }
        fs::rename(staging, root)?;
        Ok(())
    }

    fn normalise_cached_archive(
        &self,
        archive: &Path,
        partial: &Path,
        checkpoint: &Path,
    ) -> anyhow::Result<()> {
        if self.verify(archive).is_ok() {
            let _ = fs::remove_file(partial);
            let _ = fs::remove_file(checkpoint);
            return Ok(());
        }
        if archive.exists() {
            let length = fs::metadata(archive).map(|m| m.len()).unwrap_or(0);
            if length > 0 && length < self.compressed_bytes && !partial.exists() {
                fs::rename(archive, partial)?;
                // Older Portal builds used the final archive name while
                // downloading and synced the file at the end of each
                // response. Carry that resumable prefix forward; current
                // writes add a stricter checkpoint after every synced window.
                write_atomic(checkpoint, length.to_string().as_bytes())?;
            } else {
                let _ = fs::remove_file(archive);
            }
        }
        if let Ok(length) = fs::metadata(partial).map(|m| m.len()) {
            if length > self.compressed_bytes {
                fs::remove_file(partial)?;
                let _ = fs::remove_file(checkpoint);
            } else if length == self.compressed_bytes {
                // A full-size `.part` is still untrusted. Reuse it only when
                // the exact pinned digest proves it is the archive; otherwise
                // discard the ambiguous full-size prefix before requesting a
                // new range.
                if self.verify(partial).is_ok() {
                    fs::rename(partial, archive)?;
                    let _ = fs::remove_file(checkpoint);
                } else {
                    fs::remove_file(partial)?;
                    let _ = fs::remove_file(checkpoint);
                }
            }
        } else {
            let _ = fs::remove_file(checkpoint);
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
        let actual_length = fs::metadata(partial)
            .map(|m| m.len().min(self.compressed_bytes))
            .unwrap_or(0);
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
            let _ = fs::remove_file(checkpoint);
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
                let _ = fs::remove_file(partial);
                let _ = fs::remove_file(checkpoint);
                anyhow::bail!("Server returned more bytes than its Content-Range proved");
            }
            anyhow::ensure!(
                downloaded == expected_end,
                "Server ended before its Content-Range was complete"
            );
        }
        anyhow::ensure!(
            fs::metadata(partial)?.len() <= self.compressed_bytes,
            "Runtime partial download exceeds expected size"
        );
        if fs::metadata(partial)?.len() == self.compressed_bytes {
            self.emit(
                report,
                ProvisioningPhase::Downloading,
                45,
                "Debian runtime download complete; checking it…",
            );
            // A full-size prefix is still not a final archive until this verify
            // succeeds. Move it atomically only after the exact check.
            if let Err(error) = self.verify(partial) {
                let _ = fs::remove_file(partial);
                let _ = fs::remove_file(checkpoint);
                return Err(error);
            }
            if archive.exists() {
                fs::remove_file(archive)?;
            }
            fs::rename(partial, archive)?;
            let _ = fs::remove_file(checkpoint);
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

fn write_atomic(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
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
    // Android filesystems generally replace atomically. Windows does not allow
    // rename-over-existing, so remove only this disposable marker target if
    // necessary; a crash in that tiny gap leaves an incomplete, non-bootable
    // installation which the next launch repairs.
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&temporary, path)?;
    Ok(())
}
