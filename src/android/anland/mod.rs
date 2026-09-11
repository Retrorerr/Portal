//! Project Anland GPU renderer (Android-only).
//!
//! Zero-copy Plasma presentation path, kept strictly beside the stable
//! Smithay/QPainter renderer:
//!
//! ```text
//! Plasma apps -> KWin OpenGL -> Mesa freedreno/KGSL -> Android dma-buf
//!   -> GPU sync-file fence -> queueBuffer -> SurfaceFlinger
//! ```
//!
//! Selection is explicit and file-gated (mirroring the `presenter-mode` /
//! `touch-mode` patterns): `<APP_FILES>/renderer-mode` containing `anland`
//! selects this path; anything else (or a missing file) keeps the
//! known-good QPainter renderer. There is deliberately **no silent
//! fallback**: a failed GPU bring-up logs a clear diagnostic and stops,
//! never corrupting the stable path.
//!
//! Upstream reference: `third_party/anland/` (protocol, broker/consumer
//! design, hidden window ABI) + `docs/anland-*.md` as added.

pub mod anw;
pub mod broker;
pub mod consumer;
pub mod protocol;
pub mod sys;

pub use consumer::{AnlandConfig, AnlandSession};

use std::ffi::c_void;
use std::sync::Arc;

/// Renderer selection. `Smithay` is the preserved QPainter fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererKind {
    Smithay,
    Anland,
}

/// App-private flag file selecting the renderer (content `anland`).
pub fn mode_flag_path() -> std::path::PathBuf {
    std::path::Path::new(crate::core::config::APP_FILES_ROOT).join("renderer-mode")
}

/// Resolve the active renderer. Logs exactly which path is selected so
/// performance validation can never mistake one for the other.
pub fn active_renderer() -> RendererKind {
    let kind = std::fs::read_to_string(mode_flag_path())
        .map(|s| s.trim().to_ascii_lowercase())
        .map(|s| {
            if s == "anland" {
                RendererKind::Anland
            } else {
                RendererKind::Smithay
            }
        })
        .unwrap_or(RendererKind::Smithay);
    match kind {
        RendererKind::Anland => {
            log::info!("anland.renderer=anland-gpu selected via renderer-mode flag (QPainter fallback preserved on main)");
        }
        RendererKind::Smithay => {
            log::info!("anland.renderer=smithay-qpainter (stable fallback; write 'anland' to renderer-mode to try the GPU path)");
        }
    }
    kind
}

pub fn is_anland_requested() -> bool {
    matches!(active_renderer(), RendererKind::Anland)
}

/// Host side of the broker socket, bound into the guest at
/// [`guest_socket_path`].
pub fn host_socket_path() -> std::path::PathBuf {
    std::path::Path::new(crate::core::config::APP_FILES_ROOT).join("anland/display.sock")
}

/// Guest-visible broker socket (via the session's `--bind`).
pub fn guest_socket_path() -> &'static str {
    "/tmp/anland/display.sock"
}

/// Mesa freedreno/KGSL environment for the Anland guest session.
///
/// This mirrors the upstream `anland-termux` session recipe
/// (`MESA_LOADER_DRIVER_OVERRIDE=kgsl TURNIP_KMD=kgsl GALLIUM_DRIVER=freedreno
/// FD_FORCE_KGSL=1 XWAYLAND_FORCE_KGSL_SURFACELESS=1`). `EGL_PLATFORM` is
/// deliberately NOT set (it forces QtQuick software fallback with
/// OffscreenQuickView texture failures).
///
/// The hardware path needs three guest-side pieces beyond these variables:
/// the lfdevs KWin stack (runtime overlay), the `mesa-kgsl-layer` overlay
/// (lfdevs Mesa 26.3 with the kgsl winsys; stock Mesa has no kgsl winsys at
/// all), and the `drmshim.so` preload, which presents the (sandbox-denied)
/// render node backed by the real /dev/kgsl-3d0 and answers the DRM version
/// probe so Mesa selects the kgsl winsys. `ANLAND_NO_DRM_DEVICE` is NOT set
/// here: the accelerated configuration is the default.
///
/// Emergency software fallback: if `<guest>/var/lib/localdesktop/kwin-glmode`
/// contains `sw`, the KWin wrapper forces surfaceless software rendering
/// instead (see `software_gl_fallback_requested`). That mode is explicitly
/// labelled everywhere (bare-frame READY evidence, relaxed first-frame
/// watchdog) and must never silently become the default.
/// - `XWAYLAND_FORCE_KGSL_SURFACELESS=1`: the `-91` XWayland's KGSL glamor
///   backend instead of GBM (which cannot work without a render node).
///   Proven: `Xwayland glamor: using KGSL surfaceless EGL backend`.
/// - `MOZ_ENABLE_WAYLAND=1`: explicit native backend for Firefox
///   (per-launch overrides can still force X11 for A/B tests).
pub fn guest_mesa_env() -> Vec<(String, String)> {
    vec![
        ("MESA_LOADER_DRIVER_OVERRIDE".into(), "kgsl".into()),
        ("GALLIUM_DRIVER".into(), "freedreno".into()),
        ("FD_FORCE_KGSL".into(), "1".into()),
        // PR #85 opt-in: expose linear dma-buf import/export on KGSL.
        ("FD_KGSL_ENABLE_DMABUF".into(), "1".into()),
        ("TURNIP_KMD".into(), "kgsl".into()),
        ("XWAYLAND_FORCE_KGSL_SURFACELESS".into(), "1".into()),
        // Firefox backend selection: X11. Proven by A/B (about:support via
        // Marionette): native Wayland = WebRender (Software) — its dmabuf
        // compositor needs GBM/a render node (absent in PRoot, no override
        // possible). X11 + KGSL glamor + forced WR prefs (see setup.rs
        // sync_firefox_config) = real GPU `Compositing: WebRender` on
        // Adreno 830 with correct rendering (screenshot-verified).
        ("MOZ_ENABLE_WAYLAND".into(), "0".into()),
        // XInput2 for X11 clients: KWin forwards native touch through the
        // xwayland-touch XI2 device (direct touch, 20 slots, server-verified
        // via xinput). Without this Firefox X11 only sees KWin's pointer
        // emulation (tap works, drag/scroll/pinch don't).
        ("MOZ_USE_XINPUT2".into(), "1".into()),
        // GTK input method: IBus. The Portal IBus engine bridges X11/GTK
        // editable focus to the Android IME (real FocusIn/FocusOut, commit,
        // delete, enter). Qt/Wayland clients are unaffected (QT_IM_MODULE
        // unset keeps native Wayland text-input).
        ("GTK_IM_MODULE".into(), "ibus".into()),
        ("ANLAND_SOCKET".into(), guest_socket_path().into()),
        ("ANLAND".into(), "1".into()),
        ("ANLAND_SKIP_IMPLICIT_SYNC_WAIT".into(), "1".into()),
        // The audio engine has no host counterpart yet; skip it entirely.
        ("ANLAND_DISABLE_AUDIO".into(), "1".into()),
        ("KWIN_GL_DEBUG".into(), "1".into()),
    ]
}

/// Guest flag selecting the emergency software-GL fallback (`sw`) instead of
/// the default hardware-accelerated path. Read by the KWin wrapper (which
/// forces the surfaceless environment) and mirrored host-side so readiness
/// and watchdog policy match the actual renderer. Absent (or anything else)
/// means hardware: genuine fenced evidence is required as usual.
pub fn kwin_glmode_flag_path() -> std::path::PathBuf {
    std::path::Path::new(crate::core::config::PRODUCTION_FS_ROOT)
        .join("var/lib/localdesktop/kwin-glmode")
}

/// True only when the explicit emergency software fallback is requested.
/// The hardware path (default) keeps the tight fence watchdog and requires
/// fenced READY evidence; the software path (llvmpipe, CPU-synchronous)
/// labels bare frames honestly and allows a long cold first-frame budget.
pub fn software_gl_fallback_requested() -> bool {
    std::fs::read_to_string(kwin_glmode_flag_path())
        .map(|s| s.trim().to_ascii_lowercase() == "sw")
        .unwrap_or(false)
}

/// Bind mounts for the Anland guest session: broker socket dir + Mesa overlay.
pub fn session_binds() -> Vec<crate::core::runtime::BindMount> {
    use crate::core::runtime::BindMount;
    let files = std::path::Path::new(crate::core::config::APP_FILES_ROOT);
    let mesa = files.join("mesa-kgsl-layer");
    let mut binds = vec![BindMount::new(files.join("anland"), "/tmp/anland")];
    // Mesa overlay: lfdevs Mesa 26.3 (kgsl winsys) over stock paths. Each
    // entry is skipped when the layer file is absent so QPainter sessions
    // and layer-less installs are unaffected (KWin then cannot do GPU and
    // the session fails closed instead of silently falling back).
    let lib = mesa.join("usr/lib/aarch64-linux-gnu");
    let share = mesa.join("usr/share");
    let pairs = [
        (lib.join("dri"), "/usr/lib/aarch64-linux-gnu/dri"),
        (
            lib.join("libgallium-26.3.0-devel.so"),
            "/usr/lib/aarch64-linux-gnu/libgallium-26.3.0-devel.so",
        ),
        (
            lib.join("libvulkan_freedreno.so"),
            "/usr/lib/aarch64-linux-gnu/libvulkan_freedreno.so",
        ),
        (
            lib.join("libEGL_mesa.so.0.0.0"),
            "/usr/lib/aarch64-linux-gnu/libEGL_mesa.so.0.0.0",
        ),
        // Matched GLX dispatch for X11 clients: stock glvnd libGL stays,
        // but it must load the layer's libGLX_mesa (26.3) so the DRI driver
        // (layer kgsl_dri 26.3) matches its loader. Without this, GLX falls
        // back to llvmpipe (proven via glxinfo on :1).
        (
            lib.join("libGLX_mesa.so.0.0.0"),
            "/usr/lib/aarch64-linux-gnu/libGLX_mesa.so.0.0.0",
        ),
        (
            lib.join("libgbm.so.1.0.0"),
            "/usr/lib/aarch64-linux-gnu/libgbm.so.1.0.0",
        ),
        // SONAME entries: the loader resolves DT_NEEDED by SONAME, so the
        // real-name files above are dead weight unless these names resolve
        // to the layer too (otherwise stock libgbm/EGL/GLX silently win and
        // the layer never engages; proven via LD_DEBUG).
        (
            lib.join("libgbm.so.1"),
            "/usr/lib/aarch64-linux-gnu/libgbm.so.1",
        ),
        (
            lib.join("libEGL_mesa.so.0"),
            "/usr/lib/aarch64-linux-gnu/libEGL_mesa.so.0",
        ),
        (
            lib.join("libGLX_mesa.so.0"),
            "/usr/lib/aarch64-linux-gnu/libGLX_mesa.so.0",
        ),
        (lib.join("gbm"), "/usr/lib/aarch64-linux-gnu/gbm"),
        (share.join("vulkan/icd.d"), "/usr/share/vulkan/icd.d"),
        (share.join("drirc.d"), "/usr/share/drirc.d"),
    ];
    for (host, guest) in pairs {
        if host.exists() {
            binds.push(BindMount::new(host, guest));
        }
    }
    binds
}

/// Create the winit window for Anland mode and return it with the raw
/// `ANativeWindow` pointer. No EGL surface is created: the window stays on
/// the CPU API for `dequeueBuffer`/`queueBuffer`.
pub fn create_window(
    event_loop: &winit::event_loop::ActiveEventLoop,
) -> Result<(Arc<winit::window::Window>, *mut c_void), String> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    #[allow(deprecated)]
    let window = Arc::new(
        event_loop
            .create_window(winit::window::WindowAttributes::default())
            .map_err(|e| format!("Anland create_window failed: {e}"))?,
    );
    let handle = window
        .window_handle()
        .map(|h| h.as_raw())
        .map_err(|e| format!("Anland window handle failed: {e}"))?;
    match handle {
        RawWindowHandle::AndroidNdk(h) => Ok((window, h.a_native_window.as_ptr())),
        other => Err(format!("Anland requires AndroidNdk window, got {other:?}")),
    }
}
