//! Project Anland GPU renderer (Android-only).
//!
//! Zero-copy Plasma presentation path:
//!
//! ```text
//! Plasma apps -> KWin OpenGL -> Mesa freedreno/KGSL -> Android dma-buf
//!   -> GPU sync-file fence -> queueBuffer -> SurfaceFlinger
//! ```
//!
//! Selection is explicit and durable: `<APP_FILES>/renderer-mode` containing
//! `anland` selects this path. Historical QPainter/Smithay values are still
//! parsed during migration but never reactivate the retired graphics stack.
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

/// Renderer selection. `Smithay` remains as an internal legacy enum value so
/// old serialized state can be parsed, but it is no longer selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererKind {
    Smithay,
    Anland,
}

/// App-private flag file selecting the renderer (content `anland`).
pub fn mode_flag_path() -> std::path::PathBuf {
    std::path::Path::new(crate::core::config::APP_FILES_ROOT)
        .join(crate::core::renderer_policy::RENDERER_MODE_FILE)
}

/// Resolve the active renderer. Every state selects Anland; installation setup
/// makes that choice durable before the completion marker is committed.
pub fn active_renderer() -> RendererKind {
    let artifact = crate::core::provisioning::RuntimeArtifact::production();
    let runtime = artifact.classify_runtime(std::path::Path::new(
        crate::core::config::PRODUCTION_FS_ROOT,
    ));
    let kind = match crate::core::renderer_policy::resolve_renderer_mode(&mode_flag_path(), runtime)
    {
        Ok(crate::core::renderer_policy::RendererSelection::Anland) => RendererKind::Anland,
        Ok(crate::core::renderer_policy::RendererSelection::QPainter) => RendererKind::Anland,
        Err(error) => {
            log::error!("renderer-mode could not be read (using Anland): {error:#}");
            RendererKind::Anland
        }
    };
    match kind {
        RendererKind::Anland => {
            log::info!("anland.renderer=anland-gpu selected (durable renderer-mode policy)");
        }
        RendererKind::Smithay => unreachable!("legacy renderer is not selected"),
    }
    kind
}

/// Initialize or validate the durable Anland renderer selection used by setup
/// and committed-install handoff.
pub fn ensure_renderer_mode() -> anyhow::Result<RendererKind> {
    let artifact = crate::core::provisioning::RuntimeArtifact::production();
    let runtime = artifact.classify_runtime(std::path::Path::new(
        crate::core::config::PRODUCTION_FS_ROOT,
    ));
    let selection = crate::core::renderer_policy::ensure_renderer_mode(&mode_flag_path(), runtime)?;
    Ok(match selection {
        crate::core::renderer_policy::RendererSelection::Anland => RendererKind::Anland,
        crate::core::renderer_policy::RendererSelection::QPainter => RendererKind::Anland,
    })
}

/// Explicit migration/repair action: persist Anland for an existing install.
pub fn force_anland_renderer() -> anyhow::Result<RendererKind> {
    let selection = crate::core::renderer_policy::set_renderer_mode(
        &mode_flag_path(),
        crate::core::renderer_policy::RendererSelection::Anland,
    )?;
    anyhow::ensure!(
        selection == crate::core::renderer_policy::RendererSelection::Anland,
        "explicit Anland renderer selection did not persist"
    );
    Ok(RendererKind::Anland)
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

/// Guest-wide client environment for the Anland session.
///
/// The Mesa/KGSL overrides are exported by the provisioned KWin wrapper at
/// the compositor boundary, not here. `EGL_PLATFORM` is deliberately NOT set
/// (it forces QtQuick software fallback with OffscreenQuickView texture
/// failures).
///
/// The active hardware path needs the Forky Anland KWin assets and the
/// verified `mesa-kgsl-layer` overlay (the KGSL winsys is not supplied by
/// stock Debian Mesa). KWin uses KGSL-backed surfaceless EGL while Anland
/// owns Android dmabuf presentation; the retired QPainter path is not
/// selected.
/// - `MOZ_ENABLE_WAYLAND=1`: Firefox uses its normal native Wayland backend.
///
/// The old lfdevs XWayland-specific force flag is intentionally absent. The
/// active Forky stock XWayland package has not yet produced a valid KGSL
/// glamor proof on Pad 3, so exporting that variable globally would only hide
/// the unresolved P0 XWayland path behind an ignored environment knob.
pub fn guest_mesa_env() -> Vec<(String, String)> {
    vec![
        // Firefox is a normal native Wayland client. XWayland remains
        // installed and available to other applications, but Portal no
        // longer forces the browser through that compatibility path.
        ("MOZ_ENABLE_WAYLAND".into(), "1".into()),
        // GTK input method: IBus. The Portal IBus engine bridges X11/GTK
        // editable focus to the Android IME (real FocusIn/FocusOut, commit,
        // delete, enter). Qt/Wayland clients are unaffected (QT_IM_MODULE
        // unset keeps native Wayland text-input).
        ("GTK_IM_MODULE".into(), "ibus".into()),
    ]
}

/// Validate the host-side Anland launch contract without probing or starting
/// a guest session. The actual Mesa bytes are checked by `mesa_layer`; this
/// function makes sure the environment and bind targets that `launch()` will
/// use still describe the accelerated Anland/Wayland path.
pub fn validate_launch_contract() -> anyhow::Result<()> {
    let environment = guest_mesa_env();
    for (name, value) in [
        ("MOZ_ENABLE_WAYLAND", "1"),
        ("GTK_IM_MODULE", "ibus"),
    ] {
        anyhow::ensure!(
            environment.iter().any(|(actual_name, actual_value)| {
                actual_name == name && actual_value == value
            }),
            "Anland launch environment is missing {name}={value}"
        );
    }

    let binds = session_binds();
    for guest_path in [
        "/usr/lib/aarch64-linux-gnu/dri",
        "/usr/lib/aarch64-linux-gnu/gbm",
        "/usr/lib/aarch64-linux-gnu/libgallium-26.3.0-devel.so",
        "/usr/lib/aarch64-linux-gnu/libvulkan_freedreno.so",
        "/usr/lib/aarch64-linux-gnu/libEGL_mesa.so.0.0.0",
        "/usr/lib/aarch64-linux-gnu/libEGL_mesa.so.0",
        "/usr/lib/aarch64-linux-gnu/libGLX_mesa.so.0.0.0",
        "/usr/lib/aarch64-linux-gnu/libGLX_mesa.so.0",
        "/usr/lib/aarch64-linux-gnu/libgbm.so.1.0.0",
        "/usr/lib/aarch64-linux-gnu/libgbm.so.1",
        "/usr/share/vulkan/icd.d",
        "/usr/share/drirc.d",
    ] {
        anyhow::ensure!(
            binds
                .iter()
                .any(|bind| bind.guest_path == std::path::Path::new(guest_path)),
            "Anland Mesa bind is missing for {guest_path}"
        );
    }
    Ok(())
}

/// Guest flag selecting the explicit software-GL troubleshooting mode (`sw`)
/// instead of the default hardware-accelerated path. The wrapper and the
/// Android consumer read the same marker, so readiness/watchdog policy cannot
/// silently disagree with the renderer actually launched.
pub fn kwin_glmode_flag_path() -> std::path::PathBuf {
    std::path::Path::new(crate::core::config::PRODUCTION_FS_ROOT)
        .join("var/lib/localdesktop/kwin-glmode")
}

/// True only when the explicit `sw` troubleshooting marker is present.
/// Hardware remains the default and is the only release mode; software mode
/// labels CPU-synchronous frames honestly and is intentionally visible in the
/// KWin log and readiness evidence.
pub fn software_gl_fallback_requested() -> bool {
    std::fs::read_to_string(kwin_glmode_flag_path())
        .map(|value| value.trim() == "sw")
        .unwrap_or(false)
}

/// Bind mounts for the Anland guest session: broker socket dir + Mesa overlay.
pub fn session_binds() -> Vec<crate::core::runtime::BindMount> {
    use crate::core::runtime::BindMount;
    let files = std::path::Path::new(crate::core::config::APP_FILES_ROOT);
    let mesa = files.join("mesa-kgsl-layer");
    let mut binds = vec![BindMount::new(files.join("anland"), "/tmp/anland")];
    // Mesa overlay: the verified KGSL winsys over stock Debian paths. Each
    // entry is skipped when the layer file is absent; setup then fails closed
    // instead of silently selecting another graphics backend.
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
        RawWindowHandle::AndroidNdk(h) => {
            // Lifecycle evidence (spike/game-activity-host): the GameActivity
            // SurfaceView ANativeWindow backing this Winit window. Anland
            // acquires and owns it for this surface generation; suspend
            // joins surface workers and releases it before Android destroys
            // the surface while the persistent broker/session remains alive.
            let ptr = h.a_native_window.as_ptr();
            log::info!("Anland create_window: AndroidNdk a_native_window={ptr:p}");
            Ok((window, ptr))
        }
        other => Err(format!("Anland requires AndroidNdk window, got {other:?}")),
    }
}
