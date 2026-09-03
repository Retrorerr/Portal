pub mod bind;
mod compositor;
mod event_centralizer;
mod event_handler;
pub mod gl_import;
mod input;
mod keymap;
mod output_state;
pub mod protocol;
mod winit_backend;
pub mod wlegl;


pub use output_state::write_guest_output_state;

pub use compositor::{Compositor, State};
pub use event_centralizer::{centralize, centralize_injected_keyboard, CentralizedEvent};
pub use event_handler::handle;
pub use winit_backend::{
    bind, AndroidFrameTimestampSample, AndroidFrameTimestampSupport, WinitGraphicsBackend,
};

use smithay::{
    backend::renderer::gles::GlesRenderer,
    utils::{Clock, Monotonic},
};
use std::collections::HashMap;
use winit::dpi::PhysicalPosition;
use winit::platform::android::activity::AndroidApp;

/// What the fingers currently on screen are doing, following Android's gesture conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchMode {
    /// Still within touch slop and the long-press timeout: could become anything.
    Undecided,
    /// Moved past touch slop before the long press fired.
    Scroll,
    /// Long-press timeout elapsed without moving; no button sent yet.
    LongPress,
    /// Moved after a long press: left button held down.
    Drag,
}

pub struct WaylandBackend {
    pub compositor: Compositor,
    pub graphic_renderer: Option<WinitGraphicsBackend<GlesRenderer>>,
    pub android_app: AndroidApp,
    pub clock: Clock<Monotonic>,
    pub key_counter: u32,
    pub guest_scale_factor: f64,
    /// Active touch points keyed by pointer id.
    pub touch_points: HashMap<u64, PhysicalPosition<f64>>,
    /// Centroid of the active touch points at the last scroll update.
    pub scroll_centroid: Option<PhysicalPosition<f64>>,
    /// What the current gesture has been resolved to.
    pub touch_mode: TouchMode,
    /// Location where the gesture's first finger landed.
    pub touch_down_position: Option<PhysicalPosition<f64>>,
    /// When that finger landed, in `clock` milliseconds.
    pub touch_down_time: Option<u64>,
    /// `ViewConfiguration.getScaledTouchSlop()`.
    pub touch_slop_px: f64,
    /// `ViewConfiguration.getLongPressTimeout()`.
    pub long_press_timeout_ms: u64,
    /// Whether a synthesized button press is currently held (an in-progress drag).
    pub pointer_pressed: bool,
    /// Monotonic sequence sent with wp_presentation feedback.
    pub presentation_sequence: u64,
    /// An EGL frame that contained the identified KWin surface and its
    /// presentation-feedback request, awaiting Android's physical display
    /// timestamp. The frame id prevents a later recovery or KWin generation
    /// from satisfying this attempt.
    pub pending_kwin_presentation: Option<PendingKwinPresentation>,
    /// Android display refresh rate in Wayland mode units (millihertz).
    pub refresh_rate_millihz: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingKwinPresentation {
    pub generation: u64,
    pub egl_frame_id: u64,
}

impl WaylandBackend {
    /// Forget the in-flight gesture. Callers holding a pressed button must release it first.
    pub fn reset_touch_state(&mut self) {
        self.touch_points.clear();
        self.scroll_centroid = None;
        self.touch_mode = TouchMode::Undecided;
        self.touch_down_position = None;
        self.touch_down_time = None;
    }

    /// Release any synthesized pointer grab and clear pending presentation state on suspend.
    pub fn suspend_input_and_presentation(&mut self) {
        self.reset_touch_state();
        if self.pointer_pressed {
            let time = self.clock.now().as_millis() as u32;
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            self.compositor.pointer.button(
                &mut self.compositor.state,
                &smithay::input::pointer::ButtonEvent {
                    button: 0x110, // BTN_LEFT
                    state: smithay::backend::input::ButtonState::Released,
                    serial,
                    time,
                },
            );
            self.compositor.pointer.frame(&mut self.compositor.state);
            self.pointer_pressed = false;
        }
        self.pending_kwin_presentation = None;
        self.key_counter = 0;
    }
}
