use super::bind::bind_socket;
use crate::android::clipboard::{
    is_valid_clip_text, ClipboardBridge, ClipboardEvent, ClipboardSelectionData, TEXT_MIME,
    UTF8_TEXT_MIME,
};
use crate::core::startup::{is_kwin_wayland_title, StartupReadiness};
use smithay::{
    backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state},
    delegate_compositor, delegate_data_device, delegate_fractional_scale, delegate_output,
    delegate_pointer_constraints, delegate_presentation, delegate_seat, delegate_shm,
    delegate_single_pixel_buffer, delegate_viewporter, delegate_xdg_shell,
    input::{
        keyboard::KeyboardHandle, pointer::CursorImageStatus, touch::TouchHandle, Seat,
        SeatHandler, SeatState,
    },
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{protocol::wl_seat, Display},
    },
    utils::{Logical, Serial, Size},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            with_states, with_surface_tree_downward, BufferAssignment, CompositorClientState,
            CompositorHandler, CompositorState, SurfaceAttributes, TraversalAction,
        },
        fractional_scale::{self, FractionalScaleHandler, FractionalScaleManagerState},
        output::OutputHandler,
        pointer_constraints::{PointerConstraintsHandler, PointerConstraintsState},
        presentation::PresentationState,
        selection::{
            data_device::{
                clear_data_device_selection, set_data_device_focus, set_data_device_selection,
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            },
            SelectionHandler, SelectionSource, SelectionTarget,
        },
        shell::xdg::{
            Configure, PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler,
            XdgShellState, XdgToplevelSurfaceData,
        },
        shm::{ShmHandler, ShmState},
        single_pixel_buffer::SinglePixelBufferState,
        viewporter::ViewporterState,
    },
};
use smithay::{
    input::pointer::PointerHandle,
    reexports::wayland_server::{
        backend::{ClientData, ClientId, DisconnectReason, GlobalId},
        protocol::{wl_buffer, wl_surface::WlSurface},
        Client, ListeningSocket, Resource,
    },
};
use std::{error::Error, os::unix::io::OwnedFd, time::Instant};
use winit::platform::android::activity::AndroidApp;

pub struct Compositor {
    pub state: State,
    pub display: Display<State>,
    pub listener: ListeningSocket,
    pub clients: Vec<Client>,
    pub start_time: Instant,
    pub seat: Seat<State>,
    pub keyboard: KeyboardHandle<State>,
    pub touch: TouchHandle<State>,
    pub pointer: PointerHandle<State>,
    pub output: Option<Output>,
    pub output_global: Option<GlobalId>,
}

pub struct State {
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub data_device_state: DataDeviceState,
    pub seat_state: SeatState<Self>,
    // KWin 6.7 treats these protocol globals as mandatory for its nested Wayland backend.
    // Keep the state values alive for as long as the display is alive.
    pub pointer_constraints_state: PointerConstraintsState,
    pub presentation_state: PresentationState,
    pub single_pixel_buffer_state: SinglePixelBufferState,
    pub viewporter_state: ViewporterState,
    pub fractional_scale_state: FractionalScaleManagerState,
    pub size: Size<i32, Logical>,
    pub output: Option<Output>,
    pub cursor_image: CursorImageStatus,
    /// Readiness evidence for the currently identified KWin output surface.
    pub readiness: StartupReadiness,
    /// The one KWin nested output surface whose lifecycle is allowed to
    /// satisfy readiness. Recovery clients are deliberately not interchangeable
    /// with this object, even if they also create xdg-toplevels.
    pub kwin_surface: Option<WlSurface>,
    pub kwin_client_id: Option<ClientId>,
    pub kwin_generation: Option<u64>,
    /// Android clipboard bridge. It is initialized only after provisioning has produced the
    /// Wayland backend, so setup/webview stages never touch Android clipboard state.
    pub clipboard_bridge: Option<ClipboardBridge>,
    pub ahb_importer: Option<crate::android::backend::wayland::gl_import::AhbTextureImporter>,
    /// Single authoritative display state for physical dimensions, configure size, and presentation.
    pub authoritative_display_state: crate::core::coordinate_transform::AuthoritativeDisplayState,
    /// Dynamic display scaling factor for KWin's nested output surface (scale_x, scale_y).
    pub kwin_surface_scale: (f64, f64),
    /// Single source of truth for Android physical, rendered viewport, and KWin logical space.
    pub coordinate_transform: crate::core::coordinate_transform::CoordinateTransform,
}

impl State {
    /// Read the xdg-toplevel title from Smithay's role data. KWin sets this
    /// before its first no-buffer commit, so the title is available during the
    /// initial dispatch even though the surface is not mapped yet.
    fn toplevel_title(surface: &ToplevelSurface) -> Option<String> {
        with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok().and_then(|data| data.title.clone()))
        })
    }

    /// Observe a toplevel and, only when its title matches KWin's nested output
    /// identity, make it the surface for a fresh readiness generation.
    ///
    /// Wayland does not provide a trusted process identity to the compositor.
    /// Pairing KWin's upstream title prefix with the owning `ClientId` and
    /// `wl_surface` object id keeps unrelated recovery surfaces out of the
    /// readiness path while retaining protocol-visible evidence.
    pub fn observe_kwin_toplevel(&mut self, surface: &ToplevelSurface) -> bool {
        let Some(title) = Self::toplevel_title(surface) else {
            return false;
        };
        if !is_kwin_wayland_title(&title) {
            return false;
        }

        let Some(client_id) = surface.wl_surface().client().map(|client| client.id()) else {
            return false;
        };
        let wl_surface = surface.wl_surface().clone();
        let is_new_identity = self
            .kwin_surface
            .as_ref()
            .map_or(true, |known| known != &wl_surface)
            || self.kwin_client_id.as_ref() != Some(&client_id);

        if is_new_identity {
            let generation = self.readiness.begin_generation();
            self.kwin_surface = Some(wl_surface.clone());
            self.kwin_client_id = Some(client_id.clone());
            self.kwin_generation = Some(generation);
            let _ = self.readiness.mark_kwin_connected_for(generation);
            let _ = self.readiness.mark_surface_created_for(generation);
            log::info!(
                "wayland.readiness kwin_identity generation={} client={:?} surface={:?} title={:?}",
                generation,
                client_id,
                wl_surface.id(),
                title
            );
            crate::android::diagnostics::host_event(
                "wayland-readiness",
                &format!(
                    "stage=kwin-identified generation={} client={:?} surface={:?} title={}",
                    generation,
                    self.kwin_client_id,
                    wl_surface.id(),
                    title
                ),
            );
        }

        let Some(generation) = self.kwin_generation else {
            return true;
        };

        // The title request and xdg_surface.ack_configure can arrive in the
        // same dispatch batch. If the ack was already processed before this
        // title was observed, synchronize strict readiness from role data.
        let configured = with_states(&wl_surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok().map(|data| data.configured))
                .unwrap_or(false)
        });
        if configured {
            let _ = self.readiness.mark_configure_acked_for(generation);
        }
        true
    }

    /// Return whether a surface is exactly the identified KWin surface for the
    /// current client generation.
    pub fn is_known_kwin_surface(&self, surface: &WlSurface) -> bool {
        let Some(known) = self.kwin_surface.as_ref() else {
            return false;
        };
        if known != surface {
            return false;
        }
        match (&self.kwin_client_id, surface.client()) {
            (Some(expected), Some(client)) => expected == &client.id(),
            _ => false,
        }
    }

    /// Capture a buffer commit before Smithay consumes the assignment, or
    /// synchronize an already-consumed renderer buffer after title discovery.
    pub fn observe_kwin_buffer(&mut self, surface: &WlSurface, newly_committed: bool) -> bool {
        if !self.is_known_kwin_surface(surface) {
            return false;
        }
        let has_buffer = newly_committed
            || with_renderer_surface_state(surface, |state| state.buffer().is_some())
                .unwrap_or(false);
        if !has_buffer {
            return false;
        }
        let Some(generation) = self.kwin_generation else {
            return false;
        };
        let advanced = self.readiness.mark_buffer_committed_for(generation);
        if advanced {
            log::info!(
                "wayland.readiness stage=buffer-committed generation={} surface={:?}",
                generation,
                surface.id()
            );
            crate::android::diagnostics::host_event(
                "wayland-readiness",
                &format!(
                    "stage=buffer-committed generation={} surface={:?}",
                    generation,
                    surface.id()
                ),
            );
        }
        advanced
    }

    /// Return the current KWin generation when a rendered frame and feedback
    /// request are ready to be correlated with an EGL frame id.
    pub fn kwin_frame_generation(&self) -> Option<u64> {
        let generation = self.kwin_generation?;
        if self.readiness.generation() == generation
            && self.readiness.configure_acked
            && self.readiness.buffer_committed
            && !self.readiness.frame_presented
        {
            Some(generation)
        } else {
            None
        }
    }

    /// Record that an identified KWin frame has reached the requested proof
    /// point. Physical Android display-present timestamps use a stronger
    /// evidence label than the no-timestamp fallback.
    pub fn mark_kwin_frame_presented_with_evidence(
        &mut self,
        evidence: &str,
        presentation_timestamp_ns: Option<i64>,
    ) -> Option<u64> {
        let generation = self.kwin_generation?;
        if self.readiness.mark_frame_presented_for(generation) {
            log::info!(
                "wayland.readiness stage=android-frame-presented generation={} surface={:?} evidence={} android_present_ns={:?}",
                generation,
                self.kwin_surface.as_ref().map(|surface| surface.id()),
                evidence,
                presentation_timestamp_ns,
            );
            crate::android::diagnostics::host_event(
                "wayland-readiness",
                &format!(
                    "stage=android-frame-presented generation={} surface={:?} evidence={} android_present_ns={:?}",
                    generation,
                    self.kwin_surface.as_ref().map(|surface| surface.id()),
                    evidence,
                    presentation_timestamp_ns,
                ),
            );
            Some(generation)
        } else {
            None
        }
    }

    /// Compatibility helper for callers that only have submit/feedback
    /// evidence and no Android timestamp sample.
    pub fn mark_kwin_frame_presented(&mut self) -> Option<u64> {
        self.mark_kwin_frame_presented_with_evidence("egl-swap-and-wayland-feedback", None)
    }

    fn observe_kwin_surface_id(&mut self, surface: &WlSurface) {
        let toplevel = self
            .xdg_shell_state
            .toplevel_surfaces()
            .iter()
            .find(|candidate| candidate.wl_surface() == surface)
            .cloned();
        if let Some(toplevel) = toplevel {
            self.observe_kwin_toplevel(&toplevel);
        }
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        if let Some(output) = &self.output {
            output.enter(surface.wl_surface());
        }
        let configure_size = self.authoritative_display_state.configure_size();
        surface.with_pending_state(|state| {
            state
                .size
                .replace((configure_size.0, configure_size.1).into());
            state.states.set(xdg_toplevel::State::Activated);
            state.states.set(xdg_toplevel::State::Fullscreen);
            state.states.set(xdg_toplevel::State::Maximized);
        });
        surface.send_configure();
        self.observe_kwin_toplevel(&surface);
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {
        // Handle popup creation here
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // Handle popup grab here
    }

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
        // Handle popup reposition here
    }

    fn ack_configure(&mut self, surface: WlSurface, configure: Configure) {
        // Keep the strict startup state tied to the same titled KWin surface.
        // The xdg-shell delegate has already validated the serial before this
        // callback runs.
        self.observe_kwin_surface_id(&surface);
        if self.is_known_kwin_surface(&surface) {
            if let Some(generation) = self.kwin_generation {
                if self.readiness.mark_configure_acked_for(generation) {
                    let serial = match configure {
                        Configure::Toplevel(configure) => u32::from(configure.serial),
                        Configure::Popup(configure) => u32::from(configure.serial),
                    };
                    log::info!(
                        "wayland.readiness stage=configure-acked generation={} serial={} surface={:?}",
                        generation,
                        serial,
                        surface.id()
                    );
                    crate::android::diagnostics::host_event(
                        "wayland-readiness",
                        &format!(
                            "stage=configure-acked generation={} serial={} surface={:?}",
                            generation,
                            serial,
                            surface.id()
                        ),
                    );
                }
            }
        }
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.observe_kwin_toplevel(&surface);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if self.is_known_kwin_surface(surface.wl_surface()) {
            let generation = self.kwin_generation;
            log::warn!(
                "wayland.readiness kwin_surface_destroyed generation={generation:?} surface={:?}",
                surface.wl_surface().id()
            );
            crate::android::diagnostics::host_event(
                "wayland-readiness",
                &format!(
                    "stage=kwin-disconnected generation={generation:?} surface={:?}",
                    surface.wl_surface().id()
                ),
            );
            self.kwin_surface = None;
            self.kwin_client_id = None;
            self.kwin_generation = None;
            self.readiness.invalidate();
        }
    }
}

impl SelectionHandler for State {
    type SelectionUserData = ClipboardSelectionData;

    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
        if ty != SelectionTarget::Clipboard {
            return;
        }
        let Some(bridge) = self.clipboard_bridge.as_ref() else {
            return;
        };

        match source {
            Some(source) => {
                let mime_types = source.mime_types();
                let Some(mime_type) = crate::core::clipboard_policy::choose_text_mime(
                    mime_types.iter().map(String::as_str),
                )
                .map(str::to_owned) else {
                    log::debug!("Guest clipboard offered no supported text MIME type");
                    return;
                };
                if let Err(error) = bridge.forward_guest_selection(source, mime_type) {
                    log::debug!("Failed to start guest-to-Android clipboard transfer: {error}");
                }
            }
            None => bridge.clear_guest_selection(),
        }
    }

    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        user_data: &ClipboardSelectionData,
    ) {
        if ty != SelectionTarget::Clipboard {
            return;
        }
        let Some(bridge) = self.clipboard_bridge.as_ref() else {
            return;
        };
        bridge.send_selection(user_data, &mime_type, fd);
    }
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {}
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        // `on_commit_buffer_handler` consumes the current BufferAssignment and
        // keeps only the renderer-owned buffer. Capture the protocol event
        // first so readiness cannot miss the only NewBuffer commit.
        let newly_committed = with_states(surface, |states| {
            matches!(
                states
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .current()
                    .buffer
                    .as_ref(),
                Some(BufferAssignment::NewBuffer(_))
            )
        });
        on_commit_buffer_handler::<Self>(surface);
        self.observe_kwin_buffer(surface, newly_committed);
    }
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_image = image;
    }
}

impl PointerConstraintsHandler for State {
    fn new_constraint(&mut self, _surface: &WlSurface, _pointer: &PointerHandle<Self>) {
        // KWin requires the protocol to exist, but pointer locking remains opt-in. Android touch,
        // mouse and keyboard input continue to use the existing absolute-pointer path.
    }

    fn cursor_position_hint(
        &mut self,
        _surface: &WlSurface,
        _pointer: &PointerHandle<Self>,
        _location: smithay::utils::Point<f64, Logical>,
    ) {
    }
}

impl FractionalScaleHandler for State {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let preferred = self.authoritative_display_state.baseline_density_scale();
        smithay::wayland::compositor::with_states(&surface, |states| {
            fractional_scale::with_fractional_scale(states, |scale| {
                scale.set_preferred_scale(preferred);
            });
        });
    }
}

pub fn send_frames_surface_tree(surface: &WlSurface, time: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_surf, states, &()| {
            // the surface may not have any user_data if it is a subsurface and has not
            // yet been commited
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time);
            }
        },
        |_, _, &()| true,
    );
}

#[derive(Default)]
pub struct ClientState {
    compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}

    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

impl OutputHandler for State {}

// Macros used to delegate protocol handling to types in the app state.
delegate_xdg_shell!(State);
delegate_compositor!(State);
delegate_shm!(State);
delegate_seat!(State);
delegate_data_device!(State);
delegate_fractional_scale!(State);
delegate_output!(State);
delegate_pointer_constraints!(State);
delegate_presentation!(State);
delegate_single_pixel_buffer!(State);
delegate_viewporter!(State);

impl smithay::reexports::wayland_server::GlobalDispatch<
    crate::android::backend::wayland::protocol::android_wlegl::server::android_wlegl::AndroidWlegl,
    crate::android::backend::wayland::wlegl::WleglGlobalData,
> for State {
    fn bind(
        _state: &mut Self,
        _handle: &smithay::reexports::wayland_server::DisplayHandle,
        _client: &smithay::reexports::wayland_server::Client,
        resource: smithay::reexports::wayland_server::New<
            crate::android::backend::wayland::protocol::android_wlegl::server::android_wlegl::AndroidWlegl,
        >,
        global_data: &crate::android::backend::wayland::wlegl::WleglGlobalData,
        data_init: &mut smithay::reexports::wayland_server::DataInit<'_, Self>,
    ) {
        data_init.init(resource, global_data.importer);
    }
}

impl smithay::reexports::wayland_server::Dispatch<
    crate::android::backend::wayland::protocol::android_wlegl::server::android_wlegl::AndroidWlegl,
    crate::android::backend::wayland::gl_import::AhbTextureImporter,
> for State {
    fn request(
        _state: &mut Self,
        _client: &smithay::reexports::wayland_server::Client,
        resource: &crate::android::backend::wayland::protocol::android_wlegl::server::android_wlegl::AndroidWlegl,
        request: crate::android::backend::wayland::protocol::android_wlegl::server::android_wlegl::Request,
        importer: &crate::android::backend::wayland::gl_import::AhbTextureImporter,
        _dhandle: &smithay::reexports::wayland_server::DisplayHandle,
        data_init: &mut smithay::reexports::wayland_server::DataInit<'_, Self>,
    ) {
        crate::android::backend::wayland::wlegl::handle_wlegl_request(resource, request, importer, data_init);
    }
}

impl smithay::reexports::wayland_server::Dispatch<
    crate::android::backend::wayland::protocol::android_wlegl::server::android_wlegl_handle::AndroidWleglHandle,
    crate::android::backend::wayland::wlegl::WleglHandleData,
> for State {
    fn request(
        _state: &mut Self,
        _client: &smithay::reexports::wayland_server::Client,
        resource: &crate::android::backend::wayland::protocol::android_wlegl::server::android_wlegl_handle::AndroidWleglHandle,
        request: crate::android::backend::wayland::protocol::android_wlegl::server::android_wlegl_handle::Request,
        data: &crate::android::backend::wayland::wlegl::WleglHandleData,
        _dhandle: &smithay::reexports::wayland_server::DisplayHandle,
        _data_init: &mut smithay::reexports::wayland_server::DataInit<'_, Self>,
    ) {
        crate::android::backend::wayland::wlegl::handle_handle_request(resource, request, data);
    }
}

impl
    smithay::reexports::wayland_server::Dispatch<
        smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
        smithay::backend::renderer::ExternalBufferData,
    > for State
{
    fn request(
        _state: &mut Self,
        _client: &smithay::reexports::wayland_server::Client,
        _resource: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
        _request: smithay::reexports::wayland_server::protocol::wl_buffer::Request,
        _data: &smithay::backend::renderer::ExternalBufferData,
        _dhandle: &smithay::reexports::wayland_server::DisplayHandle,
        _data_init: &mut smithay::reexports::wayland_server::DataInit<'_, Self>,
    ) {
    }
}

pub trait IntoCompositorSize {
    fn into_compositor_size(self) -> Size<i32, Logical>;
}

impl IntoCompositorSize for Size<i32, Logical> {
    fn into_compositor_size(self) -> Size<i32, Logical> {
        self
    }
}

impl IntoCompositorSize for (i32, i32) {
    fn into_compositor_size(self) -> Size<i32, Logical> {
        self.into()
    }
}

impl IntoCompositorSize for (u32, u32) {
    fn into_compositor_size(self) -> Size<i32, Logical> {
        (self.0 as i32, self.1 as i32).into()
    }
}

impl IntoCompositorSize for winit::dpi::PhysicalSize<u32> {
    fn into_compositor_size(self) -> Size<i32, Logical> {
        (self.width as i32, self.height as i32).into()
    }
}

impl IntoCompositorSize for &winit::window::Window {
    fn into_compositor_size(self) -> Size<i32, Logical> {
        self.inner_size().into_compositor_size()
    }
}

impl IntoCompositorSize for smithay::utils::Size<i32, smithay::utils::Physical> {
    fn into_compositor_size(self) -> Size<i32, Logical> {
        (self.w, self.h).into()
    }
}

impl Compositor {
    /// Create a new compositor with physical/logical dimensions.
    ///
    /// Supports `(width, height)`, `winit::dpi::PhysicalSize<u32>` (e.g. from `window.inner_size()`),
    /// `&Window`, or `smithay::utils::Size`.
    pub fn new(
        size: impl IntoCompositorSize,
        guest_scale: f64,
    ) -> Result<Compositor, Box<dyn Error>> {
        let display = Display::new()?;
        let dh = display.handle();
        let size = size.into_compositor_size();
        let scale_val = guest_scale.max(1.0);

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&dh, "Local Desktop");

        let listener = bind_socket()?;
        let clients = Vec::new();

        let start_time = Instant::now();

        // Key repeat rate and delay are in milliseconds: https://wayland-book.com/seat/keyboard.html
        let keyboard = seat
            .add_keyboard(Default::default(), 1000, 200)
            .expect("Failed to add keyboard");
        let touch = seat.add_touch();
        let pointer = seat.add_pointer();

        let mut auth_display_state =
            crate::core::coordinate_transform::AuthoritativeDisplayState::new(
                size.w,
                size.h,
                (scale_val * 160.0).round() as i32,
                60000,
            );
        auth_display_state.update_kwin_scale(scale_val);
        crate::android::backend::wayland::output_state::sync_kwin_output_scale(
            &mut auth_display_state,
        );
        let coordinate_transform = auth_display_state.coordinate_transform();
        let kwin_surface_scale = auth_display_state.presentation_scale();

        let state = State {
            compositor_state: CompositorState::new::<State>(&dh),
            xdg_shell_state: XdgShellState::new::<State>(&dh),
            shm_state: ShmState::new::<State>(&dh, vec![]),
            data_device_state: DataDeviceState::new::<State>(&dh),
            seat_state,
            pointer_constraints_state: PointerConstraintsState::new::<State>(&dh),
            presentation_state: PresentationState::new::<State>(
                &dh,
                smithay::utils::Clock::<smithay::utils::Monotonic>::new().id() as u32,
            ),
            single_pixel_buffer_state: SinglePixelBufferState::new::<State>(&dh),
            viewporter_state: ViewporterState::new::<State>(&dh),
            fractional_scale_state: FractionalScaleManagerState::new::<State>(&dh),
            size,
            output: None,
            cursor_image: CursorImageStatus::default_named(),
            readiness: StartupReadiness::new(),
            kwin_surface: None,
            kwin_client_id: None,
            kwin_generation: None,
            clipboard_bridge: None,
            authoritative_display_state: auth_display_state,
            kwin_surface_scale,
            coordinate_transform,
            ahb_importer: match crate::android::backend::wayland::gl_import::AhbTextureImporter::new(
            ) {
                Ok(imp) => {
                    dh.create_global::<State, crate::android::backend::wayland::protocol::android_wlegl::server::android_wlegl::AndroidWlegl, _>(
                        1,
                        crate::android::backend::wayland::wlegl::WleglGlobalData { importer: imp },
                    );
                    log::info!("Registered android_wlegl global with AHardwareBuffer hardware acceleration support");
                    Some(imp)
                }
                Err(err) => {
                    log::warn!("Failed to initialize AhbTextureImporter: {}", err);
                    None
                }
            },
        };

        Ok(Compositor {
            state,
            listener,
            clients,
            start_time,
            display,
            seat,
            keyboard,
            touch,
            pointer,
            output: None,
            output_global: None,
        })
    }

    /// Legacy constructor defaulting to 1920x1080 for backwards compatibility.
    pub fn build() -> Result<Compositor, Box<dyn Error>> {
        Self::new((1920, 1080), 1.0)
    }

    /// Start clipboard polling after the Android NativeActivity and the nested compositor exist.
    pub fn enable_android_clipboard(&mut self, android_app: AndroidApp) {
        self.state.clipboard_bridge = Some(ClipboardBridge::new(android_app));
    }

    /// Apply completed Android -> Wayland clipboard changes without doing JNI or FD I/O on the
    /// render thread.
    pub fn process_android_clipboard(&mut self) {
        let events = self
            .state
            .clipboard_bridge
            .as_ref()
            .map(ClipboardBridge::drain_events)
            .unwrap_or_default();
        if events.is_empty() {
            return;
        }

        let dh = self.display.handle();
        for event in events {
            match event {
                ClipboardEvent::AndroidChanged(Some(text)) => {
                    if !is_valid_clip_text(&text) {
                        log::warn!("Ignoring empty or oversized Android clipboard selection");
                        continue;
                    }
                    set_data_device_selection::<State>(
                        &dh,
                        &self.seat,
                        vec![TEXT_MIME.to_owned(), UTF8_TEXT_MIME.to_owned()],
                        ClipboardSelectionData::from_text(text),
                    );
                }
                ClipboardEvent::AndroidChanged(None) => {
                    clear_data_device_selection::<State>(&dh, &self.seat);
                }
            }
        }
    }

    /// Keep wl_data_device focus aligned with the full-screen Plasma/KWin client. This is cheap
    /// and idempotent, and also covers the first toplevel appearing after the initial dispatch.
    pub fn sync_data_device_focus(&self) {
        let client = self
            .state
            .xdg_shell_state
            .toplevel_surfaces()
            .iter()
            .next()
            .and_then(|surface| surface.wl_surface().client());
        if client.is_none() {
            return;
        }
        let dh = self.display.handle();
        set_data_device_focus::<State>(&dh, &self.seat, client);
    }

    /// Re-scan all currently known xdg toplevels after dispatch. This catches
    /// the common KWin sequence where the title and the first buffer commit are
    /// delivered in one batch, while still allowing the title_changed and
    /// ack_configure callbacks to establish the identity earlier.
    pub fn observe_kwin_surfaces(&mut self) {
        let surfaces = self.state.xdg_shell_state.toplevel_surfaces().to_vec();
        for surface in surfaces {
            if self.state.observe_kwin_toplevel(&surface) {
                self.state.observe_kwin_buffer(surface.wl_surface(), false);
            }
        }
    }
}
