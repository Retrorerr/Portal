use crate::android::{
    accessibility,
    backend::wayland::{
        compositor::{send_frames_surface_tree, ClientState, State},
        write_guest_output_state, AndroidFrameTimestampSample, AndroidFrameTimestampSupport,
        CentralizedEvent, PendingKwinPresentation, TouchMode, WaylandBackend,
    },
};
use crate::core::wayland_protocol::{FrameEvent, FrameTrace};
use smithay::backend::input::ButtonState;
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::draw_render_elements;
use smithay::backend::renderer::{Color32F, Frame, Renderer};
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{self, CursorImageStatus, CursorImageSurfaceData};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_pointer::ButtonState as WlButtonState;
use smithay::utils::{IsAlive, Point, Rectangle, Transform, SERIAL_COUNTER};
use smithay::wayland::shell::xdg::ToplevelSurface;
use smithay::wayland::{
    compositor::{with_states, with_surface_tree_downward, TraversalAction},
    presentation::{PresentationFeedbackCachedState, PresentationFeedbackCallback, Refresh},
};
use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, Event, InputEvent, KeyboardKeyEvent, PointerAxisEvent,
        PointerButtonEvent,
    },
    output::{Mode, Scale},
};
use std::sync::Arc;
use winit::event_loop::{ActiveEventLoop, ControlFlow};

/// Linux input event code for the left mouse button (`BTN_LEFT`).
const BTN_LEFT: u32 = 0x110;
/// Linux input event code for the right mouse button (`BTN_RIGHT`).
const BTN_RIGHT: u32 = 0x111;

/// The nested session compositor presents one full-screen toplevel to Android.
fn get_surface(state: &State) -> Option<ToplevelSurface> {
    state
        .xdg_shell_state
        .toplevel_surfaces()
        .iter()
        .next()
        .cloned()
}

fn pointer_focus(
    state: &State,
) -> Option<(
    smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    Point<f64, smithay::utils::Logical>,
)> {
    get_surface(state).map(|surface| (surface.wl_surface().clone(), (0f64, 0f64).into()))
}

fn clamp_coordinates(state: &State, x: f64, y: f64) -> (f64, f64) {
    let logical = state.coordinate_transform.logical_source();
    (
        crate::core::android_integration::clamp_physical_coordinate(x, logical.width as i32),
        crate::core::android_integration::clamp_physical_coordinate(y, logical.height as i32),
    )
}

fn emit_pointer_motion(
    compositor: &mut crate::android::backend::wayland::Compositor,
    x: f64,
    y: f64,
    time: u32,
) {
    let pointer = compositor.pointer.clone();
    let state = &mut compositor.state;
    let (clamped_x, clamped_y) = clamp_coordinates(state, x, y);
    let round_trip = state.coordinate_transform.logical_to_physical(
        crate::core::coordinate_transform::LogicalPoint {
            x: clamped_x,
            y: clamped_y,
        },
    );
    let physical = state
        .coordinate_transform
        .logical_to_physical(crate::core::coordinate_transform::LogicalPoint { x, y });
    let error_px =
        ((round_trip.x - physical.x).powi(2) + (round_trip.y - physical.y).powi(2)).sqrt();
    log::info!(
        "input.alignment source=touch physical=({:.1},{:.1}) logical=({clamped_x:.1},{clamped_y:.1}) round_trip=({:.1},{:.1}) error_px={error_px:.3}",
        physical.x,
        physical.y,
        round_trip.x,
        round_trip.y
    );
    if let Some(focus) = pointer_focus(state) {
        let serial = SERIAL_COUNTER.next_serial();
        pointer.motion(
            state,
            Some(focus),
            &pointer::MotionEvent {
                location: (clamped_x, clamped_y).into(),
                serial,
                time,
            },
        );
        pointer.frame(state);
    }
}

/// Press a button. Also moves keyboard focus to the surface under the pointer.
fn emit_pointer_press(
    compositor: &mut crate::android::backend::wayland::Compositor,
    button: u32,
    time: u32,
) {
    let pointer = compositor.pointer.clone();
    let state = &mut compositor.state;
    if let Some(surface) = get_surface(state) {
        compositor.keyboard.set_focus(
            state,
            Some(surface.wl_surface().clone()),
            SERIAL_COUNTER.next_serial().into(),
        );
    }

    let serial = SERIAL_COUNTER.next_serial();
    pointer.button(
        state,
        &pointer::ButtonEvent {
            button,
            state: ButtonState::Pressed,
            serial,
            time,
        },
    );
    pointer.frame(state);
}

/// Release a button.
fn emit_pointer_release(
    compositor: &mut crate::android::backend::wayland::Compositor,
    button: u32,
    time: u32,
) {
    let pointer = compositor.pointer.clone();
    let state = &mut compositor.state;
    let serial = SERIAL_COUNTER.next_serial();
    pointer.button(
        state,
        &pointer::ButtonEvent {
            button,
            state: ButtonState::Released,
            serial,
            time,
        },
    );
    pointer.frame(state);
}

/// A full tap: move to the location, then a press immediately followed by a release.
fn emit_pointer_click(
    compositor: &mut crate::android::backend::wayland::Compositor,
    button: u32,
    x: f64,
    y: f64,
    time: u32,
) {
    emit_pointer_motion(compositor, x, y, time);
    emit_pointer_press(compositor, button, time);
    emit_pointer_release(compositor, button, time);
}

/// Arm the long press once the finger has stayed put for `ViewConfiguration`'s timeout.
///
/// No button is sent here: moving afterwards starts a drag with the left button held, lifting
/// instead fires a right click. Called from the redraw loop, which already ticks every frame.
fn poll_long_press(backend: &mut WaylandBackend) {
    if backend.touch_mode != TouchMode::Undecided || backend.touch_points.len() != 1 {
        return;
    }
    let (Some(down_time), Some(down_position)) =
        (backend.touch_down_time, backend.touch_down_position)
    else {
        return;
    };
    let now = backend.clock.now().as_millis() as u64;
    if now.saturating_sub(down_time) < backend.long_press_timeout_ms {
        return;
    }

    backend.touch_mode = TouchMode::LongPress;
    // Anchor the pointer where the finger landed, so a drag selects from there.
    let logical = backend
        .compositor
        .state
        .coordinate_transform
        .physical_to_logical(crate::core::coordinate_transform::PhysicalPoint {
            x: down_position.x,
            y: down_position.y,
        });
    emit_pointer_motion(&mut backend.compositor, logical.x, logical.y, now as u32);
}

pub fn handle(event: CentralizedEvent, backend: &mut WaylandBackend, event_loop: &ActiveEventLoop) {
    match event {
        CentralizedEvent::CloseRequested => {
            event_loop.exit();
        }
        CentralizedEvent::Redraw => {
            poll_long_press(backend);

            if let Err(error) = redraw(backend) {
                log::error!("Redraw failed; dropping renderer until next resume: {error}");
                backend.graphic_renderer = None;
                accessibility::set_runtime_active(false);
                event_loop.set_control_flow(ControlFlow::Wait);
                return;
            }

            // Redraw the application.
            //
            // It's preferable for applications that do not render continuously to render in
            // this event rather than in AboutToWait, since rendering in here allows
            // the program to gracefully handle redraws requested by the OS.

            // Draw.

            // Queue a RedrawRequested event.
            //
            // You only need to call this if you've determined that you need to redraw in
            // applications which do not always need to. Applications that redraw continuously
            // can render here instead.
            if let Some(winit) = backend.graphic_renderer.as_ref() {
                winit.window().request_redraw();
            }
        }
        CentralizedEvent::Input(event) => match event {
            InputEvent::Keyboard { event } => {
                let compositor = &mut backend.compositor;
                let state = &mut compositor.state;
                let serial = SERIAL_COUNTER.next_serial();
                let time = compositor.start_time.elapsed().as_millis() as u32;
                compositor.keyboard.input::<(), _>(
                    state,
                    event.key_code(),
                    event.state(),
                    serial,
                    time,
                    |_, _, _| {
                        //
                        FilterResult::Forward
                    },
                );
            }
            InputEvent::TouchDown { event } => {
                // Just move the cursor. Which button (if any) this gesture sends is only known
                // once the finger moves, lifts, or sits still long enough to be a long press.
                emit_pointer_motion(
                    &mut backend.compositor,
                    event.x(),
                    event.y(),
                    event.time_msec(),
                );
            }
            InputEvent::TouchMotion { event } => {
                let time = event.time_msec();

                // The centralizer only emits motion in Drag mode, and flips into it on the
                // first move after a long press — that transition is where the grab starts.
                if !backend.pointer_pressed {
                    emit_pointer_press(&mut backend.compositor, BTN_LEFT, time);
                    backend.pointer_pressed = true;
                }

                emit_pointer_motion(&mut backend.compositor, event.x(), event.y(), time);
            }
            InputEvent::TouchUp { event } => {
                let time = event.time_msec();

                if backend.pointer_pressed {
                    // End of a drag.
                    emit_pointer_motion(&mut backend.compositor, event.x, event.y, time);
                    emit_pointer_release(&mut backend.compositor, BTN_LEFT, time);
                    backend.pointer_pressed = false;
                } else {
                    match event.mode {
                        // A tap: left click where the finger lifted.
                        TouchMode::Undecided => emit_pointer_click(
                            &mut backend.compositor,
                            BTN_LEFT,
                            event.x,
                            event.y,
                            time,
                        ),
                        // Held still, then lifted without moving: a context menu, as on Android.
                        TouchMode::LongPress => emit_pointer_click(
                            &mut backend.compositor,
                            BTN_RIGHT,
                            event.x,
                            event.y,
                            time,
                        ),
                        // A scroll consumed the gesture; nothing to click.
                        TouchMode::Scroll | TouchMode::Drag => {}
                    }
                }
            }
            InputEvent::TouchCancel { event } => {
                if backend.pointer_pressed {
                    emit_pointer_release(&mut backend.compositor, BTN_LEFT, event.time() as u32);
                    backend.pointer_pressed = false;
                }
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let compositor = &mut backend.compositor;
                let pointer = compositor.pointer.clone();
                let serial = SERIAL_COUNTER.next_serial();
                let (clamped_x, clamped_y) =
                    clamp_coordinates(&compositor.state, event.x(), event.y());

                let round_trip = compositor.state.coordinate_transform.logical_to_physical(
                    crate::core::coordinate_transform::LogicalPoint {
                        x: clamped_x,
                        y: clamped_y,
                    },
                );
                let error_px = ((round_trip.x - event.physical_x()).powi(2)
                    + (round_trip.y - event.physical_y()).powi(2))
                .sqrt();
                let kwin_scale = compositor
                    .state
                    .authoritative_display_state
                    .effective_kwin_scale();
                if let (Some(dev), Some(src), Some(tool)) = (
                    event.android_device_id(),
                    event.android_source(),
                    event.android_tool_type(),
                ) {
                    log::info!(
                        "input.alignment device={dev} source={src:#x} tool={tool} physical=({:.1},{:.1}) logical=({clamped_x:.1},{clamped_y:.1}) round_trip=({:.1},{:.1}) error_px={error_px:.3} scale={kwin_scale:.3}",
                        event.physical_x(),
                        event.physical_y(),
                        round_trip.x,
                        round_trip.y
                    );
                } else {
                    log::info!(
                        "input.alignment physical=({:.1},{:.1}) logical=({clamped_x:.1},{clamped_y:.1}) round_trip=({:.1},{:.1}) error_px={error_px:.3} scale={kwin_scale:.3}",
                        event.physical_x(),
                        event.physical_y(),
                        round_trip.x,
                        round_trip.y
                    );
                }

                if let Some(surface) = get_surface(&compositor.state) {
                    pointer.motion(
                        &mut compositor.state,
                        Some((surface.wl_surface().clone(), (0f64, 0f64).into())),
                        &pointer::MotionEvent {
                            location: (clamped_x, clamped_y).into(),
                            serial,
                            time: event.time_msec(),
                        },
                    );
                }
                pointer.frame(&mut compositor.state);
            }
            InputEvent::PointerButton { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let button = event.button_code();

                let state = WlButtonState::from(event.state());

                let compositor = &mut backend.compositor;
                let pointer = compositor.pointer.clone();

                if let Some(surface) = get_surface(&compositor.state) {
                    compositor.keyboard.set_focus(
                        &mut compositor.state,
                        Some(surface.wl_surface().clone()),
                        0.into(),
                    );
                }
                pointer.button(
                    &mut compositor.state,
                    &pointer::ButtonEvent {
                        button,
                        state: state.try_into().unwrap(),
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(&mut compositor.state);
            }
            InputEvent::PointerAxis { event } => {
                // A second finger can turn an in-progress drag into a scroll; drop the button
                // the drag was holding rather than scrolling with it down.
                if backend.pointer_pressed {
                    emit_pointer_release(&mut backend.compositor, BTN_LEFT, event.time_msec());
                    backend.pointer_pressed = false;
                }
                let horizontal_amount = event
                    .amount(Axis::Horizontal)
                    .unwrap_or_else(|| event.amount_v120(Axis::Horizontal).unwrap_or(0.0) / 120.);
                let vertical_amount = event
                    .amount(Axis::Vertical)
                    .unwrap_or_else(|| event.amount_v120(Axis::Vertical).unwrap_or(0.0) / 120.);
                let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
                let vertical_amount_discrete = event.amount_v120(Axis::Vertical);

                {
                    let mut frame =
                        pointer::AxisFrame::new(event.time_msec()).source(event.source());
                    if horizontal_amount != 0.0 {
                        frame = frame.relative_direction(
                            Axis::Horizontal,
                            event.relative_direction(Axis::Horizontal),
                        );
                        frame = frame.value(Axis::Horizontal, horizontal_amount);
                        if let Some(discrete) = horizontal_amount_discrete {
                            frame = frame.v120(Axis::Horizontal, discrete as i32);
                        }
                    }
                    if vertical_amount != 0.0 {
                        frame = frame.relative_direction(
                            Axis::Vertical,
                            event.relative_direction(Axis::Vertical),
                        );
                        frame = frame.value(Axis::Vertical, vertical_amount);
                        if let Some(discrete) = vertical_amount_discrete {
                            frame = frame.v120(Axis::Vertical, discrete as i32);
                        }
                    }
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                    let compositor = &mut backend.compositor;
                    let pointer = compositor.pointer.clone();
                    pointer.axis(&mut compositor.state, frame);
                    pointer.frame(&mut compositor.state);
                }
            }
            _ => {}
        },
        CentralizedEvent::Resized {
            size,
            guest_scale_factor,
        } => {
            backend.compositor.state.size = (size.w, size.h).into();
            backend
                .compositor
                .state
                .authoritative_display_state
                .update_physical_size(size.w, size.h);
            let density_dpi = (guest_scale_factor * 160.0).round().max(160.0) as i32;
            backend
                .compositor
                .state
                .authoritative_display_state
                .update_density_dpi(density_dpi);
            backend.compositor.state.coordinate_transform = backend
                .compositor
                .state
                .authoritative_display_state
                .coordinate_transform();
            backend.compositor.state.kwin_surface_scale = backend
                .compositor
                .state
                .authoritative_display_state
                .presentation_scale();

            if let Some(output) = &backend.compositor.output {
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
            }

            let guest_scale = guest_scale_factor.round().max(1.0) as i32;
            write_guest_output_state(size.w, size.h, guest_scale);

            let configure_size = backend
                .compositor
                .state
                .authoritative_display_state
                .configure_size();
            if let Some(surface) = get_surface(&backend.compositor.state) {
                configure_toplevel(&surface, (configure_size.0, configure_size.1).into());
            }
        }
        CentralizedEvent::Focus(focused) => {
            if !focused {
                backend.suspend_input_and_presentation();
            }
        }
        _ => (),
    }
}

fn complete_kwin_android_presentation(
    backend: &mut WaylandBackend,
    sample: AndroidFrameTimestampSample,
) {
    let Some(pending) = backend.pending_kwin_presentation else {
        log::debug!(
            "Android display-present sample has no KWin readiness candidate: egl_frame_id={} timestamp_ns={}",
            sample.frame_id,
            sample.timestamp_ns
        );
        return;
    };

    if sample.frame_id < pending.egl_frame_id {
        // A non-KWin frame can complete first; keep waiting for the exact
        // frame id that contained the identified KWin surface.
        return;
    }
    if sample.frame_id > pending.egl_frame_id {
        // The EGL implementation retired the pending id before it became
        // queryable. Never let a later frame satisfy the old generation.
        backend.pending_kwin_presentation = None;
        crate::android::diagnostics::host_event(
            "wayland-readiness",
            &format!(
                "stage=kwin-frame-timestamp-lost generation={} expected_egl_frame_id={} observed_egl_frame_id={}",
                pending.generation, pending.egl_frame_id, sample.frame_id
            ),
        );
        return;
    }

    backend.pending_kwin_presentation = None;
    if backend.compositor.state.kwin_generation != Some(pending.generation) {
        log::debug!(
            "Ignoring Android display-present sample from stale KWin generation={} egl_frame_id={}",
            pending.generation,
            sample.frame_id
        );
        return;
    }

    if let Some(generation) = backend
        .compositor
        .state
        .mark_kwin_frame_presented_with_evidence(
            "egl-android-display-present",
            Some(sample.timestamp_ns),
        )
    {
        let surface_count = backend
            .compositor
            .state
            .xdg_shell_state
            .toplevel_surfaces()
            .len();
        let client_count = backend.compositor.clients.len();
        crate::android::diagnostics::mark_plasma_frame_presented_for_generation_with_evidence(
            surface_count,
            client_count,
            generation,
            "egl-android-display-present",
            Some(sample.timestamp_ns),
        );
    }
}

fn complete_kwin_presentation_without_android_timestamp(
    backend: &mut WaylandBackend,
    generation: u64,
) {
    if backend.pending_kwin_presentation.is_some() {
        return;
    }
    if let Some(generation) = backend
        .compositor
        .state
        .mark_kwin_frame_presented_with_evidence(
            "egl-swap-and-wayland-feedback-no-frame-timestamp",
            None,
        )
    {
        let surface_count = backend
            .compositor
            .state
            .xdg_shell_state
            .toplevel_surfaces()
            .len();
        let client_count = backend.compositor.clients.len();
        crate::android::diagnostics::mark_plasma_frame_presented_for_generation_with_evidence(
            surface_count,
            client_count,
            generation,
            "egl-swap-and-wayland-feedback-no-frame-timestamp",
            None,
        );
    }
}

fn redraw(backend: &mut WaylandBackend) -> Result<(), String> {
    // Android reports the physical display-present timestamp asynchronously.
    // Poll every sample rather than just the first one: unrelated frames can
    // be queued ahead of the KWin frame, and dropping a valid sample would
    // make the exact EGL/KWin correlation nondeterministic.
    let timestamp_samples = {
        let Some(winit) = backend.graphic_renderer.as_mut() else {
            return Ok(());
        };
        winit.poll_android_frame_timestamps()
    };
    for sample in timestamp_samples {
        complete_kwin_android_presentation(backend, sample);
    }

    let Some(winit) = backend.graphic_renderer.as_mut() else {
        return Ok(());
    };

    // Android clipboard polling and transfer workers never touch the render thread; apply only
    // completed, immutable changes here before dispatching the next batch of Wayland requests.
    backend.compositor.process_android_clipboard();

    let size = winit.window_size();
    let damage = Rectangle::from_size(size);
    let mut presentation_feedbacks = Vec::new();
    let mut frame_trace = FrameTrace::new();

    // Process requests before taking the renderer snapshot.  The previous ordering rendered
    // the old committed state, dispatched the next wl_surface.commit, then acknowledged that
    // commit's wl_surface.frame callback after the swap.  KWin uses the callback as the host
    // compositor's compositing/presentation point, so that ordering could make it submit the
    // next frame while its acknowledged buffer was still not on the Android surface.  Keeping
    // dispatch before rendering makes frame.done and wp_presentation refer to the state rendered
    // by this iteration.
    {
        let compositor = &mut backend.compositor;
        match compositor.listener.accept() {
            Ok(Some(stream)) => match compositor
                .display
                .handle()
                .insert_client(stream, Arc::new(ClientState::default()))
            {
                Ok(client) => compositor.clients.push(client),
                Err(error) => log::error!("Failed to insert Wayland client: {error}"),
            },
            Ok(None) => {}
            Err(error) => log::error!("Failed to accept Wayland client: {error}"),
        }

        compositor
            .display
            .dispatch_clients(&mut compositor.state)
            .map_err(|error| format!("Failed to dispatch clients: {error}"))?;
        compositor
            .display
            .flush_clients()
            .map_err(|error| format!("Failed to flush clients: {error}"))?;
        compositor.observe_kwin_surfaces();
        compositor.sync_data_device_focus();
        record_protocol_event(&mut frame_trace, FrameEvent::Dispatch);
    }

    // Keep the render elements alive through submit().  Smithay releases a client wl_buffer
    // when its last render-element reference is dropped; releasing it before the EGL swap can
    // let KWin reuse a shm buffer while the Android renderer is still consuming its texture.
    let mut kwin_surface_rendered = false;
    let mut kwin_feedback_requested = false;
    let rendered_elements = {
        let (renderer, mut framebuffer) = winit
            .bind()
            .map_err(|error| format!("Failed to bind EGL surface: {error}"))?;

        let compositor = &mut backend.compositor;

        let toplevels = compositor
            .state
            .xdg_shell_state
            .toplevel_surfaces()
            .to_vec();

        // Sync KWin configured scale from guest configuration (e.g. if scale changed via kscreen-doctor or Plasma Settings)
        let scale_changed = crate::android::backend::wayland::output_state::sync_kwin_output_scale(
            &mut compositor.state.authoritative_display_state,
        );
        if scale_changed {
            compositor.state.coordinate_transform = compositor
                .state
                .authoritative_display_state
                .coordinate_transform();
            log::info!(
                "KWin configured scale changed: scale={:.3} logical_geom={:?}",
                compositor
                    .state
                    .authoritative_display_state
                    .effective_kwin_scale(),
                compositor
                    .state
                    .authoritative_display_state
                    .logical_geometry(),
            );
        }

        // Detect committed KWin geometry and update presentation scale if changed
        for surface in &toplevels {
            if compositor.state.is_known_kwin_surface(surface.wl_surface()) {
                let detected = smithay::backend::renderer::utils::with_renderer_surface_state(
                    surface.wl_surface(),
                    |state| {
                        let candidate_size = state.surface_size().or_else(|| state.buffer_size());
                        if let Some(surf_size) = candidate_size {
                            if surf_size.w > 0 && surf_size.h > 0 {
                                return Some((surf_size.w as f64, surf_size.h as f64));
                            }
                        }
                        None
                    },
                )
                .flatten();

                if let Some(surf_size) = detected {
                    if compositor
                        .state
                        .authoritative_display_state
                        .update_observed_surface_size(surf_size)
                    {
                        compositor.state.kwin_surface_scale = compositor
                            .state
                            .authoritative_display_state
                            .presentation_scale();
                        log::info!(
                            "Authoritative display presentation scale updated: surface_size=({:.1}, {:.1}) presentation_scale=({:.2}, {:.2})",
                            surf_size.0,
                            surf_size.1,
                            compositor.state.kwin_surface_scale.0,
                            compositor.state.kwin_surface_scale.1,
                        );
                    }
                }
                break;
            }
        }

        let presentation_scale = compositor
            .state
            .authoritative_display_state
            .presentation_scale();
        let scale_to_use = presentation_scale.0.max(1.0);

        let mut elements = Vec::new();
        let mut non_kwin_elements = Vec::new();
        let mut kwin_elements = Vec::new();
        if toplevels.len() > 1 {
            log::debug!("event_handler: toplevels count={}", toplevels.len());
        }
        for surface in &toplevels {
            let surface_scale = if compositor.state.is_known_kwin_surface(surface.wl_surface()) {
                scale_to_use
            } else {
                1.0
            };
            let surface_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                render_elements_from_surface_tree(
                    renderer,
                    surface.wl_surface(),
                    (0, 0),
                    surface_scale,
                    1.0,
                    Kind::Unspecified,
                );
            if toplevels.len() > 1 {
                log::debug!(
                    "surface {:?} produced {} elements",
                    surface.wl_surface(),
                    surface_elements.len()
                );
            }

            if compositor.state.is_known_kwin_surface(surface.wl_surface()) {
                if !surface_elements.is_empty() {
                    kwin_surface_rendered = true;
                }
                kwin_elements.extend(surface_elements);
            } else {
                non_kwin_elements.extend(surface_elements);
            }
        }
        // Front-to-back ordering: non-KWin clients/overlays in front of nested KWin desktop
        elements.extend(non_kwin_elements);
        elements.extend(kwin_elements);

        let cursor_surface = match &compositor.state.cursor_image {
            CursorImageStatus::Surface(surface) if surface.alive() => Some(surface.clone()),
            _ => None,
        };
        if let Some(surface) = &cursor_surface {
            let hotspot = with_states(surface, |states| {
                states
                    .data_map
                    .get::<CursorImageSurfaceData>()
                    .and_then(|attributes| {
                        attributes.lock().ok().map(|attributes| attributes.hotspot)
                    })
                    .unwrap_or_default()
            });
            let pointer_logical = compositor.pointer.current_location();
            let pointer_physical = compositor.state.coordinate_transform.logical_to_physical(
                crate::core::coordinate_transform::LogicalPoint {
                    x: pointer_logical.x,
                    y: pointer_logical.y,
                },
            );
            let elem_phys_x = pointer_physical.x - (hotspot.x as f64) * presentation_scale.0;
            let elem_phys_y = pointer_physical.y - (hotspot.y as f64) * presentation_scale.1;
            let location = (elem_phys_x.round() as i32, elem_phys_y.round() as i32);
            elements.extend(render_elements_from_surface_tree(
                renderer,
                surface,
                (location.0, location.1),
                scale_to_use,
                1.0,
                Kind::Cursor,
            ));
        }

        let mut frame = renderer
            .render(&mut framebuffer, size, Transform::Flipped180)
            .map_err(|error| format!("Failed to render frame: {error:?}"))?;
        frame
            .clear(Color32F::new(0.1, 0.0, 0.0, 1.0), &[damage])
            .map_err(|error| format!("Failed to clear frame: {error:?}"))?;
        draw_render_elements(&mut frame, scale_to_use, &elements, &[damage])
            .map_err(|error| format!("Failed to draw render elements: {error:?}"))?;
        // We rely on the nested compositor to do the sync for us.
        let _ = frame
            .finish()
            .map_err(|error| format!("Failed to finish frame: {error:?}"))?;

        for surface in &toplevels {
            let surface_feedbacks = take_presentation_feedbacks_surface_tree(surface.wl_surface());
            if compositor.state.is_known_kwin_surface(surface.wl_surface())
                && !surface_feedbacks.is_empty()
            {
                // KWin queues one FrameData entry containing both its
                // presentation feedback and wl_surface.frame callback before
                // committing the output root. This bit ties readiness to that
                // specific KWin output surface, not to a recovery client.
                kwin_feedback_requested = true;
            }
            presentation_feedbacks.extend(surface_feedbacks);
        }
        if let Some(surface) = &cursor_surface {
            // Cursor surfaces are not children of the xdg toplevel tree, but they can also
            // request wp_presentation_feedback and are rendered in this frame.
            presentation_feedbacks.extend(take_presentation_feedbacks_surface_tree(surface));
        }

        record_protocol_event(&mut frame_trace, FrameEvent::Render);

        elements
    };

    // It is important that all events on the display have been dispatched and flushed to clients
    // before swapping buffers because this operation may block.
    let submitted_frame_id = winit
        .submit(Some(&[damage]))
        .map_err(|error| format!("Failed to submit frame: {error}"))?;
    let timestamp_support = winit.android_frame_timestamp_support();
    record_protocol_event(&mut frame_trace, FrameEvent::Submit);

    // The host has now submitted the frame represented by rendered_elements.  Do not release
    // client buffers until after submit() has returned (see the invariant above).
    drop(rendered_elements);

    let frame_time = backend.compositor.start_time.elapsed().as_millis() as u32;
    for surface in backend.compositor.state.xdg_shell_state.toplevel_surfaces() {
        send_frames_surface_tree(surface.wl_surface(), frame_time);
    }
    if let CursorImageStatus::Surface(surface) = &backend.compositor.state.cursor_image {
        if surface.alive() {
            send_frames_surface_tree(surface, frame_time);
        }
    }
    record_protocol_event(&mut frame_trace, FrameEvent::FrameDone);

    if let Some(output) = &backend.compositor.output {
        let presentation_time: std::time::Duration = backend.clock.now().into();
        let refresh = Refresh::fixed(std::time::Duration::from_nanos(
            crate::core::android_integration::refresh_period_nanos(backend.refresh_rate_millihz),
        ));
        backend.presentation_sequence = backend.presentation_sequence.wrapping_add(1);
        for feedback in presentation_feedbacks {
            feedback.presented(
                output,
                presentation_time,
                refresh,
                backend.presentation_sequence,
                // The Android EGL swap is not, by itself, proof that the
                // display hardware synchronized or latched this content. Do
                // not advertise the protocol's hardware/vsync flags until a
                // real Android frame-timestamp sample is available.
                smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::empty(),
            );
        }
        if kwin_surface_rendered && kwin_feedback_requested {
            let generation = backend.compositor.state.kwin_frame_generation();
            match (submitted_frame_id, generation, timestamp_support) {
                (Some(egl_frame_id), Some(generation), _) => {
                    if backend.pending_kwin_presentation.is_none() {
                        backend.pending_kwin_presentation = Some(PendingKwinPresentation {
                            generation,
                            egl_frame_id,
                        });
                        crate::android::diagnostics::host_event(
                            "wayland-readiness",
                            &format!(
                                "stage=kwin-frame-awaiting-android-present generation={} egl_frame_id={egl_frame_id}",
                                generation
                            ),
                        );
                    } else {
                        log::debug!(
                            "wayland.readiness already awaiting Android presentation for a KWin frame"
                        );
                    }
                }
                // If the device does not expose the Android timestamp
                // extension, retain the best protocol-level proof but make it
                // explicit that this is not a hardware scanout measurement.
                (None, Some(generation), AndroidFrameTimestampSupport::Unsupported) => {
                    complete_kwin_presentation_without_android_timestamp(backend, generation);
                }
                (_, None, _) => {
                    // A disconnected or superseded KWin surface cannot make
                    // the current launch ready, even if EGL accepted a frame.
                }
                (None, _, support) => {
                    log::debug!(
                        "No Android EGL frame id for KWin readiness candidate; timestamp_support={support:?}"
                    );
                }
            }
        }
        record_protocol_event(&mut frame_trace, FrameEvent::Presented);
    } else {
        for feedback in presentation_feedbacks {
            feedback.discarded();
        }
        record_protocol_event(&mut frame_trace, FrameEvent::Discarded);
    }

    backend
        .compositor
        .display
        .flush_clients()
        .map_err(|error| format!("Failed to flush presentation feedback: {error}"))?;

    Ok(())
}

/// Drain presentation feedback from the same tree that is handed to the renderer.
///
/// A feedback request belongs to the surface on which it was made.  Restricting the drain to
/// xdg roots strands feedback requested by a subsurface (or by the cursor surface), and KWin's
/// output backend expects every committed feedback to receive exactly one presented/discarded
/// event.  Traversing downward mirrors `render_elements_from_surface_tree` and keeps the two
/// lifecycles aligned.
fn take_presentation_feedbacks_surface_tree(
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> Vec<PresentationFeedbackCallback> {
    let mut feedbacks = Vec::new();
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_surface, states, &()| {
            feedbacks.extend(std::mem::take(
                &mut states
                    .cached_state
                    .get::<PresentationFeedbackCachedState>()
                    .current()
                    .callbacks,
            ));
        },
        |_, _, &()| true,
    );
    feedbacks
}

fn record_protocol_event(trace: &mut FrameTrace, event: FrameEvent) {
    if let Err(error) = trace.record(event) {
        log::error!("Wayland frame lifecycle violation at {event:?}: {error}");
    }
    log::debug!(
        "wayland.protocol frame_event={event:?} order={:?}",
        trace.events()
    );
}

fn configure_toplevel(
    surface: &ToplevelSurface,
    size: smithay::utils::Size<i32, smithay::utils::Logical>,
) {
    surface.with_pending_state(|state| {
        state.size = Some(size);
        state.states.set(xdg_toplevel::State::Activated);
        state.states.set(xdg_toplevel::State::Fullscreen);
        state.states.set(xdg_toplevel::State::Maximized);
    });
    surface.send_pending_configure();
}
