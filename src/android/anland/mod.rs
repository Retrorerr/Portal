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
/// Stock Debian Mesa 25 cannot drive KGSL (proven: llvmpipe fallback);
/// the staged `mesa-kgsl-layer` overlay plus these variables select the
/// lfdevs freedreno path with linear dma-buf sharing for Anland import.
pub fn guest_mesa_env() -> Vec<(String, String)> {
    vec![
        ("MESA_LOADER_DRIVER_OVERRIDE".into(), "kgsl".into()),
        ("GALLIUM_DRIVER".into(), "kgsl".into()),
        ("FD_FORCE_KGSL".into(), "1".into()),
        // PR #85 opt-in: expose linear dma-buf import/export on KGSL.
        ("FD_KGSL_ENABLE_DMABUF".into(), "1".into()),
        ("ANLAND_SOCKET".into(), guest_socket_path().into()),
        ("ANLAND".into(), "1".into()),
        ("ANLAND_SKIP_IMPLICIT_SYNC_WAIT".into(), "1".into()),
        // The audio engine has no host counterpart yet; skip it entirely.
        ("ANLAND_DISABLE_AUDIO".into(), "1".into()),
        ("KWIN_GL_DEBUG".into(), "1".into()),
    ]
}

/// Bind mounts for the Anland guest session: broker socket dir + Mesa overlay.
pub fn session_binds() -> Vec<crate::core::runtime::BindMount> {
    use crate::core::runtime::BindMount;
    let files = std::path::Path::new(crate::core::config::APP_FILES_ROOT);
    let mesa = files.join("mesa-kgsl-layer");
    let mut binds = vec![BindMount::new(files.join("anland"), "/tmp/anland")];
    // Mesa overlay (mirrors the proven probe binds). Each entry is skipped
    // when the layer file is absent so QPainter sessions are unaffected.
    let lib = mesa.join("usr/lib/aarch64-linux-gnu");
    let share = mesa.join("usr/share");
    let pairs = [
        (lib.join("dri"), "/usr/lib/aarch64-linux-gnu/dri"),
        (
            lib.join("libgallium-26.2.0-devel.so"),
            "/usr/lib/aarch64-linux-gnu/libgallium-26.2.0-devel.so",
        ),
        (
            lib.join("libvulkan_freedreno.so"),
            "/usr/lib/aarch64-linux-gnu/libvulkan_freedreno.so",
        ),
        (
            lib.join("libEGL_mesa.so.0.0.0"),
            "/usr/lib/aarch64-linux-gnu/libEGL_mesa.so.0.0.0",
        ),
        (
            lib.join("libgbm.so.1.0.0"),
            "/usr/lib/aarch64-linux-gnu/libgbm.so.1.0.0",
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
