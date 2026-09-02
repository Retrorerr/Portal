use crate::android::{
    accessibility,
    backend::wayland::{
        compositor::{send_frames_surface_tree, ClientState, State},
        write_guest_output_state, CentralizedEvent, TouchMode, WaylandBackend,
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

fn emit_pointer_motion(
    compositor: &mut crate::android::backend::wayland::Compositor,
    x: f64,
    y: f64,
    time: u32,
) {
    let pointer = compositor.pointer.clone();
    let state = &mut compositor.state;
    if let Some(focus) = pointer_focus(state) {
        let serial = SERIAL_COUNTER.next_serial();
        pointer.motion(
            state,
            Some(focus),
            &pointer::MotionEvent {
                location: (x, y).into(),
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
    emit_pointer_motion(
        &mut backend.compositor,
        down_position.x,
        down_position.y,
        now as u32,
    );
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

                if let Some(surface) = get_surface(&compositor.state) {
                    pointer.motion(
                        &mut compositor.state,
                        Some((surface.wl_surface().clone(), (0f64, 0f64).into())),
                        &pointer::MotionEvent {
                            location: (event.x(), event.y()).into(),
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

            if let Some(surface) = get_surface(&backend.compositor.state) {
                configure_toplevel(&surface, (size.w, size.h).into());
            }
        }
        _ => (),
    }
}

fn redraw(backend: &mut WaylandBackend) -> Result<(), String> {
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
        compositor.sync_data_device_focus();
        record_protocol_event(&mut frame_trace, FrameEvent::Dispatch);
    }

    // Keep the render elements alive through submit().  Smithay releases a client wl_buffer
    // when its last render-element reference is dropped; releasing it before the EGL swap can
    // let KWin reuse a shm buffer while the Android renderer is still consuming its texture.
    let rendered_elements = {
        let (renderer, mut framebuffer) = winit
            .bind()
            .map_err(|error| format!("Failed to bind EGL surface: {error}"))?;

        let compositor = &mut backend.compositor;

        let mut elements = compositor
            .state
            .xdg_shell_state
            .toplevel_surfaces()
            .iter()
            .flat_map(|surface| {
                render_elements_from_surface_tree(
                    renderer,
                    surface.wl_surface(),
                    (0, 0),
                    1.0,
                    1.0,
                    Kind::Unspecified,
                )
            })
            .collect::<Vec<WaylandSurfaceRenderElement<GlesRenderer>>>();

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
            let location =
                (compositor.pointer.current_location() - hotspot.to_f64()).to_i32_round();
            elements.extend(render_elements_from_surface_tree(
                renderer,
                surface,
                (location.x, location.y),
                1.0,
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
        draw_render_elements(&mut frame, 1.0, &elements, &[damage])
            .map_err(|error| format!("Failed to draw render elements: {error:?}"))?;
        // We rely on the nested compositor to do the sync for us.
        let _ = frame
            .finish()
            .map_err(|error| format!("Failed to finish frame: {error:?}"))?;

        for surface in compositor.state.xdg_shell_state.toplevel_surfaces() {
            presentation_feedbacks.extend(take_presentation_feedbacks_surface_tree(
                surface.wl_surface(),
            ));
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
    winit
        .submit(Some(&[damage]))
        .map_err(|error| format!("Failed to submit frame: {error}"))?;
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
                smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::Vsync,
            );
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
    });
    surface.send_pending_configure();
}
