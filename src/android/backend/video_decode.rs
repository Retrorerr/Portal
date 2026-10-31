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
//! Opt-in while experimental: `files/hw-video-decode` must contain `on` and
//! `files/va-layer/termuxva_drv_video.so` must exist. Without the daemon the
//! driver fails `vaInitialize` at once, so clients fall back to software.

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
/// Guest directory the VA layer is bound over (`LIBVA_DRIVERS_PATH`).
const GUEST_LAYER_DIR: &str = "/usr/local/lib/portal/va";
/// Guest path of the daemon socket; the host side is `<rootfs>/tmp/termux-va`.
const GUEST_SOCKET: &str = "/tmp/termux-va/termux-va.sock";

static SUPERVISOR_STARTED: AtomicBool = AtomicBool::new(false);

fn layer_dir() -> PathBuf {
    Path::new(config::APP_FILES_ROOT).join("va-layer")
}

/// Whether the user turned hardware video decoding on and the VA layer is
/// installed.
pub fn enabled() -> bool {
    let flag = Path::new(config::APP_FILES_ROOT).join("hw-video-decode");
    fs::read_to_string(flag).is_ok_and(|value| value.trim() == "on")
        && layer_dir().join(DRIVER_FILE).exists()
}

/// Guest environment selecting the termux-va VA driver.
pub fn guest_env() -> Vec<(String, String)> {
    if !enabled() {
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
    if !enabled() {
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

/// Start the decode daemon and restart it whenever it exits. The daemon
/// serves any number of sessions and holds no codec while idle, so it runs
/// for the life of the process.
pub fn spawn_after_ready() {
    if !enabled() || SUPERVISOR_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let binary = get_application_context().native_library_dir.join(DAEMON_LIB);
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
