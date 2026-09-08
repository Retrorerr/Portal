pub mod bind;
mod compositor;
mod event_centralizer;
mod event_handler;
pub mod gl_import;
mod input;
mod keymap;
pub mod output_state;
pub mod protocol;
pub mod socket_watcher;
pub(crate) mod surface_control_cursor;
mod text_input_v2;
mod winit_backend;
pub mod wlegl;

pub use output_state::{read_kwin_output_scale, sync_kwin_output_scale, write_guest_output_state};

pub use compositor::{Compositor, State};
pub use event_centralizer::{centralize, centralize_injected_keyboard, CentralizedEvent};
pub use event_handler::{dispatch_wayland, handle, log_presentation_state};
pub use socket_watcher::WaylandSocketWatcher;
pub use winit_backend::{
    bind, AndroidFrameTimestampSample, AndroidFrameTimestampSupport, WinitGraphicsBackend,
};

use smithay::{
    backend::renderer::{damage::OutputDamageTracker, gles::GlesRenderer},
    utils::{Clock, Monotonic},
};
use std::collections::{HashMap, HashSet};
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
    /// Whether the current touchscreen gesture has emitted its first scroll frame.
    pub touch_scroll_started: bool,
    /// What the current gesture has been resolved to.
    pub touch_mode: TouchMode,
    /// Location where the gesture's first finger landed.
    pub touch_down_position: Option<PhysicalPosition<f64>>,
    /// When that finger landed, in `clock` milliseconds.
    pub touch_down_time: Option<u64>,
    /// Resize generation at `touch_down_position` time. Guards long-press
    /// anchoring against host resizes during the press (stale physical must
    /// not remap through a newer viewport).
    pub touch_down_generation: Option<u64>,
    /// `ViewConfiguration.getScaledTouchSlop()`.
    pub touch_slop_px: f64,
    /// `ViewConfiguration.getLongPressTimeout()`.
    pub long_press_timeout_ms: u64,
    /// Whether a synthesized button press is currently held (an in-progress drag).
    pub pointer_pressed: bool,
    /// Active high-resolution Wayland axes, tracked separately by semantic source.
    pub finger_scroll_axes: ScrollAxisState,
    pub continuous_scroll_axes: ScrollAxisState,
    /// Monotonic sequence sent with wp_presentation feedback.
    pub presentation_sequence: u64,
    /// An EGL frame that contained the identified KWin surface and its
    /// presentation-feedback request, awaiting Android's physical display
    /// timestamp. The frame id prevents a later recovery or KWin generation
    /// from satisfying this attempt.
    pub pending_kwin_presentation: Option<PendingKwinPresentation>,
    /// Stable nominal Android display refresh in Wayland mode units (millihertz).
    /// Resolved once from `Display.getSupportedModes()` (preferred target;
    /// 144 Hz on the OnePlus Pad 3, otherwise the device maximum):
    /// advertised as the `wl_output` mode and used for presentation-feedback
    /// `Refresh`. Never mirrors transient VRR scanout changes.
    pub refresh_rate_millihz: i32,
    /// Last observed instantaneous Android physical/VRR scanout rate
    /// (millihertz). Diagnostics and pacing only; never advertised via
    /// `wl_output`.
    pub physical_refresh_millihz: i32,
    /// Last `clock` millisecond a host refresh-rate sample ran. The render
    /// loop consumes the cached nominal rate; `Display.getMode()` is queried
    /// at low frequency only (never per-frame) to track the instantaneous
    /// physical/VRR rate for diagnostics. Physical changes never rewrite
    /// `wl_output` and never recreate the output.
    pub last_refresh_poll_ms: Option<u64>,
    /// Whether the preferred `ANativeWindow` hint has been issued for the current
    /// native window. Reset on suspend (window destroyed); re-issued on resume.
    pub frame_rate_requested: bool,
    pub content_cadence: crate::core::content_cadence::ContentCadence,
    pub supported_refresh_millihz: Vec<i32>,
    /// Currently pressed evdev physical scancodes.
    pub pressed_keys: HashSet<u32>,
    /// Guest-side mouse button policy: presses in letterbox borders are
    /// suppressed, releases for forwarded presses always land (no stuck
    /// buttons across resizes/drags).
    pub button_tracker: crate::core::pointer_buttons::PointerButtonTracker,
    /// Touch ids whose down landed in a border and must never affect the
    /// guest (a border press sliding inside still must not click).
    pub suppressed_touch_ids: HashSet<u64>,
    /// Last `clock` millisecond a Plasma-scale refresh ran. The render loop
    /// consumes cached scale; the filesystem is stat'ed at low frequency
    /// only, never per-frame or per-commit.
    pub last_plasma_poll_ms: Option<u64>,
    /// Rendered-frame ownership gate: `note_kwin_commit()` runs only for a
    /// genuinely new KWin frame (first sighting or changed commit counter for
    /// the identified surface). A configure ACK alone must never reattribute
    /// the currently rendered buffer to a newer request.
    pub kwin_commit_gate:
        crate::core::presentation::KwinCommitGate<smithay::backend::renderer::utils::CommitCounter>,
    /// Background socket watcher that unblocks the event loop on Wayland socket traffic.
    pub socket_watcher: Option<WaylandSocketWatcher>,
    /// Whether the output needs a redraw (commits received, input motion, resize, etc.).
    pub output_dirty: bool,
    /// Tracks final Android-target damage across KWin/cursor/overlay render
    /// elements. The SHM importer independently tracks texture upload damage;
    /// this tracker carries the resulting scene damage through GLES and EGL.
    pub output_damage_tracker: Option<OutputDamageTracker>,
    /// `(width, height, scale_bits)` used to create `output_damage_tracker`.
    /// A host resize or presentation-scale change invalidates buffer history
    /// and forces a fresh full frame before partial redraws resume.
    pub output_damage_signature: Option<(i32, i32, u64)>,
    /// Coalesces event-driven host redraws onto Android display callbacks.
    pub frame_pacer: Option<crate::android::utils::frame_pacing::AndroidFramePacer>,
    /// Preferred API-33+ Choreographer timeline for the next submitted frame.
    /// Consumed exactly once so a stale vsync target never leaks into a later
    /// event-driven redraw.
    pub frame_timeline: Option<crate::android::utils::frame_pacing::AndroidFrameTimeline>,
    pub frame_timeline_stats: crate::android::utils::frame_pacing::FrameTimelineStats,
    /// Android 16 cursor-only layer. The desktop remains on EGL; `None` is the
    /// fully compatible GLES cursor fallback.
    pub surface_control_cursor: Option<surface_control_cursor::SurfaceControlCursor>,
    /// Whether a frame is currently in flight to the Android compositor.
    pub frame_in_flight: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingKwinPresentation {
    pub generation: u64,
    pub egl_frame_id: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScrollAxisState {
    pub horizontal: bool,
    pub vertical: bool,
}

impl WaylandBackend {
    /// Schedule an already-dirty frame at Android's next display callback.
    /// Devices without the NDK Choreographer API keep the immediate path.
    pub fn schedule_redraw(&self) {
        let Some(winit) = self.graphic_renderer.as_ref() else {
            return;
        };
        if self
            .frame_pacer
            .as_ref()
            .is_some_and(|pacer| pacer.request_redraw())
        {
            return;
        }
        winit.window().request_redraw();
    }

    /// Stop only axes that actually received high-resolution values for this
    /// semantic source. Wayland axis_stop has no value payload; lifecycle is
    /// therefore tracked explicitly instead of inferred from zero deltas.
    pub fn stop_scroll_source(
        &mut self,
        source: smithay::backend::input::AxisSource,
        time: u32,
    ) -> bool {
        use smithay::backend::input::{Axis, AxisSource};
        let active = match source {
            AxisSource::Finger => std::mem::take(&mut self.finger_scroll_axes),
            AxisSource::Continuous => std::mem::take(&mut self.continuous_scroll_axes),
            _ => return false,
        };
        if !active.horizontal && !active.vertical {
            return false;
        }
        let mut frame = smithay::input::pointer::AxisFrame::new(time).source(source);
        if active.horizontal {
            frame = frame.stop(Axis::Horizontal);
        }
        if active.vertical {
            frame = frame.stop(Axis::Vertical);
        }
        let pointer = self.compositor.pointer.clone();
        pointer.axis(&mut self.compositor.state, frame);
        pointer.frame(&mut self.compositor.state);
        true
    }

    pub fn stop_all_scrolls(&mut self, time: u32) {
        use smithay::backend::input::AxisSource;
        self.stop_scroll_source(AxisSource::Finger, time);
        self.stop_scroll_source(AxisSource::Continuous, time);
    }

    /// Forget the in-flight gesture. Callers holding a pressed button must release it first.
    /// Note: `suppressed_touch_ids` is deliberately preserved — `touch_points`
    /// only tracks non-suppressed fingers, so clearing the suppressed set here
    /// would un-suppress a border-started finger that is still down when the
    /// last in-guest finger lifts. Suppressed ids are removed individually on
    /// their own lift/cancel (or all at once on suspend).
    pub fn reset_touch_state(&mut self) {
        self.touch_points.clear();
        self.scroll_centroid = None;
        self.touch_scroll_started = false;
        self.touch_mode = TouchMode::Undecided;
        self.touch_down_position = None;
        self.touch_down_time = None;
        self.touch_down_generation = None;
    }

    /// Release any synthesized pointer grab and clear pending presentation state on suspend.
    pub fn suspend_input_and_presentation(&mut self) {
        self.reset_touch_state();
        // All fingers are gone on suspend: drop any border-suppressed ids too
        // (reset_touch_state preserves them for the partial-lift case above).
        self.suppressed_touch_ids.clear();
        let time = self.clock.now().as_millis() as u32;
        self.stop_all_scrolls(time);
        if self.pointer_pressed {
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
        // Release every mouse button the guest believes is held (not just
        // BTN_LEFT) so none can stick across suspend while the tracker resets.
        let held_buttons = self.button_tracker.drain_pressed();
        for button in held_buttons.iter().copied() {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            self.compositor.pointer.button(
                &mut self.compositor.state,
                &smithay::input::pointer::ButtonEvent {
                    button,
                    state: smithay::backend::input::ButtonState::Released,
                    serial,
                    time,
                },
            );
        }
        if !held_buttons.is_empty() {
            self.compositor.pointer.frame(&mut self.compositor.state);
        }
        for scancode in self.pressed_keys.drain() {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            self.compositor.keyboard.input::<(), _>(
                &mut self.compositor.state,
                scancode.into(),
                smithay::backend::input::KeyState::Released,
                serial,
                time,
                |_, _, _| smithay::input::keyboard::FilterResult::Forward,
            );
        }
        self.pending_kwin_presentation = None;
        self.key_counter = 0;
    }
}
