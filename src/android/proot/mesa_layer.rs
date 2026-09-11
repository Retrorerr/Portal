//! Pinned lfdevs Mesa KGSL layer (`mesa-kgsl-layer` in app files).
//!
//! Stock Debian Mesa has no kgsl winsys at all (its freedreno winsys
//! backends are `[drm]` only), so without this layer no GBM device can ever
//! be created in the sandbox and KWin's Anland backend exits. The layer
//! (lfdevs Mesa 26.3 with the kgsl winsys) is overlaid onto the guest by
//! `session_binds`, which skips absent files — so this provisioning is what
//! makes hardware sessions possible. Downloaded once (marker-gated),
//! verified (size + SHA-256, fail closed), subset-extracted with symlinks
//! preserved. QPainter sessions never pay for it (gated on the renderer
//! flag); a failed download retries on a later launch and the session
//! fails closed at GBM setup with a diagnosable kwin log.

use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// Pinned upstream release (lfdevs mesa-for-android-container). Every byte
/// is hard-pinned; any mismatch fails closed and retries later.
pub const LAYER_VERSION: &str = "26.3.0-20260824";
pub const LAYER_URL: &str = "https://github.com/lfdevs/mesa-for-android-container/releases/download/mesa-26.3.0-devel-20260824/mesa-for-android-container_26.3.0-devel-20260824_debian_trixie_arm64.tar.gz";
pub const LAYER_COMPRESSED_BYTES: u64 = 11648933;
pub const LAYER_SHA256: &str =
    "c014cf66bdbff96417ee30d34f006cf51df64ae04893d599711b0b6b73b52ccf";
/// Completion marker beside the layer (version + sha, exact match required).
pub const LAYER_MARKER: &str = "mesa-kgsl-layer.complete";

/// Archive members to extract, relative to the tarball root (`./` stripped).
/// Mirrors `session_binds` exactly: the DRI drivers (kgsl), the GBM backend,
/// gallium/EGL/GLX/GBM libraries including SONAME links (the loader resolves
/// DT_NEEDED by SONAME — real-name files alone would silently lose to
/// stock), the freedreno Vulkan ICD reference, and drirc defaults.
pub const LAYER_WANTED: &[&str] = &[
    "usr/lib/aarch64-linux-gnu/dri",
    "usr/lib/aarch64-linux-gnu/gbm",
    "usr/lib/aarch64-linux-gnu/libgallium-26.3.0-devel.so",
    "usr/lib/aarch64-linux-gnu/libvulkan_freedreno.so",
    "usr/lib/aarch64-linux-gnu/libEGL_mesa.so.0.0.0",
    "usr/lib/aarch64-linux-gnu/libEGL_mesa.so.0",
    "usr/lib/aarch64-linux-gnu/libEGL_mesa.so",
    "usr/lib/aarch64-linux-gnu/libGLX_mesa.so.0.0.0",
    "usr/lib/aarch64-linux-gnu/libGLX_mesa.so.0",
    "usr/lib/aarch64-linux-gnu/libGLX_mesa.so",
    "usr/lib/aarch64-linux-gnu/libgbm.so.1.0.0",
    "usr/lib/aarch64-linux-gnu/libgbm.so.1",
    "usr/lib/aarch64-linux-gnu/libgbm.so",
    "usr/share/vulkan/icd.d",
    "usr/share/drirc.d",
];

fn layer_dir() -> PathBuf {
    Path::new(crate::core::config::APP_FILES_ROOT).join("mesa-kgsl-layer")
}

fn marker_path() -> PathBuf {
    Path::new(crate::core::config::APP_FILES_ROOT).join(LAYER_MARKER)
}

fn marker_content() -> String {
    format!("{LAYER_VERSION}\n{LAYER_SHA256}\n")
}

/// True when the exact pinned layer is fully staged.
pub fn is_provisioned() -> bool {
    fs::read_to_string(marker_path())
        .map(|s| s == marker_content())
        .unwrap_or(false)
        && layer_dir()
            .join("usr/lib/aarch64-linux-gnu/dri/kgsl_dri.so")
            .is_file()
}

fn wanted(archive_path: &str) -> bool {
    let stripped = archive_path.strip_prefix("./").unwrap_or(archive_path);
    LAYER_WANTED.iter().any(|w| {
        stripped == *w || stripped.starts_with(&format!("{w}/"))
    })
}

/// Download (fail closed), verify, and atomically promote the layer.
/// Blocking: Anland hardware sessions cannot boot without it, and there is
/// no graceful degradation (KWin exits at GBM setup). Runs at most once per
/// install thanks to the marker; QPainter sessions never reach here.
pub fn provision() {
    if is_provisioned() {
        return;
    }
    if let Err(e) = provision_inner() {
        log::error!(
            "mesa KGSL layer unavailable ({e}); Anland GPU boot will fail at GBM setup, will retry on next launch"
        );
    }
}

fn provision_inner() -> anyhow::Result<()> {
    let files = Path::new(crate::core::config::APP_FILES_ROOT);
    let archive_path = files.join("mesa-kgsl-layer.tar.gz");
    let staging = files.join("mesa-kgsl-layer.staging");
    let target = layer_dir();

    log::info!("mesa KGSL layer {LAYER_VERSION}: downloading ({LAYER_COMPRESSED_BYTES} bytes)...");
    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| anyhow::anyhow!("http client: {e}"))?
        .get(LAYER_URL)
        .send()
        .map_err(|e| anyhow::anyhow!("download failed: {e}"))?;
    if !response.status().is_success() {
        anyhow::bail!("download HTTP {}", response.status());
    }
    let mut hasher = Sha256::new();
    let mut bytes: u64 = 0;
    let mut out = fs::File::create(&archive_path)?;
    let mut stream = response;
    let mut buf = [0u8; 1024 * 256];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])?;
        bytes += n as u64;
    }
    drop(out);
    if bytes != LAYER_COMPRESSED_BYTES {
        let _ = fs::remove_file(&archive_path);
        anyhow::bail!("size mismatch: got {bytes}, expected {LAYER_COMPRESSED_BYTES}");
    }
    let digest = format!("{:x}", hasher.finalize());
    if digest != LAYER_SHA256 {
        let _ = fs::remove_file(&archive_path);
        anyhow::bail!("SHA-256 mismatch: got {digest}");
    }
    log::info!("mesa KGSL layer download verified ({bytes} bytes, sha256={digest:.16}...)");

    // Extract the wanted subset (symlinks preserved) into staging, then
    // atomically promote over any previous (possibly hand-placed) tree so
    // the on-disk layer always matches the pins exactly.
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    let file = fs::File::open(&archive_path)?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);
    let mut kept = 0usize;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().replace('\\', "/");
        if !wanted(&path) {
            continue;
        }
        entry.unpack_in(&staging)?;
        kept += 1;
    }
    if kept == 0 {
        anyhow::bail!("archive contained none of the wanted layer paths");
    }
    // Sanity: the kgsl driver and gallium core must have survived.
    for sentinel in [
        "usr/lib/aarch64-linux-gnu/dri/kgsl_dri.so",
        "usr/lib/aarch64-linux-gnu/libgallium-26.3.0-devel.so",
    ] {
        if !staging.join(sentinel).exists() {
            anyhow::bail!("extracted layer is missing sentinel {sentinel}");
        }
    }
    let _ = fs::remove_dir_all(&target);
    fs::rename(&staging, &target)?;
    let _ = fs::remove_file(&archive_path);
    fs::write(marker_path(), marker_content())?;
    log::info!("mesa KGSL layer {LAYER_VERSION} provisioned ({kept} entries)");
    Ok(())
}
