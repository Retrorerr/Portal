//! One release image, verified before extraction and atomically promoted.
use std::{fs, io::{Read, Write}, path::Path, time::{Duration, Instant}};
use serde::Deserialize;
use sha2::{Digest, Sha256};

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

    fn identity(&self) -> String { format!("{}\n{}\n", self.version, self.sha256) }

    pub fn validate_image(&self, root: &Path) -> anyhow::Result<()> {
        anyhow::ensure!(fs::read_to_string(root.join(IMAGE_MARKER))?.trim() == self.version,
            "Runtime artifact version mismatch");
        let os = fs::read_to_string(root.join("usr/lib/os-release"))?;
        anyhow::ensure!(os.lines().any(|l| l == "ID=debian") &&
            os.lines().any(|l| l == "VERSION_ID=\"13\"" || l == "VERSION_ID=13"), "Expected Debian 13");
        for path in ["usr/bin/dpkg", "usr/bin/apt", "usr/bin/bash", "usr/bin/kwin_wayland",
                     "usr/bin/plasmashell", "usr/bin/python3", "var/lib/dpkg/status"] {
            anyhow::ensure!(root.join(path).is_file(), "Runtime missing {path}");
        }
        anyhow::ensure!(!root.join("usr/bin/pacman").exists(), "Unexpected package manager in runtime");
        Ok(())
    }

    pub fn is_ready(&self, root: &Path) -> bool {
        fs::read_to_string(root.join(READY_MARKER)).ok().as_deref() == Some(self.identity().as_str())
            && self.validate_image(root).is_ok()
    }

    pub fn verify(&self, archive: &Path) -> anyhow::Result<()> {
        anyhow::ensure!(fs::metadata(archive)?.len() == self.compressed_bytes, "Runtime download size mismatch");
        let mut file = fs::File::open(archive)?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 256 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 { break; }
            hash.update(&buffer[..count]);
        }
        anyhow::ensure!(format!("{:x}", hash.finalize()) == self.sha256, "Runtime SHA-256 mismatch");
        Ok(())
    }

    pub fn extract(&self, archive: &Path, staging: &Path, report: &impl Fn(String)) -> anyhow::Result<()> {
        self.verify(archive)?;
        if staging.exists() { fs::remove_dir_all(staging)?; }
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
        if self.is_ready(&root) { return Ok(()); }
        fs::create_dir_all(base)?;
        let archive = base.join("portal-runtime.tar.xz");
        let staging = base.join("runtime-B.staging");
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(30)).timeout(Duration::from_secs(60 * 60)).build()?;
        let mut last_error = None;
        for attempt in 1..=3 {
            let result = (|| -> anyhow::Result<()> {
                if self.verify(&archive).is_err() {
                    report(format!("Downloading Debian runtime (attempt {attempt}/3)"));
                    let mut response = client.get(&self.url).send()?.error_for_status()?;
                    let mut file = fs::File::create(&archive)?;
                    let mut buffer = [0u8; 256 * 1024];
                    let mut downloaded = 0u64;
                    let mut last = Instant::now();
                    loop {
                        let count = response.read(&mut buffer)?;
                        if count == 0 { break; }
                        downloaded += count as u64;
                        anyhow::ensure!(downloaded <= self.compressed_bytes, "Runtime exceeds expected size");
                        file.write_all(&buffer[..count])?;
                        if last.elapsed() >= Duration::from_secs(1) {
                            report(format!("Downloading Debian runtime: {} / {} MiB", downloaded / 1048576, self.compressed_bytes / 1048576));
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
                report("Debian runtime ready. Configuring Portal…".into());
                Ok(())
            })();
            match result {
                Ok(()) => return Ok(()),
                Err(error) => {
                    report(format!("Debian runtime attempt {attempt}/3 failed: {error:#}"));
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap())
    }
}
