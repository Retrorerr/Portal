//! One release image, verified before extraction and atomically promoted.
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

#[derive(Debug, Deserialize)]
pub struct RuntimeArtifact {
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub compressed_bytes: u64,
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
    ///
    /// This remains strict for downloaded/staged content.  A mutable runtime
    /// that has already been installed is checked by `is_bootable` instead so
    /// a normal APK update does not throw away the user's Debian packages.
    pub fn validate_image(&self, root: &Path) -> anyhow::Result<()> {
        anyhow::ensure!(
            fs::read_to_string(root.join(IMAGE_MARKER))?.trim() == self.version,
            "Runtime artifact version mismatch"
        );
        Self::validate_debian_layout(root)
    }

    fn validate_completion_marker(root: &Path) -> anyhow::Result<()> {
        let marker = fs::read_to_string(root.join(READY_MARKER))?;
        let mut lines = marker.lines();
        let version = lines.next().unwrap_or_default().trim();
        let digest = lines.next().unwrap_or_default().trim();
        anyhow::ensure!(
            !version.is_empty(),
            "Runtime completion marker has no version"
        );
        anyhow::ensure!(
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Runtime completion marker has no SHA-256 identity"
        );
        Ok(())
    }

    /// Validate a persistent Debian 13 runtime without requiring the current
    /// APK's pinned artifact identity.  The completion marker is deliberately
    /// still required: an unmarked or partial tree must be reprovisioned, not
    /// treated as a mutable installation.
    pub fn validate_compatible(&self, root: &Path) -> anyhow::Result<()> {
        Self::validate_completion_marker(root)?;
        Self::validate_debian_layout(root)
    }

    pub fn is_bootable(&self, root: &Path) -> bool {
        self.validate_compatible(root).is_ok()
    }

    pub fn is_ready(&self, root: &Path) -> bool {
        fs::read_to_string(root.join(READY_MARKER)).ok().as_deref()
            == Some(self.identity().as_str())
            && self.validate_image(root).is_ok()
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
        let mut marker = fs::File::create(staging.join(READY_MARKER))?;
        marker.write_all(self.identity().as_bytes())?;
        marker.sync_all()?;
        Ok(())
    }

    pub fn provision(&self, base: &Path, report: impl Fn(String)) -> anyhow::Result<()> {
        let root = base.join("runtime-B");
        // runtime-B is mutable user state.  Once it is a complete Debian 13
        // installation, keep it across Portal APK/image revisions; artifact
        // identity is only strict for staging/download validation below.
        if self.is_bootable(&root) {
            return Ok(());
        }
        // A completion marker means Portal previously claimed ownership of a
        // complete runtime.  Never silently replace a marked-but-corrupt tree:
        // it may contain user packages/configuration and needs an explicit
        // recovery path rather than destructive reprovisioning.
        if fs::symlink_metadata(root.join(READY_MARKER)).is_ok() {
            anyhow::bail!(
                "Existing Debian runtime is marked complete but failed validation; refusing to replace user data"
            );
        }
        fs::create_dir_all(base)?;
        let staging = base.join("runtime-B.staging");
        // A previous attempt may have crashed between `rename(root, previous)`
        // and `rename(staging, root)`, leaving a valid READY-marked staging
        // directory behind. Promote it directly instead of re-downloading.
        if self.is_ready(&staging) {
            report("Resuming interrupted Debian runtime setup…".into());
            if root.exists() {
                let previous = base.join("runtime-B.previous");
                if previous.exists() {
                    let _ = fs::remove_dir_all(&previous);
                }
                fs::rename(&root, previous)?;
            }
            fs::rename(&staging, &root)?;
            let _ = fs::remove_file(base.join("portal-runtime.tar.xz"));
            return Ok(());
        }
        // Incomplete staging is never launchable and extract() replaces it.
        // Reclaim it before preflight so a retry does not count it twice.
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        // Allow payload allocation overhead as well as the compressed download.
        // Existing runtime/backup data is already excluded from available bytes.
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
            let needed = self
                .compressed_bytes
                .saturating_mul(5)
                .saturating_add(512 * 1024 * 1024)
                .saturating_sub(
                    fs::metadata(base.join("portal-runtime.tar.xz"))
                        .map(|m| m.len().min(self.compressed_bytes))
                        .unwrap_or(0),
                );
            anyhow::ensure!(available >= needed,
                "Not enough storage: free {} MiB; Portal needs {} MiB available. Free space and retry setup.",
                available / 1048576, needed / 1048576);
        }
        let archive = base.join("portal-runtime.tar.xz");
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60 * 60))
            .build()?;
        let mut last_error = None;
        for attempt in 1..=3 {
            let result = (|| -> anyhow::Result<()> {
                if self.verify(&archive).is_err() {
                    report(format!("Downloading Debian runtime (attempt {attempt}/3)"));
                    let offset = fs::metadata(&archive).map(|m| m.len()).unwrap_or(0);
                    let offset = if offset < self.compressed_bytes {
                        offset
                    } else {
                        0
                    };
                    let mut request = client.get(&self.url);
                    if offset > 0 {
                        request =
                            request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
                    }
                    let mut response = request.send()?.error_for_status()?;
                    let resume =
                        offset > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
                    if resume {
                        let expected = format!("bytes {offset}-");
                        anyhow::ensure!(
                            response
                                .headers()
                                .get(reqwest::header::CONTENT_RANGE)
                                .and_then(|h| h.to_str().ok())
                                .is_some_and(|h| h.starts_with(&expected)),
                            "Server returned an invalid download range"
                        );
                    }
                    let mut file = fs::OpenOptions::new()
                        .create(true)
                        .write(true)
                        .append(resume)
                        .truncate(!resume)
                        .open(&archive)?;
                    let mut buffer = [0u8; 256 * 1024];
                    let mut downloaded = if resume { offset } else { 0 };
                    let mut last = Instant::now();
                    loop {
                        let count = response.read(&mut buffer)?;
                        if count == 0 {
                            break;
                        }
                        downloaded += count as u64;
                        anyhow::ensure!(
                            downloaded <= self.compressed_bytes,
                            "Runtime exceeds expected size"
                        );
                        file.write_all(&buffer[..count])?;
                        if last.elapsed() >= Duration::from_secs(1) {
                            report(format!(
                                "Downloading Debian runtime: {} / {} MiB",
                                downloaded / 1048576,
                                self.compressed_bytes / 1048576
                            ));
                            last = Instant::now();
                        }
                    }
                    file.sync_all()?;
                }
                report("Verifying Debian runtime SHA-256…".into());
                self.extract(&archive, &staging, &report)?;
                // Preserve an old developer/previous-version runtime explicitly, never boot it.
                if root.exists() {
                    let previous = base.join("runtime-B.previous");
                    if previous.exists() {
                        fs::remove_dir_all(&previous)?;
                    }
                    fs::rename(&root, previous)?;
                }
                fs::rename(&staging, &root)?;
                let _ = fs::remove_file(&archive);
                // Retain one previous runtime for recovery; the next upgrade
                // rotates it before moving the current root into its place.
                report("Debian runtime ready. Configuring Portal…".into());
                Ok(())
            })();
            match result {
                Ok(()) => return Ok(()),
                Err(error) => {
                    report(format!(
                        "Debian runtime attempt {attempt}/3 failed: {error:#}"
                    ));
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap())
    }
}
