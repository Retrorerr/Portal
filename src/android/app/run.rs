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
use winit::event::{ElementState, Ime, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::WindowId;

fn configure_output(backend: &mut crate::android::backend::wayland::WaylandBackend) {
    let Some(winit) = backend.graphic_renderer.as_ref() else {
        return;
    };

    let window_size = winit.window_size();
    let size = (window_size.w, window_size.h);
    let density_dpi = ndk::density_dpi(&backend.android_app).max(1);
    // Not `winit.scale_factor()`: that reads `AConfiguration`, which still reports the 160 dpi
    // default on the first launch and only becomes accurate after a configuration change.
    let guest_scale_factor = ndk::scale_factor(&backend.android_app);
    backend.guest_scale_factor = guest_scale_factor;
    backend.refresh_rate_millihz = ndk::refresh_rate_millihz(&backend.android_app);
    backend.compositor.state.size = size.into();

    let mut display_state = crate::core::coordinate_transform::AuthoritativeDisplayState::new(
        window_size.w,
        window_size.h,
        density_dpi,
        backend.refresh_rate_millihz,
    );
    if let Some(scale) = backend
        .compositor
        .state
        .authoritative_display_state
        .kwin_scale
    {
        display_state.update_kwin_scale(scale);
    }
    crate::android::backend::wayland::output_state::sync_kwin_output_scale(&mut display_state);
    if let Some(surf_size) = backend
        .compositor
        .state
        .authoritative_display_state
        .observed_surface_size
    {
        display_state.update_observed_surface_size(surf_size);
    }
    backend.compositor.state.authoritative_display_state = display_state;
    backend.compositor.state.coordinate_transform = display_state.coordinate_transform();
    backend.compositor.state.kwin_surface_scale = display_state.presentation_scale();

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
        configure_toplevel(surface, configure_size.0, configure_size.1);
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
    if backend.graphic_renderer.is_none() {
        match bind(event_loop) {
            Ok(winit) => backend.graphic_renderer = Some(winit),
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

    configure_output(backend);
    accessibility::set_runtime_active(true);

    if let Some(winit) = backend.graphic_renderer.as_ref() {
        winit.window().request_redraw();
    }
    handle(CentralizedEvent::Redraw, backend, event_loop);
    if backend.graphic_renderer.is_none() {
        log::error!("Initial Wayland frame failed; guest session will not be launched");
        return false;
    }
    launch();
    // Start the standalone-client PipeWire/AAudio backend.
    pipewire_standalone_aaudio::spawn_after_ready(android_app.clone());
    true
}

fn configure_toplevel(surface: &ToplevelSurface, width: i32, height: i32) {
    surface.with_pending_state(|state| {
        state.size.replace((width, height).into());
        state.states.set(xdg_toplevel::State::Activated);
        state.states.set(xdg_toplevel::State::Fullscreen);
        state.states.set(xdg_toplevel::State::Maximized);
    });
    surface.send_configure();
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
            let result = if show {
                ime::show(&backend.android_app)
            } else {
                ime::hide(&backend.android_app)
            };
            if let Err(error) = result {
                log::warn!("Could not update Android software-keyboard visibility: {error}");
            }
        }

        for text in ime::drain_commits() {
            if !backend.compositor.state.commit_android_text(&text) {
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
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let mut runtime_failed = false;
        if let PolarBearBackend::Wayland(backend) = &mut self.backend {
            if backend.graphic_renderer.is_none() {
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
            runtime_failed = backend.graphic_renderer.is_none();
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
            backend.graphic_renderer = None;
            backend.suspend_input_and_presentation();
            // Kill the standalone-client PipeWire/AAudio backend if it was started.
            pipewire_standalone_aaudio::shutdown();
        }
    }
}

/// Forward Android IME commits through the same physical-key path as a hardware keyboard.
///
/// The compositor currently receives evdev key events, not arbitrary Unicode strings. The
/// host-testable policy intentionally handles printable ASCII plus editing/control keys; a
/// non-ASCII commit is logged and dropped until the guest text-input-v3/virtual-keyboard bridge
/// can carry it without guessing the user's keyboard layout.
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
        let time = backend.clock.now().as_millis() as u64;
        if shift_required {
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
        if shift_required {
            let time = backend.clock.now().as_millis() as u64;
            handle(
                centralize_injected_keyboard(42, ElementState::Released, time, backend),
                backend,
                event_loop,
            );
        }
    }
}
