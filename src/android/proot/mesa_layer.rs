//! Pinned Mesa KGSL layer (`mesa-kgsl-layer` in app files).
//!
//! Stock Debian Mesa has no kgsl winsys at all (its freedreno winsys
//! backends are `[drm]` only), so without this layer no GBM device can ever
//! be created in the sandbox and KWin's Anland backend exits. The layer
//! (lfdevs Mesa 26.3 with the kgsl winsys, plus Portal's patches) is overlaid onto the guest by
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
//!
//! Refreshes: the pins below are the build this APK ships with. Portal's
//! updater may install a newer Portal Mesa release (see
//! `core::update_sources::mesa_update`); its pin is stored beside the layer
//! and wins while it is newer than the built-in one. A refreshed layer is
//! on trial until a hardware Plasma session reaches its first frame: the
//! layer it replaced is parked as `.fallback`, and after
//! [`MAX_TRIAL_LAUNCHES`] launches that never get there, the fallback is put
//! back and the new version is never offered again.

use crate::core::update_sources::{compare_mesa_versions, MesaRelease, MESA_DOWNLOAD_PREFIX};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Built-in release: lfdevs mesa-for-android-container's tag rebuilt with
/// `patches/mesa/kgsl-layer-26.3.0` (KGSL implicit sync, merged-submit BO
/// lifetime, wait timeouts in ms and checked error paths, blitter scissor) by
/// `.github/workflows/mesa-kgsl-layer-arm64.yml`. Every
/// byte is hard-pinned; any mismatch fails closed and retries later.
pub const LAYER_VERSION: &str = "26.3.0-20261015";
pub const LAYER_URL: &str = "https://github.com/Retrorerr/Portal/releases/download/mesa-26.3.0-devel-portal-20261015/mesa-for-android-container_26.3.0-devel-20260824_debian_trixie_arm64.tar.gz";
pub const LAYER_COMPRESSED_BYTES: u64 = 11653498;
pub const LAYER_SHA256: &str = "aa82c6716367ac67143c727ea391e14ee736ee2f620b3488733f9d6785348826";
/// Completion marker beside the layer (version + sha, exact match required).
pub const LAYER_MARKER: &str = "mesa-kgsl-layer.complete";
/// A refreshed pin (JSON [`LayerPin`]); absent means the built-in one.
const PIN_FILE: &str = "mesa-kgsl-layer.pin";
/// The layer a refresh replaced, kept until the new one has drawn a desktop.
const FALLBACK_DIR: &str = "mesa-kgsl-layer.fallback";
const FALLBACK_PIN_FILE: &str = "mesa-kgsl-layer.fallback.pin";
/// JSON [`Trial`] while a refreshed layer has not drawn a desktop yet.
const TRIAL_FILE: &str = "mesa-kgsl-layer.trial";
/// Refreshed versions that never drew a desktop here, one per line.
const DENIED_FILE: &str = "mesa-kgsl-layer.denied";
/// Launches a refreshed layer gets to reach a desktop frame.
pub const MAX_TRIAL_LAUNCHES: u32 = 2;

/// One layer build: what to fetch and the exact bytes it must have.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LayerPin {
    pub version: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

impl LayerPin {
    fn plausible(&self) -> bool {
        self.url.starts_with(MESA_DOWNLOAD_PREFIX)
            && self.size > 0
            && self.size <= 256 << 20
            && self.sha256.len() == 64
            && self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            && compare_mesa_versions(&self.version, &self.version).is_some()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Trial {
    version: String,
    launches: u32,
}

/// The build this APK ships with.
pub fn builtin_pin() -> LayerPin {
    LayerPin {
        version: LAYER_VERSION.to_owned(),
        url: LAYER_URL.to_owned(),
        size: LAYER_COMPRESSED_BYTES,
        sha256: LAYER_SHA256.to_owned(),
    }
}

fn read_pin(path: &Path) -> Option<LayerPin> {
    let pin: LayerPin = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    pin.plausible().then_some(pin)
}

/// The layer Portal should have: a refreshed pin while it is newer than the
/// built-in one (a later APK may ship a newer build itself).
pub fn active_pin() -> LayerPin {
    let builtin = builtin_pin();
    match read_pin(&files_root().join(PIN_FILE)) {
        Some(pin) if compare_mesa_versions(&pin.version, &builtin.version) == Some(Ordering::Greater) => pin,
        _ => builtin,
    }
}

/// Network policy: bounded retries, generous per-attempt budget so slow
/// connections do not randomly fail. Heavy work runs in a spawned setup
/// stage (see `setup_mesa_layer`), never inline on the launch path.
const DOWNLOAD_ATTEMPTS: u32 = 3;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Archive members to extract, relative to the tarball root (`./` stripped).
/// Mirrors `session_binds` exactly: the DRI drivers (kgsl), the GBM backend,
/// EGL/GLX/GBM libraries including SONAME links (the loader resolves
/// DT_NEEDED by SONAME — real-name files alone would silently lose to
/// stock), the freedreno Vulkan ICD reference, and drirc defaults. The
/// versioned gallium core (`libgallium-<mesa>.so`) is matched by [`wanted`].
pub const LAYER_WANTED: &[&str] = &[
    "usr/lib/aarch64-linux-gnu/dri",
    "usr/lib/aarch64-linux-gnu/gbm",
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
    let pin = active_pin();
    crate::core::mesa_layer::is_provisioned_at(&files_root(), &pin.version, &pin.sha256)
}

fn wanted(archive_path: &str) -> bool {
    let stripped = archive_path.strip_prefix("./").unwrap_or(archive_path);
    let gallium = stripped
        .strip_prefix("usr/lib/aarch64-linux-gnu/libgallium-")
        .is_some_and(|rest| rest.ends_with(".so") && !rest.contains('/'));
    gallium
        || LAYER_WANTED
            .iter()
            .any(|w| stripped == *w || stripped.starts_with(&format!("{w}/")))
}

/// Legacy entry point (no progress). Prefer `provision_with_progress` from
/// the spawned setup stage so the UI stays live during the ~11 MB fetch.
pub fn provision() {
    if let Err(error) = provision_with_progress(|_| {}) {
        log::error!("mesa KGSL layer provisioning failed: {error:#}");
    }
}

/// Download (fail closed), verify, and transactionally promote the layer.
///
/// Cheap when already provisioned. Reports `checking / downloading /
/// verifying / extracting / promoting` through `report`. Blocking for the
/// calling thread: run it inside a spawned setup stage, never inline on
/// the Plasma launch path. QPainter callers must not reach here at all.
pub fn provision_with_progress(report: impl Fn(String)) -> anyhow::Result<()> {
    if is_provisioned() {
        return Ok(());
    }
    if let Err(e) = provision_inner(&report) {
        log::error!(
            "mesa KGSL layer unavailable ({e}); setup will remain incomplete and retry safely"
        );
        report(format!("Mesa layer unavailable: {e:#}"));
        return Err(e);
    }
    Ok(())
}

fn provision_inner(report: &impl Fn(String)) -> anyhow::Result<()> {
    let base = files_root();
    let pin = active_pin();
    report("Checking Mesa KGSL layer…".to_string());

    // Recover an interrupted promotion first: a valid parked/staged tree
    // must not trigger a redundant 11 MB download. Recovery also performs
    // the one-time legacy migration (current-pin marker, identity absent)
    // and repairs a stale marker only when the target already carries the
    // current internal identity.
    match crate::core::mesa_layer::recover_interrupted(&base, &pin.version, &pin.sha256) {
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

    // As for the Debian runtime: a lost connection is waited out, only hard
    // failures count against the attempts.
    let mut failures = 0;
    let mut offline_waits = 0;
    loop {
        let e = match provision_attempt(&base, &pin, report) {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        if e.downcast_ref::<crate::core::provisioning::WaitingForNetwork>().is_some() {
            offline_waits += 1;
            log::warn!("mesa KGSL layer download lost its connection: {e:#}; waiting to resume");
            report("Waiting for an internet connection…".to_string());
            std::thread::sleep(crate::core::provisioning::offline_backoff(offline_waits));
            continue;
        }
        failures += 1;
        log::warn!("mesa KGSL layer attempt {failures}/{DOWNLOAD_ATTEMPTS} failed: {e:#}");
        if failures >= DOWNLOAD_ATTEMPTS {
            return Err(e);
        }
        report("Something went wrong; trying again…".to_string());
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn provision_attempt(base: &Path, pin: &LayerPin, report: &impl Fn(String)) -> anyhow::Result<()> {
    let paths = crate::core::mesa_layer::layer_paths(base);

    // Reuse an already-verified archive from a previous interrupted run.
    if verify_archive(&paths.archive, pin).is_ok() {
        report("Mesa layer download already verified, reusing…".to_string());
    } else {
        report("Downloading graphics drivers…".to_string());
        download_archive(&paths.archive, pin, |_, _| {}).map_err(crate::core::provisioning::tag_network_error)?;
    }

    report("Verifying Mesa layer SHA-256…".to_string());
    verify_archive(&paths.archive, pin)?;

    report("Extracting Mesa layer…".to_string());
    extract_to_staging(&paths.archive, &paths.staging)?;
    // Stamp the staging tree with the current pin before validating: the
    // internal identity is what binds this tree to this exact archive.
    crate::core::mesa_layer::write_identity(&paths.staging, &pin.version, &pin.sha256)?;
    // Authoritative validation of staging (full runtime chain + pin
    // identity, not just two sentinels). Failure here must not touch the
    // live tree.
    crate::core::mesa_layer::validate_layer_dir(&paths.staging, &pin.version, &pin.sha256)
        .map_err(|e| {
            let _ = fs::remove_dir_all(&paths.staging);
            anyhow::anyhow!("extracted layer failed validation: {e:#}")
        })?;

    report("Promoting Mesa layer…".to_string());
    crate::core::mesa_layer::promote_staged(base, &pin.version, &pin.sha256)?;

    // Disposable archive cleanup may log-and-continue only now that a
    // valid target + marker are installed.
    if let Err(e) = fs::remove_file(&paths.archive) {
        log::warn!("mesa KGSL layer: cannot remove archive: {e}");
    }
    log::info!("mesa KGSL layer {} provisioned", pin.version);
    report("Mesa layer ready.".to_string());
    Ok(())
}

fn verify_archive(archive_path: &Path, pin: &LayerPin) -> anyhow::Result<()> {
    let len = fs::metadata(archive_path)
        .map(|m| m.len())
        .map_err(|_| anyhow::anyhow!("archive missing"))?;
    anyhow::ensure!(
        len == pin.size,
        "size mismatch: got {len}, expected {}",
        pin.size
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
    anyhow::ensure!(digest == pin.sha256, "SHA-256 mismatch: got {digest}");
    Ok(())
}

fn download_archive(
    archive_path: &Path,
    pin: &LayerPin,
    mut progress: impl FnMut(u64, u64),
) -> anyhow::Result<()> {
    // Fresh download per attempt; a corrupt partial must never be resumed
    // as if it were complete (size+SHA gate below fails closed anyway).
    let _ = fs::remove_file(archive_path);
    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(crate::core::provisioning::DOWNLOAD_STALL_TIMEOUT)
        .tcp_keepalive(Duration::from_secs(15))
        .build()
        .map_err(|e| anyhow::anyhow!("http client: {e}"))?;
    // Keep the reqwest error itself in the chain so a lost connection is
    // recognised as one.
    let mut response = client
        .get(&pin.url)
        .send()
        .map_err(|e| anyhow::Error::new(e).context("download failed"))?;
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
            bytes <= pin.size,
            "download exceeds expected size ({bytes} > {})",
            pin.size
        );
        progress(bytes, pin.size);
        if last_report.elapsed() >= Duration::from_secs(2) {
            log::info!(
                "mesa KGSL layer downloading: {bytes}/{} bytes ({:.1} MiB, {}s)",
                pin.size,
                bytes as f64 / 1048576.0,
                start.elapsed().as_secs()
            );
            last_report = Instant::now();
        }
    }
    out.flush()?;
    let _ = out.sync_all();
    drop(out);
    if bytes != pin.size {
        let _ = fs::remove_file(archive_path);
        anyhow::bail!("size mismatch: got {bytes}, expected {}", pin.size);
    }
    let digest = format!("{:x}", hasher.finalize());
    if digest != pin.sha256 {
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

fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes)?;
    crate::core::mesa_layer::sync_file_best_effort(&temporary);
    fs::rename(&temporary, path)?;
    if let Some(parent) = path.parent() {
        crate::core::mesa_layer::sync_dir_best_effort(parent);
    }
    Ok(())
}

fn read_trial(base: &Path) -> Option<Trial> {
    serde_json::from_str(&fs::read_to_string(base.join(TRIAL_FILE)).ok()?).ok()
}

fn write_trial(base: &Path, trial: &Trial) -> anyhow::Result<()> {
    write_atomic(&base.join(TRIAL_FILE), serde_json::to_string(trial)?.as_bytes())
}

/// Make `pin` the active one: the built-in pin is the absence of a file.
fn store_pin(base: &Path, pin: &LayerPin) -> anyhow::Result<()> {
    let path = base.join(PIN_FILE);
    if *pin == builtin_pin() {
        match fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    } else {
        write_atomic(&path, serde_json::to_string(pin)?.as_bytes())
    }
}

/// Versions that failed to draw a desktop here; the updater skips them.
pub fn denied_versions() -> Vec<String> {
    fs::read_to_string(files_root().join(DENIED_FILE))
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a refreshed layer is still waiting for its first desktop frame.
pub fn trial_pending() -> bool {
    read_trial(&files_root()).is_some()
}

/// Replace the provisioned layer with `release`. The current layer is parked
/// as the fallback until a hardware session draws a desktop on the new one
/// ([`trial_after_desktop_ready`]), and comes back by itself if it never
/// does ([`trial_before_launch`]). Plasma must not be running: its
/// processes have the current driver mapped.
pub fn apply_refresh(
    release: &MesaRelease,
    mut progress: impl FnMut(u16, String),
) -> anyhow::Result<()> {
    let base = files_root();
    let paths = crate::core::mesa_layer::layer_paths(&base);
    let current = active_pin();
    let next = LayerPin {
        version: release.version.clone(),
        url: release.download.url.clone(),
        size: release.download.size,
        sha256: release.download.sha256.clone(),
    };
    anyhow::ensure!(
        next.plausible(),
        "the graphics driver release is not a usable Portal Mesa build"
    );
    if compare_mesa_versions(&next.version, &current.version) != Some(Ordering::Greater) {
        return Ok(());
    }
    anyhow::ensure!(
        read_trial(&base).is_none(),
        "the previous graphics driver update has not started Plasma yet"
    );
    anyhow::ensure!(
        is_provisioned(),
        "the current graphics driver is incomplete; Portal repairs it on the next launch"
    );

    progress(0, "Downloading the graphics driver…".to_owned());
    let mut failures = 0;
    let mut offline_waits = 0;
    loop {
        let result = download_archive(&paths.archive, &next, |done, total| {
            progress(
                (done * 80 / total.max(1)) as u16,
                "Downloading the graphics driver…".to_owned(),
            )
        })
        .map_err(crate::core::provisioning::tag_network_error);
        let Err(error) = result else { break };
        // A dropped connection is waited out for a few minutes; a bad
        // response or a checksum mismatch counts against the attempts.
        if error
            .downcast_ref::<crate::core::provisioning::WaitingForNetwork>()
            .is_some()
            && offline_waits < 20
        {
            offline_waits += 1;
            progress(0, "Waiting for an internet connection…".to_owned());
            std::thread::sleep(crate::core::provisioning::offline_backoff(offline_waits));
            continue;
        }
        failures += 1;
        if failures >= DOWNLOAD_ATTEMPTS {
            return Err(error.context("could not download the graphics driver"));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    verify_archive(&paths.archive, &next)?;

    progress(85, "Unpacking the graphics driver…".to_owned());
    extract_to_staging(&paths.archive, &paths.staging)?;
    crate::core::mesa_layer::write_identity(&paths.staging, &next.version, &next.sha256)?;
    if let Err(error) =
        crate::core::mesa_layer::validate_layer_dir(&paths.staging, &next.version, &next.sha256)
    {
        let _ = fs::remove_dir_all(&paths.staging);
        anyhow::bail!("the new graphics driver is incomplete: {error:#}");
    }

    progress(95, "Installing the graphics driver…".to_owned());
    // Order matters for crash recovery: the trial exists before the pin
    // moves, so a half-done swap is always either undone or verified.
    let fallback = base.join(FALLBACK_DIR);
    if fs::symlink_metadata(&fallback).is_ok() {
        fs::remove_dir_all(&fallback)?;
    }
    write_atomic(
        &base.join(FALLBACK_PIN_FILE),
        serde_json::to_string(&current)?.as_bytes(),
    )?;
    fs::rename(&paths.target, &fallback)?;
    let promoted = write_trial(
        &base,
        &Trial {
            version: next.version.clone(),
            launches: 0,
        },
    )
    .and_then(|()| store_pin(&base, &next))
    .and_then(|()| crate::core::mesa_layer::promote_staged(&base, &next.version, &next.sha256));
    if let Err(error) = promoted {
        restore_fallback(&base, None);
        return Err(error.context("could not install the new graphics driver"));
    }
    let _ = fs::remove_file(&paths.archive);
    log::info!(
        "mesa KGSL layer refreshed {} -> {} (on trial)",
        current.version,
        next.version
    );
    crate::android::diagnostics::host_event(
        "mesa-layer-refresh",
        &format!("{} -> {}", current.version, next.version),
    );
    progress(100, "Graphics driver installed".to_owned());
    Ok(())
}

/// Put the parked layer back and forget the trial. With `deny`, that
/// version is never offered again.
fn restore_fallback(base: &Path, deny: Option<&str>) {
    let paths = crate::core::mesa_layer::layer_paths(base);
    let fallback = base.join(FALLBACK_DIR);
    let pin = read_pin(&base.join(FALLBACK_PIN_FILE)).unwrap_or_else(builtin_pin);
    let usable =
        crate::core::mesa_layer::validate_layer_dir(&fallback, &pin.version, &pin.sha256).is_ok();
    if usable {
        let _ = fs::remove_dir_all(&paths.target);
        let _ = fs::remove_file(&paths.marker);
        if let Err(error) = fs::rename(&fallback, &paths.target) {
            log::error!("mesa KGSL layer: could not restore the previous driver: {error}");
        } else if let Err(error) = crate::core::mesa_layer::write_marker_best_effort(
            &paths.marker,
            &pin.version,
            &pin.sha256,
        ) {
            log::error!("mesa KGSL layer: could not mark the restored driver: {error:#}");
        }
    } else {
        // Nothing to go back to: drop the bad tree so the next setup run
        // provisions the previous pin from scratch.
        log::error!(
            "mesa KGSL layer: no usable fallback; the previous driver will be downloaded again"
        );
        let _ = fs::remove_dir_all(&paths.target);
        let _ = fs::remove_file(&paths.marker);
        let _ = fs::remove_dir_all(&fallback);
    }
    if let Err(error) = store_pin(base, &pin) {
        log::error!("mesa KGSL layer: could not restore the previous pin: {error:#}");
    }
    if let Some(version) = deny {
        let path = base.join(DENIED_FILE);
        let mut denied = fs::read_to_string(&path).unwrap_or_default();
        if !denied.lines().any(|line| line.trim() == version) {
            denied.push_str(version);
            denied.push('\n');
            let _ = write_atomic(&path, denied.as_bytes());
        }
    }
    let _ = fs::remove_file(base.join(FALLBACK_PIN_FILE));
    let _ = fs::remove_file(base.join(TRIAL_FILE));
}

/// Before a hardware session starts: count the launches of a refreshed
/// layer that has not drawn a desktop, and put the previous layer back once
/// it has had [`MAX_TRIAL_LAUNCHES`]. Also undoes a refresh that stopped
/// halfway (a crash between parking the old layer and moving the pin).
pub fn trial_before_launch() {
    let base = files_root();
    let Some(mut trial) = read_trial(&base) else {
        return;
    };
    if active_pin().version != trial.version {
        log::warn!(
            "mesa KGSL layer: an interrupted refresh to {} is undone",
            trial.version
        );
        restore_fallback(&base, None);
        return;
    }
    if !is_provisioned() {
        log::warn!(
            "mesa KGSL layer: refreshed driver {} is incomplete; restoring",
            trial.version
        );
        restore_fallback(&base, Some(&trial.version));
        return;
    }
    if trial.launches >= MAX_TRIAL_LAUNCHES {
        log::error!(
            "mesa KGSL layer: {} never drew a desktop in {} launches; restoring the previous driver",
            trial.version,
            trial.launches
        );
        crate::android::diagnostics::host_event("mesa-layer-rollback", &trial.version);
        restore_fallback(&base, Some(&trial.version));
        return;
    }
    trial.launches += 1;
    if let Err(error) = write_trial(&base, &trial) {
        log::warn!("mesa KGSL layer: could not count the trial launch: {error:#}");
    }
}

/// A hardware session drew a desktop: a refreshed layer is proven, so the
/// parked one goes.
pub fn trial_after_desktop_ready() {
    let base = files_root();
    let Some(trial) = read_trial(&base) else {
        return;
    };
    if active_pin().version != trial.version || !is_provisioned() {
        return;
    }
    let _ = fs::remove_dir_all(base.join(FALLBACK_DIR));
    let _ = fs::remove_file(base.join(FALLBACK_PIN_FILE));
    let _ = fs::remove_file(base.join(TRIAL_FILE));
    log::info!(
        "mesa KGSL layer {} drew a desktop; refresh confirmed",
        trial.version
    );
    crate::android::diagnostics::host_event("mesa-layer-confirmed", &trial.version);
}
