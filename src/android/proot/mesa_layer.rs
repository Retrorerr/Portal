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
//!
//! Transaction model (`mesa-kgsl-layer` / `.staging` / `.previous`):
//! the completion marker is invalidated BEFORE the working tree is
//! touched, staging is fully validated before promotion, the previous
//! good tree is parked (not deleted) until the replacement validates
//! again post-rename and the marker is written. A crash between
//! `target -> previous` and `staging -> target` is recovered on the next
//! launch (promote valid staging, else restore previous). The last
//! known-good tree is never deleted before its replacement proves out.
//! Validation is authoritative in `crate::core::mesa_layer`
//! (`validate_layer_dir`), derived from `session_binds()` so the two
//! cannot silently diverge. Marker alone is never proof.
//!
//! Pin identity: every layer carries an internal `.portal-mesa-identity`
//! file (version + archive SHA, written at extraction). A layer counts as
//! current only when structure, internal identity, and the external
//! marker all agree on the pins below, so a future pin change can never
//! relabel an old tree. Installs predating the identity file migrate
//! in place only when their external marker already proves the same
//! current pin.

use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Pinned upstream release (lfdevs mesa-for-android-container). Every byte
/// is hard-pinned; any mismatch fails closed and retries later.
pub const LAYER_VERSION: &str = "26.3.0-20260824";
pub const LAYER_URL: &str = "https://github.com/lfdevs/mesa-for-android-container/releases/download/mesa-26.3.0-devel-20260824/mesa-for-android-container_26.3.0-devel-20260824_debian_trixie_arm64.tar.gz";
pub const LAYER_COMPRESSED_BYTES: u64 = 11648933;
pub const LAYER_SHA256: &str = "c014cf66bdbff96417ee30d34f006cf51df64ae04893d599711b0b6b73b52ccf";
/// Completion marker beside the layer (version + sha, exact match required).
pub const LAYER_MARKER: &str = "mesa-kgsl-layer.complete";

/// Network policy: bounded retries, generous per-attempt budget so slow
/// connections do not randomly fail. Heavy work runs in a spawned setup
/// stage (see `setup_mesa_layer`), never inline on the launch path.
const DOWNLOAD_ATTEMPTS: u32 = 3;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(300);

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

fn files_root() -> PathBuf {
    Path::new(crate::core::config::APP_FILES_ROOT).to_path_buf()
}

/// True when the exact pinned layer is fully staged: marker matches
/// exactly AND the authoritative runtime chain validates as the current
/// pin (structure + internal identity).
pub fn is_provisioned() -> bool {
    crate::core::mesa_layer::is_provisioned_at(&files_root(), LAYER_VERSION, LAYER_SHA256)
}

fn wanted(archive_path: &str) -> bool {
    let stripped = archive_path.strip_prefix("./").unwrap_or(archive_path);
    LAYER_WANTED
        .iter()
        .any(|w| stripped == *w || stripped.starts_with(&format!("{w}/")))
}

/// Legacy entry point (no progress). Prefer `provision_with_progress` from
/// the spawned setup stage so the UI stays live during the ~11 MB fetch.
pub fn provision() {
    provision_with_progress(|_| {});
}

/// Download (fail closed), verify, and transactionally promote the layer.
///
/// Cheap when already provisioned. Reports `checking / downloading /
/// verifying / extracting / promoting` through `report`. Blocking for the
/// calling thread: run it inside a spawned setup stage, never inline on
/// the Plasma launch path. QPainter callers must not reach here at all.
pub fn provision_with_progress(report: impl Fn(String)) {
    if is_provisioned() {
        return;
    }
    if let Err(e) = provision_inner(&report) {
        log::error!(
            "mesa KGSL layer unavailable ({e}); Anland GPU boot will fail at GBM setup, will retry on next launch"
        );
        report(format!("Mesa layer unavailable: {e:#}"));
    }
}

fn provision_inner(report: &impl Fn(String)) -> anyhow::Result<()> {
    let base = files_root();
    report("Checking Mesa KGSL layer…".to_string());

    // Recover an interrupted promotion first: a valid parked/staged tree
    // must not trigger a redundant 11 MB download. Recovery also performs
    // the one-time legacy migration (current-pin marker, identity absent)
    // and repairs a stale marker only when the target already carries the
    // current internal identity.
    match crate::core::mesa_layer::recover_interrupted(&base, LAYER_VERSION, LAYER_SHA256) {
        Ok(crate::core::mesa_layer::Recovery::PromotedStaging) => {
            log::info!("mesa KGSL layer: recovered interrupted promotion (staged -> live)");
            report("Mesa layer recovered after interruption.".to_string());
        }
        Ok(crate::core::mesa_layer::Recovery::RestoredPrevious) => {
            log::info!("mesa KGSL layer: restored previous good tree after interruption");
            report("Mesa layer restored after interruption.".to_string());
        }
        Ok(crate::core::mesa_layer::Recovery::Migrated) => {
            log::info!("mesa KGSL layer: recorded internal identity for pre-identity install (no download)");
            report("Mesa layer already present.".to_string());
        }
        Ok(crate::core::mesa_layer::Recovery::RepairedMarker) => {
            log::info!("mesa KGSL layer: repaired stale marker (tree already valid)");
            report("Mesa layer already present.".to_string());
        }
        Ok(_) => {}
        Err(e) => {
            log::warn!("mesa KGSL layer recovery deferred ({e:#}); will re-provision");
        }
    }
    if is_provisioned() {
        return Ok(());
    }

    let mut last_error = None;
    for attempt in 1..=DOWNLOAD_ATTEMPTS {
        let result = provision_attempt(&base, attempt, report);
        match result {
            Ok(()) => return Ok(()),
            Err(e) => {
                log::warn!("mesa KGSL layer attempt {attempt}/{DOWNLOAD_ATTEMPTS} failed: {e:#}");
                report(format!(
                    "Mesa layer attempt {attempt}/{DOWNLOAD_ATTEMPTS} failed: {e:#}"
                ));
                last_error = Some(e);
            }
        }
    }
    Err(last_error.unwrap())
}

fn provision_attempt(base: &Path, attempt: u32, report: &impl Fn(String)) -> anyhow::Result<()> {
    let paths = crate::core::mesa_layer::layer_paths(base);

    // Reuse an already-verified archive from a previous interrupted run.
    if verify_archive(&paths.archive).is_ok() {
        report("Mesa layer download already verified, reusing…".to_string());
    } else {
        report(format!(
            "Downloading Mesa layer {LAYER_VERSION} (attempt {attempt}/{DOWNLOAD_ATTEMPTS})…"
        ));
        download_archive(&paths.archive)?;
    }

    report("Verifying Mesa layer SHA-256…".to_string());
    verify_archive(&paths.archive)?;

    report("Extracting Mesa layer…".to_string());
    extract_to_staging(&paths.archive, &paths.staging)?;
    // Stamp the staging tree with the current pin before validating: the
    // internal identity is what binds this tree to this exact archive.
    crate::core::mesa_layer::write_identity(&paths.staging, LAYER_VERSION, LAYER_SHA256)?;
    // Authoritative validation of staging (full runtime chain + pin
    // identity, not just two sentinels). Failure here must not touch the
    // live tree.
    crate::core::mesa_layer::validate_layer_dir(&paths.staging, LAYER_VERSION, LAYER_SHA256)
        .map_err(|e| {
            let _ = fs::remove_dir_all(&paths.staging);
            anyhow::anyhow!("extracted layer failed validation: {e:#}")
        })?;

    report("Promoting Mesa layer…".to_string());
    crate::core::mesa_layer::promote_staged(base, LAYER_VERSION, LAYER_SHA256)?;

    // Disposable archive cleanup may log-and-continue only now that a
    // valid target + marker are installed.
    if let Err(e) = fs::remove_file(&paths.archive) {
        log::warn!("mesa KGSL layer: cannot remove archive: {e}");
    }
    log::info!("mesa KGSL layer {LAYER_VERSION} provisioned");
    report("Mesa layer ready.".to_string());
    Ok(())
}

fn verify_archive(archive_path: &Path) -> anyhow::Result<()> {
    let len = fs::metadata(archive_path)
        .map(|m| m.len())
        .map_err(|_| anyhow::anyhow!("archive missing"))?;
    anyhow::ensure!(
        len == LAYER_COMPRESSED_BYTES,
        "size mismatch: got {len}, expected {LAYER_COMPRESSED_BYTES}"
    );
    let mut file = fs::File::open(archive_path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = format!("{:x}", hasher.finalize());
    anyhow::ensure!(digest == LAYER_SHA256, "SHA-256 mismatch: got {digest}");
    Ok(())
}

fn download_archive(archive_path: &Path) -> anyhow::Result<()> {
    // Fresh download per attempt; a corrupt partial must never be resumed
    // as if it were complete (size+SHA gate below fails closed anyway).
    let _ = fs::remove_file(archive_path);
    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(ATTEMPT_TIMEOUT)
        .build()
        .map_err(|e| anyhow::anyhow!("http client: {e}"))?;
    let mut response = client
        .get(LAYER_URL)
        .send()
        .map_err(|e| anyhow::anyhow!("download failed: {e}"))?;
    if !response.status().is_success() {
        anyhow::bail!("download HTTP {}", response.status());
    }
    let mut hasher = Sha256::new();
    let mut bytes: u64 = 0;
    let mut out = fs::File::create(archive_path)?;
    let mut buf = [0u8; 256 * 1024];
    let start = Instant::now();
    let mut last_report = Instant::now();
    loop {
        let n = response.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])?;
        bytes += n as u64;
        anyhow::ensure!(
            bytes <= LAYER_COMPRESSED_BYTES,
            "download exceeds expected size ({bytes} > {LAYER_COMPRESSED_BYTES})"
        );
        if last_report.elapsed() >= Duration::from_secs(2) {
            log::info!(
                "mesa KGSL layer downloading: {bytes}/{LAYER_COMPRESSED_BYTES} bytes ({:.1} MiB, {}s)",
                bytes as f64 / 1048576.0,
                start.elapsed().as_secs()
            );
            last_report = Instant::now();
        }
    }
    out.flush()?;
    let _ = out.sync_all();
    drop(out);
    if bytes != LAYER_COMPRESSED_BYTES {
        let _ = fs::remove_file(archive_path);
        anyhow::bail!("size mismatch: got {bytes}, expected {LAYER_COMPRESSED_BYTES}");
    }
    let digest = format!("{:x}", hasher.finalize());
    if digest != LAYER_SHA256 {
        let _ = fs::remove_file(archive_path);
        anyhow::bail!("SHA-256 mismatch: got {digest}");
    }
    log::info!("mesa KGSL layer download verified ({bytes} bytes, sha256={digest:.16}...)");
    Ok(())
}

fn extract_to_staging(archive_path: &Path, staging: &Path) -> anyhow::Result<()> {
    // Fresh staging every attempt so an incomplete tree is never accepted.
    if fs::symlink_metadata(staging).is_ok() {
        fs::remove_dir_all(staging)?;
    }
    fs::create_dir_all(staging)?;
    let file = fs::File::open(archive_path)?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);
    let mut kept = 0usize;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().replace('\\', "/");
        if !wanted(&path) {
            continue;
        }
        entry.unpack_in(staging)?;
        kept += 1;
    }
    if kept == 0 {
        let _ = fs::remove_dir_all(staging);
        anyhow::bail!("archive contained none of the wanted layer paths");
    }
    log::info!("mesa KGSL layer extracted ({kept} entries) to staging");
    Ok(())
}
