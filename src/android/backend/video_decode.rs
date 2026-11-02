//! Experimental hardware video decoding for the guest (VA-API over MediaCodec).
//!
//! The SoC's V4L2 decoder nodes (`/dev/video32`/`33` on SM8750) are
//! `system:camera 0660`, so an app can't drive the hardware decoder directly.
//! MediaCodec is the only path: `libportal_va_daemon.so` (termux-va, see
//! `third_party/termux-va`) runs in the app context and decodes for guest
//! clients over a Unix socket in the guest's `/tmp`, handing frames back
//! through a shared-memory slot pool. On the guest side, VA-API clients
//! (FFmpeg, mpv, Firefox's RDD process) load the `termuxva` driver from the
//! VA layer: lfdevs Mesa with the termux-va bridge in its VA frontend plus
//! Portal's KGSL patches (`patches/mesa/va-layer-r1`). Its libgallium is
//! named after another Mesa version than the KGSL layer's, so the two load
//! side by side.
//!
//! The driver is a pinned release asset (`.github/workflows/va-layer-arm64.yml`
//! builds it), installed into `files/va-layer` by the setup stage and, for
//! installs that predate it, in the background at launch. On by default;
//! `files/hw-video-decode` containing `off` turns it off. Without the daemon
//! or the driver, `vaInitialize` fails at once and clients fall back to
//! software decoding.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::android::utils::application_context::get_application_context;
use crate::core::config;

const DAEMON_LIB: &str = "libportal_va_daemon.so";
const DRIVER_FILE: &str = "termuxva_drv_video.so";

/// The driver this APK ships with: `patches/mesa/va-layer-r1` built by
/// `.github/workflows/va-layer-arm64.yml`, gzip-compressed. Every byte is
/// pinned; any mismatch fails closed and retries on a later launch.
pub const LAYER_VERSION: &str = "r1-20261101";
const LAYER_URL: &str = "https://github.com/Retrorerr/Portal/releases/download/va-layer-r1-20261101/termuxva_drv_video.so.gz";
const LAYER_COMPRESSED_BYTES: u64 = 3_010_628;
const LAYER_SHA256: &str = "6da4fb42c275d72eb893a4f12844a19913c97dc892f16910631fdb4517c34f5b";
/// Upper bound for the decompressed driver.
const LAYER_MAX_BYTES: u64 = 64 << 20;
/// `<version> <sha256>` of the installed driver, beside the layer directory.
const LAYER_MARKER: &str = "va-layer.complete";
/// Guest directory the VA layer is bound over (`LIBVA_DRIVERS_PATH`).
const GUEST_LAYER_DIR: &str = "/usr/local/lib/portal/va";
/// Guest path of the daemon socket; the host side is `<rootfs>/tmp/termux-va`.
const GUEST_SOCKET: &str = "/tmp/termux-va/termux-va.sock";

static SUPERVISOR_STARTED: AtomicBool = AtomicBool::new(false);

fn layer_dir() -> PathBuf {
    Path::new(config::APP_FILES_ROOT).join("va-layer")
}

fn files_root() -> PathBuf {
    Path::new(config::APP_FILES_ROOT).to_path_buf()
}

/// Whether hardware video decoding is wanted: on unless
/// `files/hw-video-decode` says `off`. The driver exports decoded frames
/// through KGSL only, so other GPUs keep software decoding.
fn wanted() -> bool {
    Path::new(crate::core::drm_nodes::KGSL_DEVICE).exists()
        && fs::read_to_string(files_root().join("hw-video-decode"))
            .map_or(true, |value| value.trim() != "off")
}

fn marker_line() -> String {
    format!("{LAYER_VERSION} {LAYER_SHA256}")
}

/// The pinned driver is in place (marker and file agree).
pub fn layer_installed() -> bool {
    fs::read_to_string(files_root().join(LAYER_MARKER))
        .is_ok_and(|marker| marker.trim() == marker_line())
        && layer_dir().join(DRIVER_FILE).is_file()
}

/// Whether sessions get the VA driver and the daemon runs.
pub fn enabled() -> bool {
    wanted() && layer_installed()
}

/// Guest environment selecting the termux-va VA driver. Set whenever
/// decoding is wanted, so a driver installed during the session works in
/// apps started after it lands; until then libva fails to load the driver
/// and clients decode in software.
pub fn guest_env() -> Vec<(String, String)> {
    if !wanted() {
        return Vec::new();
    }
    vec![
        ("LIBVA_DRIVER_NAME".into(), "termuxva".into()),
        ("LIBVA_DRIVERS_PATH".into(), GUEST_LAYER_DIR.into()),
        ("TERMUX_VA_BRIDGE".into(), "1".into()),
        ("TERMUX_VA_GPU_BACKEND".into(), "kgsl".into()),
        ("TERMUX_VA_SOCKET".into(), GUEST_SOCKET.into()),
    ]
}

/// The VA layer directory, bound over [`GUEST_LAYER_DIR`].
pub fn session_binds() -> Vec<crate::core::runtime::BindMount> {
    if !wanted() || fs::create_dir_all(layer_dir()).is_err() {
        return Vec::new();
    }
    vec![crate::core::runtime::BindMount::new(
        layer_dir(),
        GUEST_LAYER_DIR,
    )]
}

fn socket_dir() -> PathBuf {
    use crate::core::runtime::LinuxRuntime;
    crate::android::runtime::proot::PRootRuntime::active()
        .rootfs_path()
        .join("tmp/termux-va")
}

/// Install the pinned driver when it is missing, then start the decode
/// daemon. Runs in the background; a failed download retries on the next
/// launch.
pub fn spawn_after_ready() {
    if !wanted() {
        return;
    }
    if layer_installed() {
        spawn_daemon();
        return;
    }
    thread::spawn(|| match install_layer() {
        Ok(()) => spawn_daemon(),
        Err(error) => log::warn!("[VideoDecode] VA layer unavailable: {error:#}"),
    });
}

/// Setup stage: install the driver before the desktop first starts. Never
/// fails setup; the launch path retries.
pub fn install_for_setup() {
    if wanted() && !layer_installed() {
        if let Err(error) = install_layer() {
            log::warn!("[VideoDecode] VA layer install deferred to launch: {error:#}");
        }
    }
}

fn install_layer() -> anyhow::Result<()> {
    static INSTALLING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = INSTALLING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if layer_installed() {
        return Ok(());
    }
    let mut attempt = 0;
    loop {
        attempt += 1;
        match install_attempt() {
            Ok(()) => {
                log::info!("[VideoDecode] VA layer {LAYER_VERSION} installed");
                return Ok(());
            }
            Err(error) if attempt < 3 => {
                log::warn!("[VideoDecode] VA layer attempt {attempt}/3 failed: {error:#}");
                thread::sleep(Duration::from_secs(5 * attempt));
            }
            Err(error) => return Err(error),
        }
    }
}

/// Download, verify (size and SHA-256 of the compressed asset), decompress
/// beside the driver and rename it into place, then write the marker. The
/// marker goes first so an interrupted replacement never counts as current.
fn install_attempt() -> anyhow::Result<()> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};

    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(crate::core::provisioning::DOWNLOAD_STALL_TIMEOUT)
        .build()?;
    let mut response = client.get(LAYER_URL).send()?.error_for_status()?;
    let mut compressed = Vec::with_capacity(LAYER_COMPRESSED_BYTES as usize);
    let mut buf = [0u8; 256 * 1024];
    loop {
        let n = response.read(&mut buf)?;
        if n == 0 {
            break;
        }
        compressed.extend_from_slice(&buf[..n]);
        anyhow::ensure!(
            compressed.len() as u64 <= LAYER_COMPRESSED_BYTES,
            "download exceeds {LAYER_COMPRESSED_BYTES} bytes"
        );
    }
    anyhow::ensure!(
        compressed.len() as u64 == LAYER_COMPRESSED_BYTES,
        "size mismatch: got {}, expected {LAYER_COMPRESSED_BYTES}",
        compressed.len()
    );
    let digest = format!("{:x}", Sha256::digest(&compressed));
    anyhow::ensure!(digest == LAYER_SHA256, "SHA-256 mismatch: got {digest}");

    let dir = layer_dir();
    fs::create_dir_all(&dir)?;
    let _ = fs::remove_file(files_root().join(LAYER_MARKER));
    let staging = dir.join(format!("{DRIVER_FILE}.partial"));
    let mut driver = Vec::new();
    flate2::read::GzDecoder::new(compressed.as_slice())
        .take(LAYER_MAX_BYTES + 1)
        .read_to_end(&mut driver)?;
    anyhow::ensure!(
        driver.len() as u64 <= LAYER_MAX_BYTES && driver.starts_with(b"\x7fELF"),
        "decompressed driver is not a plausible ELF library"
    );
    let mut file = fs::File::create(&staging)?;
    file.write_all(&driver)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&staging, dir.join(DRIVER_FILE))?;
    crate::core::mesa_layer::sync_dir_best_effort(&dir);

    let marker = files_root().join(LAYER_MARKER);
    let temporary = marker.with_extension("tmp");
    fs::write(&temporary, marker_line())?;
    fs::rename(&temporary, &marker)?;
    crate::core::mesa_layer::sync_dir_best_effort(&files_root());
    Ok(())
}

/// Start the decode daemon and restart it whenever it exits. The daemon
/// serves any number of sessions and holds no codec while idle, so it runs
/// for the life of the process.
fn spawn_daemon() {
    if SUPERVISOR_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let binary = get_application_context()
        .native_library_dir
        .join(DAEMON_LIB);
    if !binary.exists() {
        log::warn!("[VideoDecode] {DAEMON_LIB} missing; hardware video decoding unavailable");
        return;
    }
    thread::spawn(move || {
        let mut backoff = Duration::from_secs(1);
        loop {
            let dir = socket_dir();
            if let Err(error) = fs::create_dir_all(&dir) {
                log::error!("[VideoDecode] create {}: {error}", dir.display());
            }
            let started = Instant::now();
            let status = Command::new(&binary)
                .arg("--sock")
                .arg(&dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            match status {
                Ok(status) => log::warn!("[VideoDecode] daemon exited: {status}"),
                Err(error) => log::error!("[VideoDecode] spawn {DAEMON_LIB}: {error}"),
            }
            if started.elapsed() > Duration::from_secs(60) {
                backoff = Duration::from_secs(1);
            }
            thread::sleep(backoff);
            backoff = (backoff * 2).min(Duration::from_secs(60));
        }
    });
}
