use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use super::build::{PolarBearApp, PolarBearBackend};
use crate::android::{
    accessibility::{self, AppUserEvent},
    backend::{
        pipewire_standalone_aaudio,
        wayland::{
            bind, centralize, centralize_injected_keyboard, handle, write_guest_output_state,
            CentralizedEvent, State,
        },
        webview::{ErrorVariant, WebviewAction, WebviewBackend},
    },
    ime,
    proot::launch::{is_running, launch, stop, take_failure},
    utils::{
        ndk::{self, run_in_jvm},
        webview::{runtime_error_page_url, setup_page_url, show_webview_popup},
        webview_handoff,
    },
};
use crate::core::android_input::committed_ascii_to_key_events;
use crate::core::config;
use crate::core::runtime::LinuxRuntime;
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::utils::Transform;
use smithay::wayland::shell::xdg::ToplevelSurface;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::WindowId;

fn configure_output(backend: &mut crate::android::backend::wayland::WaylandBackend) {
    let Some(winit) = backend.graphic_renderer.as_ref() else {
        return;
    };

    let window_size = winit.window_size();
    // Zero/invalid means the Android surface is temporarily unavailable:
    // preserve last valid state and resume on the next valid size. Never turn
    // 0 into fake 1px desktop geometry.
    if !crate::core::presentation::is_valid_host_size(window_size.w, window_size.h) {
        log::warn!(
            "configure_output: ignoring invalid Android surface {}x{}; preserving last valid {:?}",
            window_size.w,
            window_size.h,
            backend
                .compositor
                .state
                .authoritative_display_state
                .physical_size,
        );
        return;
    }
    let size = (window_size.w, window_size.h);
    let density_dpi = ndk::density_dpi(&backend.android_app).max(1);
    // Not `winit.scale_factor()`: that reads `AConfiguration`, which still reports the 160 dpi
    // default on the first launch and only becomes accurate after a configuration change.
    let guest_scale_factor = ndk::scale_factor(&backend.android_app);
    backend.guest_scale_factor = guest_scale_factor;
    // Report Android's effective app refresh, independently of the requested maximum.
    let observed = ndk::refresh_rate_millihz(&backend.android_app);
    if crate::core::android_integration::is_valid_refresh_millihz(observed) {
        backend.physical_refresh_millihz = observed;
        let state = &mut backend.compositor.state.authoritative_display_state;
        state.note_physical_refresh_millihz(observed);
    }
    backend
        .compositor
        .state
        .authoritative_display_state
        .refresh_rate_millihz = backend.refresh_rate_millihz;
    backend.compositor.state.size = size.into();

    // Mutate the existing authoritative state in place so resize generations,
    // requested configures, committed guest geometry and Plasma scale survive
    // suspend/resume. A fresh `new()` here would reset generation to 0 and
    // discard transitional guest state.
    let host_changed = {
        let state = &mut backend.compositor.state.authoritative_display_state;
        // Density is panel metadata, never rewrites Plasma scale/guest.
        state.update_density_dpi(density_dpi);
        // Host target: bumps generation only on real change, coalesces repeats.
        let changed = state
            .try_update_physical_size(window_size.w, window_size.h)
            .is_some();
        // Explicit display-configuration refresh (not the render path):
        // mtime-cached, parses JSON only when kwinoutputconfig.json changed.
        if crate::android::backend::wayland::output_state::sync_kwin_output_scale(state) {
            crate::android::backend::wayland::log_presentation_state(
                "plasma-scale",
                &backend.compositor.state,
            );
        }
        changed
    };
    let display_state = backend.compositor.state.authoritative_display_state;
    backend.compositor.state.coordinate_transform = display_state.coordinate_transform();
    let uniform = display_state.uniform_presentation_scale();
    backend.compositor.state.kwin_surface_scale = (uniform, uniform);
    {
        let snapshot = backend
            .compositor
            .state
            .authoritative_display_state
            .presentation_snapshot();
        backend.button_tracker.reevaluate(&snapshot);
    }
    if host_changed {
        crate::android::backend::wayland::log_presentation_state(
            "host-resize",
            &backend.compositor.state,
        );
    }

    let physical_size_mm = display_state.physical_size_mm();

    let output = backend
        .compositor
        .output
        .get_or_insert_with(|| {
            Output::new(
                "Portal Wayland Compositor".into(),
                PhysicalProperties {
                    size: physical_size_mm.into(),
                    subpixel: Subpixel::HorizontalRgb,
                    make: "Portal".into(),
                    model: config::VERSION.into(),
                },
            )
        })
        .clone();

    backend.compositor.state.output = Some(output.clone());

    if backend.compositor.output_global.is_none() {
        let dh = backend.compositor.display.handle();
        backend.compositor.output_global = Some(output.create_global::<State>(&dh));
    }

    // Stable nominal mode: VRR physical changes never reach this `Mode`.
    let mode = Mode {
        size: size.into(),
        refresh: backend.refresh_rate_millihz,
    };
    output.set_preferred(mode);
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );
    let guest_scale = display_state.baseline_density_scale().round().max(1.0) as i32;
    write_guest_output_state(window_size.w, window_size.h, guest_scale);

    let configure_size = display_state.configure_size();
    for surface in backend.compositor.state.xdg_shell_state.toplevel_surfaces() {
        output.enter(surface.wl_surface());
        if let Some(serial) = configure_toplevel(surface, configure_size.0, configure_size.1) {
            backend
                .compositor
                .state
                .authoritative_display_state
                .note_configure_sent(serial);
        }
    }
}

/// Bind the current Android surface and start the native Wayland runtime. This is shared by the
/// normal resume path and the event-triggered setup handoff, so a completed setup never needs to
/// recreate the NativeActivity or sleep for an arbitrary amount of time.
fn resume_wayland(
    backend: &mut crate::android::backend::wayland::WaylandBackend,
    event_loop: &ActiveEventLoop,
    android_app: &winit::platform::android::activity::AndroidApp,
) -> bool {
    // Project Anland: explicit renderer selection. The GPU path owns the
    // window via dequeue/queue and never creates a Smithay EGL surface.
    if crate::android::anland::is_anland_requested() {
        return resume_anland(backend, event_loop, android_app);
    }
    if backend.graphic_renderer.is_none() {
        match bind(event_loop) {
            Ok(winit) => {
                backend.graphic_renderer = Some(winit);
                backend.output_damage_tracker = None;
                backend.output_damage_signature = None;
                backend.frame_timeline = None;
                backend.frame_timeline_stats = Default::default();
            }
            Err(error) => {
                log::error!("Failed to initialize Wayland renderer on resume: {error}");
                accessibility::set_runtime_active(false);
                event_loop.set_control_flow(ControlFlow::Wait);
                return false;
            }
        }
    } else {
        log::info!("Ignoring redundant resume while renderer is already active");
    }

    if backend.socket_watcher.is_none() {
        let (listener_fd, display_fd) = backend.compositor.socket_fds();
        if let Some(proxy) = accessibility::event_loop_proxy() {
            match crate::android::backend::wayland::WaylandSocketWatcher::spawn(
                listener_fd,
                display_fd,
                proxy,
            ) {
                Ok(watcher) => backend.socket_watcher = Some(watcher),
                Err(error) => log::error!("Failed to spawn WaylandSocketWatcher: {error}"),
            }
        }
    }

    if backend.surface_control_cursor.is_none() {
        if let Some(winit) = backend.graphic_renderer.as_ref() {
            match crate::android::backend::wayland::surface_control_cursor::SurfaceControlCursor::new(
                winit.native_window_ptr(),
            ) {
                Ok(cursor) => {
                    log::info!("presenter.cursor=surface-control release_fences=api36");
                    backend.surface_control_cursor = Some(cursor);
                }
                Err(error) => log::info!("presenter.cursor=egl reason={error}"),
            }
        }
    }

    backend.output_dirty = true;
    configure_output(backend);
    // High-refresh-rate hint is sticky per ANativeWindow: issue once per
    // window creation (resume), not per-frame. Uses only the supported NDK
    // `ANativeWindow_setFrameRate[WithChangeStrategy]` path (preferred rate
    // from the supported modes, DEFAULT, seamless) — no global settings, no
    // root, no OEM hacks. Re-issued after every suspend because suspend
    // destroys the native window; redundant resumes on the same window skip
    // the re-request. The hint rate matches the nominal `wl_output` target
    // resolved in `configure_output` so KWin, the output mode, and Android
    // all agree.
    if !backend.frame_rate_requested {
        let rate_hz = ndk::preferred_high_refresh_millihz(&backend.android_app) as f32 / 1000.0;
        crate::android::utils::frame_rate::ensure_high_refresh_rate_hz(
            &backend.android_app,
            rate_hz,
        );
        backend.frame_rate_requested = true;
        // On-device mode evidence for OxygenOS policy verification (dumpsys).
        ndk::log_display_modes(&backend.android_app);
    }
    // Prime the change detector so the first throttled poll compares against
    // the fresh resume snapshot instead of a stale pre-suspend rate.
    backend.last_refresh_poll_ms = Some(backend.clock.now().as_millis() as u64);
    accessibility::set_runtime_active(true);

    if let Some(winit) = backend.graphic_renderer.as_ref() {
        winit.window().request_redraw();
    }
    handle(CentralizedEvent::Redraw, backend, event_loop);
    if backend.graphic_renderer.is_none() {
        log::error!("Initial Wayland frame failed; guest session will not be launched");
        return false;
    }
    pipewire_standalone_aaudio::spawn_after_ready(android_app.clone());
    launch();
    // Spawn guest/audio workers first so they retain normal priority. Only
    // the NativeActivity event/render thread receives Android display
    // priority; no affinity, root capability or guest-wide nice change.
    crate::android::utils::frame_pacing::prioritize_current_render_thread();
    true
}

/// Project Anland resume: take over the window for zero-copy GPU
/// presentation instead of binding the Smithay EGL renderer. The guest
/// session launched below picks up the Anland broker socket, Mesa overlay
/// and KWin environment from `launch()`; KWin selects its Anland backend via
/// `$ANLAND_SOCKET` and renders with OpenGL/freedreno into our buffers.
fn resume_anland(
    backend: &mut crate::android::backend::wayland::WaylandBackend,
    event_loop: &ActiveEventLoop,
    android_app: &winit::platform::android::activity::AndroidApp,
) -> bool {
    if backend.anland.is_some() {
        log::info!("Ignoring redundant Anland resume while GPU session is active");
        return true;
    }
    if backend.graphic_renderer.is_some() {
        log::warn!(
            "anland.session Smithay renderer unexpectedly active; dropping it for the GPU takeover"
        );
        backend.graphic_renderer = None;
    }
    let (window, raw) = match crate::android::anland::create_window(event_loop) {
        Ok(pair) => pair,
        Err(error) => {
            log::error!("anland.session window creation failed: {error}");
            accessibility::set_runtime_active(false);
            event_loop.set_control_flow(ControlFlow::Wait);
            return false;
        }
    };
    let size = window.inner_size();
    if size.width == 0 || size.height == 0 {
        log::error!(
            "anland.session invalid Android surface {}x{}; refusing GPU takeover",
            size.width,
            size.height
        );
        accessibility::set_runtime_active(false);
        event_loop.set_control_flow(ControlFlow::Wait);
        return false;
    }
    // Same sticky per-window high-refresh hint as the Smithay path.
    if !backend.frame_rate_requested {
        let rate_hz = ndk::preferred_high_refresh_millihz(android_app) as f32 / 1000.0;
        crate::android::utils::frame_rate::ensure_high_refresh_rate_hz(android_app, rate_hz);
        backend.frame_rate_requested = true;
        ndk::log_display_modes(android_app);
    }
    backend.last_refresh_poll_ms = Some(backend.clock.now().as_millis() as u64);
    let refresh_mhz = ndk::preferred_high_refresh_millihz(android_app).max(1) as u32;
    let config = crate::android::anland::AnlandConfig {
        width: size.width,
        height: size.height,
        refresh_mhz,
        socket_path: crate::android::anland::host_socket_path(),
    };
    match crate::android::anland::AnlandSession::start(raw, window, &config) {
        Ok(session) => {
            log::info!(
                "anland.session=active compositor=kwin-opengl(expected) driver=freedreno(expected) window={}x{} refresh_mhz={}",
                size.width,
                size.height,
                refresh_mhz
            );
            backend.anland = Some(session);
        }
        Err(error) => {
            log::error!("anland.session=start failed (stable QPainter path untouched): {error}");
            accessibility::set_runtime_active(false);
            event_loop.set_control_flow(ControlFlow::Wait);
            return false;
        }
    }
    accessibility::set_runtime_active(true);
    pipewire_standalone_aaudio::spawn_after_ready(android_app.clone());
    launch();
    crate::android::utils::frame_pacing::prioritize_current_render_thread();
    true
}

/// Forward raw window input to the Anland producer as fixed-size input
/// events. Pointer motion carries session-synthesized relative deltas (the
/// KWin backend emits both absolute and relative motion per event; zeroed
/// deltas froze the touchpad cursor). Both CursorMoved and AndroidPointerMoved
/// (touchpad/mouse hover with raw device identity) feed the same path.
///
/// Motion is hot (60Hz+): logged at info at most once per second so physical
/// tests stay observable in logcat without flooding it.
static LAST_MOTION_LOG_NS: AtomicU64 = AtomicU64::new(0);

fn note_motion_logged(x: f64, y: f64, detail: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    if now.wrapping_sub(LAST_MOTION_LOG_NS.load(Ordering::Relaxed)) >= 1_000_000_000 {
        LAST_MOTION_LOG_NS.store(now, Ordering::Relaxed);
        log::info!("anland.input motion x={x:.1} y={y:.1} ({detail})");
    }
}
fn forward_anland_input(
    backend: &crate::android::backend::wayland::WaylandBackend,
    event: &WindowEvent,
) {
    use crate::android::anland::protocol::InputEvent as AnlandInput;
    let Some(session) = backend.anland.as_ref() else {
        return;
    };
    match event {
        WindowEvent::Touch(touch) => {
            let action = match touch.phase {
                TouchPhase::Started => crate::android::anland::protocol::INPUT_ACTION_DOWN,
                TouchPhase::Moved => crate::android::anland::protocol::INPUT_ACTION_MOVE,
                TouchPhase::Ended | TouchPhase::Cancelled => {
                    crate::android::anland::protocol::INPUT_ACTION_UP
                }
            };
            note_motion_logged(
                touch.location.x,
                touch.location.y,
                &format!("touch action={action} id={}", touch.id),
            );
            session.send_input(&AnlandInput::touch(
                action,
                touch.location.x as f32,
                touch.location.y as f32,
                touch.id as i32,
            ));
            session.send_input(&AnlandInput::touch_frame());
        }
        WindowEvent::CursorMoved { position, .. } => {
            note_motion_logged(position.x, position.y, "cursor");
            session.send_pointer_motion(position.x as f32, position.y as f32);
        }
        WindowEvent::AndroidPointerMoved {
            position,
            android_device_id,
            source,
            tool_type,
            ..
        } => {
            log::debug!(
                "anland.input pointer dev={android_device_id} src={source:#x} tool={tool_type}"
            );
            note_motion_logged(position.x, position.y, "android-pointer");
            session.send_pointer_motion(position.x as f32, position.y as f32);
        }
        WindowEvent::MouseInput { state, button, .. } => {
            let code = match button {
                MouseButton::Left => 0x110,
                MouseButton::Right => 0x111,
                MouseButton::Middle => 0x112,
                MouseButton::Back => 0x116,
                MouseButton::Forward => 0x115,
                MouseButton::Other(b) => 0x110 + (*b as u32),
            };
            let pressed = *state == ElementState::Pressed;
            log::info!("anland.input button code={code:#x} pressed={pressed}");
            session.send_input(&AnlandInput::pointer_button(code, pressed));
        }
        WindowEvent::MouseWheel {
            delta, phase, ..
        } => {
            // Scroll-stop terminates live finger streams so KWin emits
            // axis-stop and kinetic scrolling settles (phase arrives with
            // zero deltas from the touchpad state machine).
            let finished = matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled);
            match delta {
                MouseScrollDelta::LineDelta(x, y) => {
                    log::debug!("anland.input wheel lines x={x} y={y}");
                    if *y != 0.0 {
                        session.send_input(&AnlandInput::pointer_axis(0, *y, *y as i32));
                    }
                    if *x != 0.0 {
                        session.send_input(&AnlandInput::pointer_axis(1, *x, *x as i32));
                    }
                }
                MouseScrollDelta::PixelDelta(pos) => {
                    // Physical touchpad smooth scroll: finger-source events.
                    // The rebuilt backend emits these with Finger source
                    // (kinetic) and applies the Portal Touchpad kcminputrc
                    // settings; the host sends raw buffer-px deltas and must
                    // not scale or invert (no double application).
                    log::debug!(
                        "anland.input finger scroll x={:.1} y={:.1}",
                        pos.x,
                        pos.y
                    );
                    if pos.y != 0.0 {
                        session.send_finger_axis(0, pos.y as f32);
                    }
                    if pos.x != 0.0 {
                        session.send_finger_axis(1, pos.x as f32);
                    }
                }
            }
            if finished {
                session.send_finger_stops();
            }
        }
        WindowEvent::KeyboardInput { event, .. } => {
            if event.state == ElementState::Pressed && event.repeat {
                // Compositor-side autorepeat owns repeats; forward the press.
            }
            let Some(scancode) = crate::android::backend::wayland::keymap::physicalkey_to_scancode(
                event.physical_key,
            ) else {
                return;
            };
            let action = match event.state {
                ElementState::Pressed => crate::android::anland::protocol::INPUT_ACTION_DOWN,
                ElementState::Released => crate::android::anland::protocol::INPUT_ACTION_UP,
            };
            log::info!(
                "anland.input key action={action} scancode={scancode}"
            );
            session.send_input(&AnlandInput::key(action, scancode as i32));
        }
        _ => {}
    }
}

fn configure_toplevel(surface: &ToplevelSurface, width: i32, height: i32) -> Option<u32> {
    surface.with_pending_state(|state| {
        state.size.replace((width, height).into());
        state.states.set(xdg_toplevel::State::Activated);
        state.states.set(xdg_toplevel::State::Fullscreen);
        state.states.set(xdg_toplevel::State::Maximized);
    });
    // Capture the serial so later KWin commits attribute to this request.
    Some(u32::from(surface.send_configure()))
}

impl PolarBearApp {
    /// Open the current provisioning/recovery page in the existing Activity.
    ///
    /// Keeping this in one helper matters for runtime failures: replacing the Wayland backend
    /// must immediately surface the actionable page instead of waiting for Android to emit a
    /// second `resumed` callback.
    fn show_webview(&mut self) {
        let PolarBearBackend::WebView(backend) = &mut self.backend else {
            return;
        };
        accessibility::set_runtime_active(false);
        backend.attach_android_app(self.frontend.android_app.clone());
        let token = backend.auth_token();
        let url = match &backend.error {
            ErrorVariant::None => setup_page_url(backend.socket_port, &token),
            ErrorVariant::Unsupported => runtime_error_page_url(
                backend.socket_port,
                &token,
                "This device cannot run the bundled ARM64 Linux guest.",
            ),
            ErrorVariant::Runtime(reason) => {
                runtime_error_page_url(backend.socket_port, &token, reason)
            }
        };
        // A configuration change can produce multiple resumed callbacks while the old popup is
        // still alive. Reusing that popup keeps all Java calls on one Looper and avoids a second
        // WebView covering the actual desktop.
        if webview_handoff::is_open() {
            return;
        }
        let android_app = self.frontend.android_app.clone();
        thread::spawn(move || {
            run_in_jvm(
                move |env, app| {
                    show_webview_popup(env, app, &url);
                },
                android_app,
            );
        });
    }

    /// Replace a failed Wayland session with a local, graphical recovery page.
    ///
    /// This is deliberately an in-process backend swap. The NativeActivity and its current
    /// configuration remain alive, so Android does not briefly expose a blank native surface or
    /// require a fixed-delay activity recreation.
    fn enter_runtime_error(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        let android_app = self.frontend.android_app.clone();
        // Reap the tracked PRoot/session worker before dropping the compositor. Otherwise its
        // launch guard can keep the next Retry Plasma request from starting a new session.
        stop();
        if let PolarBearBackend::Wayland(backend) = &mut self.backend {
            if let Err(error) = ime::hide(&backend.android_app) {
                log::debug!("Software keyboard bridge could not be hidden: {error}");
            }
            backend.graphic_renderer = None;
            // Project Anland: the error screen needs the window back.
            if let Some(session) = backend.anland.take() {
                session.stop();
            }
        }
        accessibility::set_runtime_active(false);
        ime::reset();
        pipewire_standalone_aaudio::shutdown();
        webview_handoff::clear();
        log::error!("Switching to graphical runtime error screen: {reason}");
        self.backend =
            PolarBearBackend::WebView(WebviewBackend::runtime_error(android_app, reason));
        self.show_webview();
    }

    /// Handle an action received by the runtime error page without blocking the winit loop.
    fn handle_webview_actions(&mut self, event_loop: &ActiveEventLoop) {
        let retry_requested = match &self.backend {
            PolarBearBackend::WebView(backend)
                if matches!(&backend.error, ErrorVariant::Runtime(_)) =>
            {
                backend.take_action(WebviewAction::RetryPlasma)
            }
            PolarBearBackend::Wayland(_) => false,
            PolarBearBackend::WebView(_) => false,
        };
        if !retry_requested {
            return;
        }

        // The action arrives on the WebSocket reader while its PopupWindow is still visible. Ask
        // that Looper to exit first and complete the backend swap from its follow-up wake event.
        self.pending_runtime_retry = true;
        if !webview_handoff::request_close(self.frontend.android_app.clone()) {
            self.finish_runtime_retry(event_loop);
        }
    }

    /// Finish a Retry Plasma request after the old WebView has dismissed itself.
    fn finish_runtime_retry(&mut self, event_loop: &ActiveEventLoop) {
        if !self.pending_runtime_retry || webview_handoff::is_open() || is_running() {
            if self.pending_runtime_retry && is_running() {
                log::debug!("Waiting for the cancelled guest session before rebuilding Plasma");
            }
            return;
        }
        self.pending_runtime_retry = false;

        let android_app = self.frontend.android_app.clone();
        // This is normally already stopped by `enter_runtime_error`; keeping the operation
        // idempotent covers errors raised by a guest process just before the overlay appeared.
        stop();
        log::info!("Retry Plasma action accepted; rebuilding the Wayland backend");
        let backend = crate::android::proot::setup::setup(android_app.clone());
        self.backend = backend;
        let rebuilt_to_webview = matches!(&self.backend, PolarBearBackend::WebView(_));
        if let PolarBearBackend::WebView(backend) = &mut self.backend {
            backend.attach_android_app(android_app);
            log::error!("Plasma retry could not rebuild the guest backend; keeping the error page");
        }
        if rebuilt_to_webview {
            self.show_webview();
            return;
        }
        let resume_failed = if let PolarBearBackend::Wayland(backend) = &mut self.backend {
            !resume_wayland(backend, event_loop, &self.frontend.android_app)
        } else {
            false
        };
        if resume_failed {
            self.enter_runtime_error("Wayland could not be resumed after Retry Plasma");
        }
    }

    /// Transition from completed provisioning WebView to Wayland backend in-process.
    /// Returns true if a transition occurred.
    fn handle_setup_complete(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if !webview_handoff::take_setup_complete() {
            return false;
        }
        // The final setup callback has already closed the popup. Re-run the idempotent
        // dispatcher synchronously on this event-loop turn; all stages now report
        // complete, so this constructs the Wayland backend in the current Activity.
        let android_app = self.frontend.android_app.clone();
        let mut backend = crate::android::proot::setup::setup(android_app.clone());
        if let PolarBearBackend::WebView(webview) = &mut backend {
            webview.attach_android_app(android_app);
            log::error!(
                "Setup completion event arrived before the guest was ready; retaining the WebView error screen"
            );
        }
        self.backend = backend;
        if let PolarBearBackend::Wayland(backend) = &mut self.backend {
            let _ = resume_wayland(backend, event_loop, &self.frontend.android_app);
            crate::android::tablet_mode_manager::apply_kwin_tablet_mode(
                ime::is_desktop_input_present(),
            );
        }
        true
    }
}

impl ApplicationHandler<AppUserEvent> for PolarBearApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(reason) = take_failure() {
            if matches!(&self.backend, PolarBearBackend::Wayland(_)) {
                self.enter_runtime_error(reason);
                return;
            }
        }
        if matches!(&self.backend, PolarBearBackend::WebView(_)) {
            if self.handle_setup_complete(event_loop) {
                return;
            }
            self.show_webview();
            return;
        }

        let resume_failed = if let PolarBearBackend::Wayland(backend) = &mut self.backend {
            ime::reset();
            let runtime = crate::android::runtime::proot::PRootRuntime::active();
            crate::android::proot::setup::sync_guest_network_config(runtime.rootfs_path());
            let failed = !resume_wayland(backend, event_loop, &self.frontend.android_app);
            if !failed {
                ime::refresh_visibility();
                crate::android::tablet_mode_manager::apply_kwin_tablet_mode(
                    ime::is_desktop_input_present(),
                );
            }
            failed
        } else {
            false
        };
        if resume_failed {
            self.enter_runtime_error("Wayland could not be initialized after the Activity resumed");
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: AppUserEvent) {
        if let Some(reason) = take_failure() {
            if matches!(&self.backend, PolarBearBackend::Wayland(_)) {
                self.enter_runtime_error(reason);
                return;
            }
        }
        if matches!(&self.backend, PolarBearBackend::WebView(_)) {
            accessibility::drain_pending_events();
            // Setup owns RetrySetup while provisioning is active. Runtime error pages are
            // handled below for RetryPlasma; draining IME commits here prevents a stale editor
            // from leaking text into a future Wayland session.
            ime::reset();
            if self.pending_runtime_retry {
                self.finish_runtime_retry(event_loop);
                return;
            }
            self.handle_webview_actions(event_loop);
            if self.pending_runtime_retry {
                return;
            }
            // Retry Plasma may have replaced the error page with a live Wayland backend. Do not
            // run setup-completion handling against that newly installed backend.
            if !matches!(&self.backend, PolarBearBackend::WebView(_)) {
                return;
            }
            if self.handle_setup_complete(event_loop) {
                return;
            }
            return;
        }

        let PolarBearBackend::Wayland(backend) = &mut self.backend else {
            return;
        };

        if let Some(show) = ime::take_visibility_request() {
            log::info!("Portal event loop: applying ime visibility request show={show}");
            let result = if show {
                ime::show(&backend.android_app)
            } else {
                ime::hide(&backend.android_app)
            };
            if let Err(error) = result {
                log::warn!("Could not update Android software-keyboard visibility: {error}");
            } else {
                log::info!("Portal event loop: ime visibility updated successfully show={show}");
            }
        }

        for text in ime::drain_commits() {
            if backend.anland.is_some() {
                inject_committed_text_anland(&text, backend);
            } else if !backend.compositor.state.commit_android_text(&text) {
                inject_committed_text(&text, backend, event_loop);
            }
        }

        for event in accessibility::drain_pending_events() {
            let event = centralize_injected_keyboard(
                event.scancode,
                event.state,
                event.event_time_ms,
                backend,
            );
            handle(event, backend, event_loop);
        }

        if let AppUserEvent::ChoreographerFrame {
            frame_time_ns,
            deadline_ns,
            expected_present_ns,
            vsync_id,
        } = _event
        {
            log::debug!(
                "android.frame_callback time_ns={frame_time_ns} deadline_ns={deadline_ns} expected_present_ns={expected_present_ns} vsync_id={vsync_id}"
            );
            if backend.output_dirty {
                backend.frame_timeline =
                    Some(crate::android::utils::frame_pacing::AndroidFrameTimeline {
                        frame_time_ns,
                        deadline_ns,
                        expected_present_ns,
                        vsync_id,
                    });
                backend.frame_timeline_stats.note_callback();
                if let Some(winit) = backend.graphic_renderer.as_ref() {
                    winit.window().request_redraw();
                }
            }
        }

        if _event == AppUserEvent::AndroidClipboardChanged {
            // Dedicated wake for external clipboard changes. Apply the queued
            // update and flush Wayland immediately so a later Ctrl+V observes
            // the fresh selection. `process_android_clipboard` is non-blocking
            // (no Binder/FD work) and already flushes.
            log::info!("clipdiag wake received");
            backend.compositor.sync_kwin_seat_focus();
            let applied = backend.compositor.process_android_clipboard();
            log::info!("clipdiag wake applied={applied}");
        }

        if _event == AppUserEvent::WaylandTraffic {
            if let Ok(dirty) = crate::android::backend::wayland::dispatch_wayland(backend) {
                if dirty || backend.output_dirty {
                    backend.schedule_redraw();
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let mut runtime_failed = false;
        if let PolarBearBackend::Wayland(backend) = &mut self.backend {
            // Anland GPU mode owns no Smithay renderer; the session on
            // `backend.anland` is the renderer for liveness purposes.
            if backend.graphic_renderer.is_none() && backend.anland.is_none() {
                if matches!(event, WindowEvent::CloseRequested) {
                    event_loop.exit();
                } else {
                    log::info!(
                        "Ignoring window event while renderer is suspended: {:?}",
                        event
                    );
                }
                return;
            }

            // Project Anland: mirror raw input to the GPU producer. The
            // Smithay handlers below stay warm (state machines, gesture
            // tracking) but render nothing while their renderer is None.
            forward_anland_input(backend, &event);

            match &event {
                // Focus changes are common during rotation, popup dismissal and app switching.
                // Do not summon the soft keyboard for every Focused(true): Android should show it
                // only after winit enables text input (or an explicit text-entry request).
                WindowEvent::Focused(true) => {}
                WindowEvent::Focused(false) => {
                    if let Err(error) = ime::hide(&backend.android_app) {
                        log::debug!("Software keyboard bridge could not be hidden: {error}");
                    }
                    ime::reset();
                }
                WindowEvent::Ime(Ime::Enabled) => {
                    if let Err(error) = ime::show(&backend.android_app) {
                        log::debug!("Software keyboard bridge could not be shown: {error}");
                    }
                }
                WindowEvent::Ime(Ime::Commit(text)) => {
                    inject_committed_text(text, backend, event_loop);
                    return;
                }
                WindowEvent::Ime(Ime::Disabled) => {
                    if let Err(error) = ime::hide(&backend.android_app) {
                        log::debug!("Software keyboard bridge could not be hidden: {error}");
                    }
                }
                WindowEvent::Ime(Ime::Preedit(_, _)) => {}
                _ => {}
            }

            // Map raw events to our own events
            let event = centralize(event, backend);

            // Handle the centralized events
            handle(event, backend, event_loop);
            runtime_failed = backend.graphic_renderer.is_none() && backend.anland.is_none();
        }
        if runtime_failed {
            self.enter_runtime_error("Wayland lost its renderer while handling a window event");
        }
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        accessibility::set_runtime_active(false);
        ime::reset();
        event_loop.set_control_flow(ControlFlow::Wait);

        if let PolarBearBackend::Wayland(backend) = &mut self.backend {
            if let Err(error) = ime::hide(&backend.android_app) {
                log::debug!("Software keyboard bridge could not be hidden on suspend: {error}");
            }
            // Project Anland: stop the GPU session first, while the
            // ANativeWindow is still valid (bounded joins, then disconnect).
            if let Some(session) = backend.anland.take() {
                session.stop();
            }
            backend.socket_watcher = None;
            // Drop child layers while this lifecycle generation's parent
            // ANativeWindow is still valid.
            backend.surface_control_cursor = None;
            backend.graphic_renderer = None;
            backend.output_damage_tracker = None;
            backend.output_damage_signature = None;
            backend.suspend_input_and_presentation();
            backend.frame_timeline = None;
            backend.frame_timeline_stats = Default::default();
            // The ANativeWindow is destroyed on suspend; the preferred-rate hint
            // must be re-issued on the next resume's fresh window.
            backend.frame_rate_requested = false;
            backend.last_refresh_poll_ms = None;
            // Kill the standalone-client PipeWire/AAudio backend if it was started.
            pipewire_standalone_aaudio::shutdown();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let PolarBearBackend::Wayland(backend) = &mut self.backend {
            if let Ok(dirty) = crate::android::backend::wayland::dispatch_wayland(backend) {
                if dirty || backend.output_dirty {
                    backend.schedule_redraw();
                }
            }

            if backend.touch_mode == crate::android::backend::wayland::TouchMode::Undecided
                && backend.touch_points.len() == 1
            {
                if let Some(down_time) = backend.touch_down_time {
                    let now = backend.clock.now().as_millis() as u64;
                    let elapsed = now.saturating_sub(down_time);
                    if elapsed >= backend.long_press_timeout_ms {
                        handle(CentralizedEvent::Redraw, backend, event_loop);
                    } else {
                        let remaining = backend.long_press_timeout_ms - elapsed;
                        event_loop.set_control_flow(ControlFlow::WaitUntil(
                            std::time::Instant::now() + std::time::Duration::from_millis(remaining),
                        ));
                        return;
                    }
                }
            }

            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

/// Emit one evdev key edge on the Anland session (no Smithay involvement).
fn anland_key(session: &crate::android::anland::AnlandSession, scancode: u32, pressed: bool) {
    use crate::android::anland::protocol::InputEvent as AnlandInput;
    session.send_input(&AnlandInput::key(
        if pressed {
            crate::android::anland::protocol::INPUT_ACTION_DOWN
        } else {
            crate::android::anland::protocol::INPUT_ACTION_UP
        },
        scancode as i32,
    ));
}

/// Anland-mode routing for Android IME commits (QPainter path untouched).
///
/// Delivery follows the focused-client kind, because X11 clients never
/// activate KWin's input method (no XIM bridge) while Wayland text clients
/// do (bridge ACTIVATE, tracked by `is_ime_context_active`):
/// - backspace runs / enter: layout-independent evdev keys (14 / 28);
/// - Wayland text client focused: full-Unicode TEXT_INPUT to KWin's input
///   method (`commitText`), same destination as the bridge's commit_string;
/// - otherwise (X11/unknown): ASCII via evdev keys (existing mapping),
///   non-ASCII via Android clipboard + deferred Ctrl+V (the guest sync
///   daemon serves it into the Wayland/X selection; benign on failure —
///   text stays pasted-ready in the clipboard).
/// Preedit has no backend channel (KWin exposes only commitText) and stays
/// dropped; commits are never duplicated (exactly one branch runs).
fn inject_committed_text_anland(
    text: &str,
    backend: &mut crate::android::backend::wayland::WaylandBackend,
) {
    let Some(session) = backend.anland.as_ref() else {
        return;
    };
    if !text.is_empty() && text.chars().all(|c| c == '\x08') {
        let count = text.chars().count();
        log::info!("anland.ime {count} backspace(s) via evdev keys");
        for _ in text.chars() {
            anland_key(session, 14, true);
            anland_key(session, 14, false);
        }
        return;
    }
    if text == "\n" || text == "\r\n" || text == "\r" {
        log::info!("anland.ime enter via evdev key");
        anland_key(session, 28, true);
        anland_key(session, 28, false);
        return;
    }
    if crate::android::ime::is_ime_context_active() {
        if session.send_text(text) {
            return;
        }
        log::warn!("anland.ime send_text failed; falling back");
    }
    if text.is_empty() {
        return;
    }
    // Deliver to both input-method channels; each self-gates on real focus
    // (KWin IM context for Wayland text clients, IBus editable focus for
    // X11/GTK), so exactly one lands — and no focus-kind detection race can
    // lose text while ACTIVATE is still in flight. Both drops are silent
    // no-ops, so sending to both is safe. (Do NOT early-return on send
    // success: sent only means framed; delivery is decided focus-side.)
    let mut framed = session.send_text(text);
    framed |= crate::android::ime::send_engine_text(text);
    if framed {
        return;
    }
    // Neither channel could take it (no session, engine down): degraded
    // ASCII keys so plain Latin text still types somewhere sane.
    let events = committed_ascii_to_key_events(text);
    if events.is_empty() {
        log::warn!("anland.ime commit dropped (no channel, non-ascii)");
        return;
    }
    log::info!("anland.ime ascii commit via evdev keys (degraded)");
    for (scancode, shift_required) in events {
        let shift_edge = shift_required && !backend.pressed_keys.contains(&42);
        if shift_edge {
            anland_key(session, 42, true);
        }
        anland_key(session, scancode, true);
        anland_key(session, scancode, false);
        if shift_edge {
            anland_key(session, 42, false);
        }
    }
}

/// Forward Android IME commits through the same physical-key path as a hardware keyboard.
///
/// The Smithay compositor receives evdev key events, not arbitrary Unicode strings. The
/// host-testable policy intentionally handles printable ASCII plus editing/control keys; a
/// non-ASCII commit is logged and dropped (Anland mode routes those via TEXT_INPUT/paste).
fn inject_committed_text(
    text: &str,
    backend: &mut crate::android::backend::wayland::WaylandBackend,
    event_loop: &ActiveEventLoop,
) {
    let events = committed_ascii_to_key_events(text);
    if events.is_empty() && !text.is_empty() {
        log::debug!("Dropping software-keyboard commit with no supported ASCII keys");
        return;
    }

    for (scancode, shift_required) in events {
        // The IME-generated Shift shares `pressed_keys`/`key_counter` with the
        // physical keyboard. If the user already holds Shift, emitting another
        // press/release would remove their physical modifier on release. Only
        // emit the Shift edge when it is not already held.
        let shift_edge = shift_required && !backend.pressed_keys.contains(&42);
        let time = backend.clock.now().as_millis() as u64;
        if shift_edge {
            handle(
                centralize_injected_keyboard(42, ElementState::Pressed, time, backend),
                backend,
                event_loop,
            );
        }
        let time = backend.clock.now().as_millis() as u64;
        handle(
            centralize_injected_keyboard(scancode, ElementState::Pressed, time, backend),
            backend,
            event_loop,
        );
        let time = backend.clock.now().as_millis() as u64;
        handle(
            centralize_injected_keyboard(scancode, ElementState::Released, time, backend),
            backend,
            event_loop,
        );
        if shift_edge {
            let time = backend.clock.now().as_millis() as u64;
            handle(
                centralize_injected_keyboard(42, ElementState::Released, time, backend),
                backend,
                event_loop,
            );
        }
    }
}
