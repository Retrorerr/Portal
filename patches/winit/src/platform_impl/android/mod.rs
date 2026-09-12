use std::cell::Cell;
use std::collections::VecDeque;
use std::hash::Hash;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use android_activity::input::{
    self, Button, InputEvent, KeyAction, Keycode, MotionAction, Source, ToolType,
};
use android_activity::{
    AndroidApp, AndroidAppWaker, ConfigurationRef, InputStatus, MainEvent, Rect,
};
use tracing::{debug, trace, warn};

use crate::cursor::Cursor;
use crate::dpi::{PhysicalPosition, PhysicalSize, Position, Size};
use crate::error;
use crate::error::EventLoopError;
use crate::event::{self, Event, Force, InnerSizeWriter, MouseButton, StartCause, WindowEvent};
use crate::event_loop::{self, ActiveEventLoop as RootAEL, ControlFlow, DeviceEvents};
use crate::platform::pump_events::PumpStatus;
use crate::platform_impl::Fullscreen;
use crate::window::{
    self, CursorGrabMode, CustomCursor, CustomCursorSource, ImePurpose, ResizeDirection, Theme,
    WindowButtons, WindowLevel,
};

mod keycodes;
mod touchpad;

use touchpad::{GestureAction as TouchpadGestureAction, TouchpadGestureStateMachine};

pub(crate) use crate::cursor::{
    NoCustomCursor as PlatformCustomCursor, NoCustomCursor as PlatformCustomCursorSource,
};
pub(crate) use crate::icon::NoIcon as PlatformIcon;

static HAS_FOCUS: AtomicBool = AtomicBool::new(true);

fn send_mouse_button<T: 'static, F>(
    callback: &mut F,
    target: &RootAEL,
    window_id: window::WindowId,
    device_id: event::DeviceId,
    state: event::ElementState,
    button: MouseButton,
) where
    F: FnMut(Event<T>, &RootAEL),
{
    callback(
        Event::WindowEvent {
            window_id,
            event: WindowEvent::MouseInput { device_id, state, button },
        },
        target,
    );
}

fn send_mouse_wheel<T: 'static, F>(
    callback: &mut F,
    target: &RootAEL,
    window_id: window::WindowId,
    device_id: event::DeviceId,
    x: f64,
    y: f64,
    phase: event::TouchPhase,
) where
    F: FnMut(Event<T>, &RootAEL),
{
    callback(
        Event::WindowEvent {
            window_id,
            event: WindowEvent::MouseWheel {
                device_id,
                delta: event::MouseScrollDelta::PixelDelta(PhysicalPosition { x, y }),
                phase,
            },
        },
        target,
    );
}

/// Which pointer a motion action refers to, decided from the action alone.
///
/// The action pointer index is only meaningful for Down/PointerDown/Up/
/// PointerUp. Every other pointer-class event (HoverEnter/HoverMove/
/// HoverExit, Move, Scroll, ButtonPress/ButtonRelease, captured-pointer
/// motion, ...) must use the first actual pointer instead: under GameActivity
/// the action index is not valid for those actions, and `pointer_at_index`
/// explicitly panics when the index is out of range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointerRule {
    ActionIndex,
    FirstPointer,
}

fn pointer_rule(action: MotionAction) -> PointerRule {
    match action {
        MotionAction::Down
        | MotionAction::PointerDown
        | MotionAction::Up
        | MotionAction::PointerUp => PointerRule::ActionIndex,
        _ => PointerRule::FirstPointer,
    }
}

/// Pure index resolution for [`pointer_rule`]: `None` means the event has no
/// usable pointer — skip pointer-dependent processing instead of panicking.
fn resolve_pointer_index(
    rule: PointerRule,
    action_index: usize,
    pointer_count: usize,
) -> Option<usize> {
    match rule {
        PointerRule::ActionIndex => (action_index < pointer_count).then_some(action_index),
        PointerRule::FirstPointer => (pointer_count > 0).then_some(0),
    }
}

/// Select the pointer a motion event refers to without ever indexing out of
/// range. Returns `None` for zero-pointer events and out-of-range action
/// indices; callers skip pointer-dependent processing in that case instead
/// of fabricating a pointer or panicking.
fn action_pointer<'a>(motion_event: &'a input::MotionEvent<'a>) -> Option<input::Pointer<'a>> {
    let rule = pointer_rule(motion_event.action());
    let index = resolve_pointer_index(
        rule,
        motion_event.pointer_index(),
        motion_event.pointer_count(),
    )?;
    Some(motion_event.pointer_at_index(index))
}

/// Android 14 touchpad gesture axes (per-sample relative deltas, display
/// px). GameActivity only copies explicitly enabled axes into the native
/// event, so these are enabled once at event-loop init (see below); X/Y are
/// on by default.
const GESTURE_X_AXIS: u32 = 50;
const GESTURE_Y_AXIS: u32 = 51;

/// The exact gesture-axis set Portal requires from GameActivity. Single
/// source for both enablement and coverage: enabling anything else is a
/// conscious decision, not drift.
const TOUCHPAD_GESTURE_AXIS_IDS: [u32; 2] = [GESTURE_X_AXIS, GESTURE_Y_AXIS];

/// Pure fold for gesture samples: historical samples oldest-first, then the
/// current sample. Separated from `Pointer::history()` for unit coverage.
fn collect_gesture_samples<I>(history: I, current: (f64, f64)) -> Vec<(f64, f64)>
where
    I: Iterator<Item = (f64, f64)>,
{
    let (lower, upper) = history.size_hint();
    let mut samples = Vec::with_capacity(upper.unwrap_or(lower) + 1);
    samples.extend(history);
    samples.push(current);
    samples
}

/// Android 14+ gesture axes are per-sample relative deltas. Preserve every
/// batched historical sample, oldest first, then the current sample.
///
/// Backend-independent: `Pointer::history()` serves both NativeActivity and
/// GameActivity (the patched android-activity carries 53 axes, so 50/51 are
/// readable on both; no NDK cast anywhere).
fn touchpad_gesture_samples(pointer: &input::Pointer<'_>) -> Vec<(f64, f64)> {
    let x_axis = input::Axis::from(GESTURE_X_AXIS);
    let y_axis = input::Axis::from(GESTURE_Y_AXIS);
    let history = pointer.history().map(|historical| {
        (
            historical.axis_value(x_axis) as f64,
            historical.axis_value(y_axis) as f64,
        )
    });
    let current = (
        pointer.axis_value(x_axis) as f64,
        pointer.axis_value(y_axis) as f64,
    );
    let samples = collect_gesture_samples(history, current);
    // Physical-proof logging for two-finger-scroll validation: opt-in via
    // the `gesture-axis-debug-log` feature (plus debug builds). Capped,
    // nonzero-only, never in normal debug/release usage.
    #[cfg(all(debug_assertions, feature = "gesture-axis-debug-log"))]
    {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static LOGGED_GESTURE_SAMPLES: AtomicUsize = AtomicUsize::new(0);
        if samples.iter().any(|(x, y)| *x != 0.0 || *y != 0.0)
            && LOGGED_GESTURE_SAMPLES.fetch_add(1, Ordering::Relaxed) < 25
        {
            tracing::info!("touchpad gesture axes 50/51 live samples: {samples:?}");
        }
    }
    samples
}

#[cfg(test)]
mod pointer_selection_tests {
    use super::{
        collect_gesture_samples, pointer_rule, resolve_pointer_index, PointerRule,
        TOUCHPAD_GESTURE_AXIS_IDS,
    };
    use android_activity::input::MotionAction;

    #[test]
    fn hover_move_selects_first_pointer() {
        assert_eq!(pointer_rule(MotionAction::HoverMove), PointerRule::FirstPointer);
        assert_eq!(
            resolve_pointer_index(PointerRule::FirstPointer, usize::MAX, 1),
            Some(0)
        );
    }

    #[test]
    fn scroll_selects_first_pointer() {
        assert_eq!(pointer_rule(MotionAction::Scroll), PointerRule::FirstPointer);
        assert_eq!(
            resolve_pointer_index(PointerRule::FirstPointer, usize::MAX, 1),
            Some(0)
        );
    }

    #[test]
    fn button_press_selects_first_pointer() {
        assert_eq!(
            pointer_rule(MotionAction::ButtonPress),
            PointerRule::FirstPointer
        );
        assert_eq!(
            resolve_pointer_index(PointerRule::FirstPointer, usize::MAX, 1),
            Some(0)
        );
    }

    #[test]
    fn down_uses_valid_action_index() {
        assert_eq!(pointer_rule(MotionAction::Down), PointerRule::ActionIndex);
        assert_eq!(resolve_pointer_index(PointerRule::ActionIndex, 0, 1), Some(0));
    }

    #[test]
    fn pointer_down_uses_valid_nonzero_index() {
        assert_eq!(
            pointer_rule(MotionAction::PointerDown),
            PointerRule::ActionIndex
        );
        assert_eq!(resolve_pointer_index(PointerRule::ActionIndex, 1, 2), Some(1));
    }

    #[test]
    fn out_of_range_action_index_selects_nothing() {
        assert_eq!(pointer_rule(MotionAction::Up), PointerRule::ActionIndex);
        assert_eq!(resolve_pointer_index(PointerRule::ActionIndex, 3, 2), None);
    }

    #[test]
    fn zero_pointer_event_selects_nothing() {
        assert_eq!(pointer_rule(MotionAction::HoverMove), PointerRule::FirstPointer);
        assert_eq!(resolve_pointer_index(PointerRule::FirstPointer, 0, 0), None);
        assert_eq!(pointer_rule(MotionAction::Down), PointerRule::ActionIndex);
        assert_eq!(resolve_pointer_index(PointerRule::ActionIndex, 0, 0), None);
    }

    #[test]
    fn game_activity_enables_exactly_gesture_axes_50_51() {
        // The init path iterates this exact set: no more, no fewer. Axis
        // 48/49/52 stay disabled until Portal needs them.
        assert_eq!(TOUCHPAD_GESTURE_AXIS_IDS, [50, 51]);
    }

    #[test]
    fn gesture_samples_order_oldest_history_then_current() {
        let history = vec![(1.0, -1.0), (2.0, -2.5)].into_iter();
        assert_eq!(
            collect_gesture_samples(history, (3.0, -3.0)),
            vec![(1.0, -1.0), (2.0, -2.5), (3.0, -3.0)]
        );
    }

    #[test]
    fn gesture_samples_empty_history_yields_current_only() {
        let history = Vec::new().into_iter();
        assert_eq!(collect_gesture_samples(history, (0.5, 0.0)), vec![(0.5, 0.0)]);
    }
}

/// Returns the minimum `Option<Duration>`, taking into account that `None`
/// equates to an infinite timeout, not a zero timeout (so can't just use
/// `Option::min`)
fn min_timeout(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    a.map_or(b, |a_timeout| b.map_or(Some(a_timeout), |b_timeout| Some(a_timeout.min(b_timeout))))
}

struct PeekableReceiver<T> {
    recv: mpsc::Receiver<T>,
    first: Option<T>,
}

impl<T> PeekableReceiver<T> {
    pub fn from_recv(recv: mpsc::Receiver<T>) -> Self {
        Self { recv, first: None }
    }

    pub fn has_incoming(&mut self) -> bool {
        if self.first.is_some() {
            return true;
        }
        match self.recv.try_recv() {
            Ok(v) => {
                self.first = Some(v);
                true
            },
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => {
                warn!("Channel was disconnected when checking incoming");
                false
            },
        }
    }

    pub fn try_recv(&mut self) -> Result<T, mpsc::TryRecvError> {
        if let Some(first) = self.first.take() {
            return Ok(first);
        }
        self.recv.try_recv()
    }
}

#[derive(Clone)]
struct SharedFlagSetter {
    flag: Arc<AtomicBool>,
}
impl SharedFlagSetter {
    pub fn set(&self) -> bool {
        self.flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed).is_ok()
    }
}

struct SharedFlag {
    flag: Arc<AtomicBool>,
}

// Used for queuing redraws from arbitrary threads. We don't care how many
// times a redraw is requested (so don't actually need to queue any data,
// we just need to know at the start of a main loop iteration if a redraw
// was queued and be able to read and clear the state atomically)
impl SharedFlag {
    pub fn new() -> Self {
        Self { flag: Arc::new(AtomicBool::new(false)) }
    }

    pub fn setter(&self) -> SharedFlagSetter {
        SharedFlagSetter { flag: self.flag.clone() }
    }

    pub fn get_and_reset(&self) -> bool {
        self.flag.swap(false, std::sync::atomic::Ordering::AcqRel)
    }
}

#[derive(Clone)]
pub struct RedrawRequester {
    flag: SharedFlagSetter,
    waker: AndroidAppWaker,
}

impl RedrawRequester {
    fn new(flag: &SharedFlag, waker: AndroidAppWaker) -> Self {
        RedrawRequester { flag: flag.setter(), waker }
    }

    pub fn request_redraw(&self) {
        if self.flag.set() {
            // Only explicitly try to wake up the main loop when the flag
            // value changes
            self.waker.wake();
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct KeyEventExtra {
    pub meta_state: u32,
}

pub struct EventLoop<T: 'static> {
    pub(crate) android_app: AndroidApp,
    window_target: event_loop::ActiveEventLoop,
    redraw_flag: SharedFlag,
    user_events_sender: mpsc::Sender<T>,
    user_events_receiver: PeekableReceiver<T>, // must wake looper whenever something gets sent
    loop_running: bool,                        // Dispatched `NewEvents<Init>`
    running: bool,
    pending_redraw: bool,
    cause: StartCause,
    ignore_volume_keys: bool,
    combining_accent: Option<char>,
    pressed_mouse_buttons: std::collections::HashSet<MouseButton>,
    touchpad_gestures: TouchpadGestureStateMachine,
    touchpad_clock: Instant,
    last_touchpad_device_id: Option<event::DeviceId>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlatformSpecificEventLoopAttributes {
    pub(crate) android_app: Option<AndroidApp>,
    pub(crate) ignore_volume_keys: bool,
}

impl Default for PlatformSpecificEventLoopAttributes {
    fn default() -> Self {
        Self { android_app: Default::default(), ignore_volume_keys: true }
    }
}

impl<T: 'static> EventLoop<T> {
    pub(crate) fn new(
        attributes: &PlatformSpecificEventLoopAttributes,
    ) -> Result<Self, EventLoopError> {
        let (user_events_sender, user_events_receiver) = mpsc::channel();

        let android_app = attributes.android_app.as_ref().expect(
            "An `AndroidApp` as passed to android_main() is required to create an `EventLoop` on \
             Android",
        );
        let density = android_app.config().density().map(|dpi| dpi as f64 / 160.0).unwrap_or(1.0);
        let redraw_flag = SharedFlag::new();

        // Portal touchpad scrolling: GameActivity only copies explicitly
        // enabled axes into the native event, so enable the Android 14
        // gesture axes once here. NativeActivity carries the full event
        // regardless (its enable call is a no-op) and is cfg-excluded so its
        // behaviour is byte-for-byte unaffected.
        #[cfg(not(feature = "android-native-activity"))]
        for axis_id in TOUCHPAD_GESTURE_AXIS_IDS {
            android_app.enable_motion_axis(input::Axis::from(axis_id));
        }

        Ok(Self {
            android_app: android_app.clone(),
            window_target: event_loop::ActiveEventLoop {
                p: ActiveEventLoop {
                    app: android_app.clone(),
                    control_flow: Cell::new(ControlFlow::default()),
                    exit: Cell::new(false),
                    redraw_requester: RedrawRequester::new(
                        &redraw_flag,
                        android_app.create_waker(),
                    ),
                },
                _marker: PhantomData,
            },
            redraw_flag,
            user_events_sender,
            user_events_receiver: PeekableReceiver::from_recv(user_events_receiver),
            loop_running: false,
            running: false,
            pending_redraw: false,
            cause: StartCause::Init,
            ignore_volume_keys: attributes.ignore_volume_keys,
            combining_accent: None,
            pressed_mouse_buttons: std::collections::HashSet::new(),
            touchpad_gestures: TouchpadGestureStateMachine::with_density(density),
            touchpad_clock: Instant::now(),
            last_touchpad_device_id: None,
        })
    }

    fn cancel_touchpad_input<F>(&mut self, callback: &mut F)
    where
        F: FnMut(Event<T>, &RootAEL),
    {
        let Some(device_id) = self.last_touchpad_device_id else {
            self.touchpad_gestures.cancel();
            self.pressed_mouse_buttons.clear();
            return;
        };
        let window_id = window::WindowId(WindowId);
        if self.touchpad_gestures.end_scroll() {
            send_mouse_wheel(
                callback,
                self.window_target(),
                window_id,
                device_id,
                0.0,
                0.0,
                event::TouchPhase::Cancelled,
            );
        }
        if self.touchpad_gestures.cancel() == Some(TouchpadGestureAction::DragEnd) {
            send_mouse_button(
                callback,
                self.window_target(),
                window_id,
                device_id,
                event::ElementState::Released,
                MouseButton::Left,
            );
        }
        let released: Vec<MouseButton> = self.pressed_mouse_buttons.drain().collect();
        for button in released {
            send_mouse_button(
                callback,
                self.window_target(),
                window_id,
                device_id,
                event::ElementState::Released,
                button,
            );
        }
    }

    fn single_iteration<F>(&mut self, main_event: Option<MainEvent<'_>>, callback: &mut F)
    where
        F: FnMut(Event<T>, &RootAEL),
    {
        trace!("Mainloop iteration");

        let cause = self.cause;
        let mut pending_redraw = self.pending_redraw;
        let mut resized = false;

        callback(Event::NewEvents(cause), self.window_target());

        if let Some(event) = main_event {
            trace!("Handling main event {:?}", event);

            match event {
                MainEvent::InitWindow { .. } => {
                    callback(Event::Resumed, self.window_target());
                },
                MainEvent::TerminateWindow { .. } => {
                    self.cancel_touchpad_input(callback);
                    callback(Event::Suspended, self.window_target());
                },
                MainEvent::WindowResized { .. } => resized = true,
                MainEvent::RedrawNeeded { .. } => pending_redraw = true,
                MainEvent::ContentRectChanged { .. } => {
                    warn!("TODO: find a way to notify application of content rect change");
                },
                MainEvent::GainedFocus => {
                    HAS_FOCUS.store(true, Ordering::Relaxed);
                    callback(
                        Event::WindowEvent {
                            window_id: window::WindowId(WindowId),
                            event: WindowEvent::Focused(true),
                        },
                        self.window_target(),
                    );
                },
                MainEvent::LostFocus => {
                    self.cancel_touchpad_input(callback);
                    HAS_FOCUS.store(false, Ordering::Relaxed);
                    callback(
                        Event::WindowEvent {
                            window_id: window::WindowId(WindowId),
                            event: WindowEvent::Focused(false),
                        },
                        self.window_target(),
                    );
                },
                MainEvent::ConfigChanged { .. } => {
                    let monitor = MonitorHandle::new(self.android_app.clone());
                    let old_scale_factor = monitor.scale_factor();
                    let scale_factor = monitor.scale_factor();
                    if (scale_factor - old_scale_factor).abs() < f64::EPSILON {
                        let new_inner_size = Arc::new(Mutex::new(
                            MonitorHandle::new(self.android_app.clone()).size(),
                        ));
                        let event = Event::WindowEvent {
                            window_id: window::WindowId(WindowId),
                            event: WindowEvent::ScaleFactorChanged {
                                inner_size_writer: InnerSizeWriter::new(Arc::downgrade(
                                    &new_inner_size,
                                )),
                                scale_factor,
                            },
                        };
                        callback(event, self.window_target());
                    }
                },
                MainEvent::LowMemory => {
                    callback(Event::MemoryWarning, self.window_target());
                },
                MainEvent::Start => {
                    // XXX: how to forward this state to applications?
                    warn!("TODO: forward onStart notification to application");
                },
                MainEvent::Resume { .. } => {
                    debug!("App Resumed - is running");
                    self.running = true;
                },
                MainEvent::SaveState { .. } => {
                    // XXX: how to forward this state to applications?
                    // XXX: also how do we expose state restoration to apps?
                    warn!("TODO: forward saveState notification to application");
                },
                MainEvent::Pause => {
                    self.cancel_touchpad_input(callback);
                    debug!("App Paused - stopped running");
                    self.running = false;
                },
                MainEvent::Stop => {
                    // XXX: how to forward this state to applications?
                    warn!("TODO: forward onStop notification to application");
                },
                MainEvent::Destroy => {
                    // XXX: maybe exit mainloop to drop things before being
                    // killed by the OS?
                    warn!("TODO: forward onDestroy notification to application");
                },
                MainEvent::InsetsChanged { .. } => {
                    // XXX: how to forward this state to applications?
                    warn!("TODO: handle Android InsetsChanged notification");
                },
                unknown => {
                    trace!("Unknown MainEvent {unknown:?} (ignored)");
                },
            }
        } else {
            trace!("No main event to handle");
        }

        // temporarily decouple `android_app` from `self` so we aren't holding
        // a borrow of `self` while iterating
        let android_app = self.android_app.clone();

        // Process input events
        match android_app.input_events_iter() {
            Ok(mut input_iter) => loop {
                let read_event =
                    input_iter.next(|event| self.handle_input_event(&android_app, event, callback));

                if !read_event {
                    break;
                }
            },
            Err(err) => {
                tracing::warn!("Failed to get input events iterator: {err:?}");
            },
        }

        // Empty the user event buffer
        {
            while let Ok(event) = self.user_events_receiver.try_recv() {
                callback(crate::event::Event::UserEvent(event), self.window_target());
            }
        }

        if self.running {
            if resized {
                let size = if let Some(native_window) = self.android_app.native_window().as_ref() {
                    let width = native_window.width() as _;
                    let height = native_window.height() as _;
                    PhysicalSize::new(width, height)
                } else {
                    PhysicalSize::new(0, 0)
                };
                let event = Event::WindowEvent {
                    window_id: window::WindowId(WindowId),
                    event: WindowEvent::Resized(size),
                };
                callback(event, self.window_target());
            }

            pending_redraw |= self.redraw_flag.get_and_reset();
            if pending_redraw {
                pending_redraw = false;
                let event = Event::WindowEvent {
                    window_id: window::WindowId(WindowId),
                    event: WindowEvent::RedrawRequested,
                };
                callback(event, self.window_target());
            }
        }

        // This is always the last event we dispatch before poll again
        callback(Event::AboutToWait, self.window_target());

        self.pending_redraw = pending_redraw;
    }

    fn handle_input_event<F>(
        &mut self,
        android_app: &AndroidApp,
        event: &InputEvent<'_>,
        callback: &mut F,
    ) -> InputStatus
    where
        F: FnMut(Event<T>, &RootAEL),
    {
        let mut input_status = InputStatus::Handled;
        match event {
            InputEvent::MotionEvent(motion_event) => {
                let action = motion_event.action();
                let source = motion_event.source();
                // The action pointer index is only valid for Down/PointerDown/
                // Up/PointerUp (and even then must be range-checked); every
                // other pointer-class event uses the first actual pointer.
                // Zero-pointer events carry nothing to classify or forward:
                // consume them quietly instead of panicking.
                let Some(pointer) = action_pointer(motion_event) else {
                    return input_status;
                };

                let tool_type = pointer.tool_type();
                // On Samsung Dex, `tool_type()` still reports `Finger` when using built-in trackpad
                // So we also check for `source()`, as it correctly reports `Mouse` (although other devices such as Desktop AVDs report `Unknown``)

                if tool_type != ToolType::Finger
                    || source == Source::Mouse
                    || source == Source::Touchpad
                {
                    let window_id = window::WindowId(WindowId);
                    let device_id = event::DeviceId(DeviceId(motion_event.device_id()));

                    let is_touchpad = source == Source::Touchpad
                        || (source == Source::Mouse && tool_type == ToolType::Finger);
                    let is_mouse = source == Source::Mouse && tool_type != ToolType::Finger;
                    if is_touchpad {
                        self.last_touchpad_device_id = Some(device_id);
                    }

                    // Android 14+ gesture axes 50/51 are per-sample relative
                    // deltas. Preserve every batched historical sample, oldest
                    // first, then the current sample.
                    let gesture_samples = is_touchpad
                        .then(|| touchpad_gesture_samples(&pointer))
                        .unwrap_or_default();
                    let has_gesture_scroll =
                        gesture_samples.iter().any(|(x, y)| *x != 0.0 || *y != 0.0);

                    let gesture_time = self.touchpad_clock.elapsed();
                    let button_state = motion_event.button_state();

                    // 1. Continuous Touchpad Scrolling
                    if is_touchpad && has_gesture_scroll {
                        let was_scrolling = self.touchpad_gestures.is_scrolling();
                        if self.touchpad_gestures.scroll() == Some(TouchpadGestureAction::DragEnd) {
                            send_mouse_button(
                                callback,
                                self.window_target(),
                                window_id,
                                device_id,
                                event::ElementState::Released,
                                MouseButton::Left,
                            );
                        }
                        let mut first = !was_scrolling;
                        for (gesture_dx, gesture_dy) in gesture_samples {
                            if gesture_dx == 0.0 && gesture_dy == 0.0 {
                                continue;
                            }
                            send_mouse_wheel(
                                callback,
                                self.window_target(),
                                window_id,
                                device_id,
                                gesture_dx,
                                gesture_dy,
                                if first {
                                    event::TouchPhase::Started
                                } else {
                                    event::TouchPhase::Moved
                                },
                            );
                            first = false;
                        }
                    } else if action == MotionAction::Scroll {
                        if is_touchpad {
                            let h = pointer.axis_value(input::Axis::Hscroll) as f64 * 20.0;
                            let v = pointer.axis_value(input::Axis::Vscroll) as f64 * 20.0;
                            if h != 0.0 || v != 0.0 {
                                let was_scrolling = self.touchpad_gestures.is_scrolling();
                                if self.touchpad_gestures.scroll()
                                    == Some(TouchpadGestureAction::DragEnd)
                                {
                                    send_mouse_button(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        event::ElementState::Released,
                                        MouseButton::Left,
                                    );
                                }
                                send_mouse_wheel(
                                    callback,
                                    self.window_target(),
                                    window_id,
                                    device_id,
                                    h,
                                    v,
                                    if was_scrolling {
                                        event::TouchPhase::Moved
                                    } else {
                                        event::TouchPhase::Started
                                    },
                                );
                            }
                        } else if is_mouse {
                            let h = pointer.axis_value(input::Axis::Hscroll);
                            let v = pointer.axis_value(input::Axis::Vscroll);
                            if h != 0.0 || v != 0.0 {
                                callback(
                                    Event::WindowEvent {
                                        window_id,
                                        event: WindowEvent::MouseWheel {
                                            device_id,
                                            delta: event::MouseScrollDelta::LineDelta(
                                                h as f32, v as f32,
                                            ),
                                            phase: event::TouchPhase::Moved,
                                        },
                                    },
                                    self.window_target(),
                                );
                            }
                        }
                    }

                    // 2. Mouse / Touchpad Pointer Movement
                    if (action == MotionAction::HoverMove || action == MotionAction::Move)
                        && !has_gesture_scroll
                    {
                        if is_touchpad && self.touchpad_gestures.end_scroll() {
                            send_mouse_wheel(
                                callback,
                                self.window_target(),
                                window_id,
                                device_id,
                                0.0,
                                0.0,
                                event::TouchPhase::Ended,
                            );
                        }
                        let location =
                            PhysicalPosition { x: pointer.x() as _, y: pointer.y() as _ };
                        if is_touchpad
                            && self.touchpad_gestures.movement(
                                gesture_time,
                                location.x,
                                location.y,
                                button_state.primary(),
                            ) == Some(TouchpadGestureAction::DragStart)
                        {
                            send_mouse_button(
                                callback,
                                self.window_target(),
                                window_id,
                                device_id,
                                event::ElementState::Pressed,
                                MouseButton::Left,
                            );
                        }
                        callback(
                            Event::WindowEvent {
                                window_id,
                                event: WindowEvent::AndroidPointerMoved {
                                    device_id,
                                    android_device_id: motion_event.device_id(),
                                    source: source.into(),
                                    tool_type: tool_type.into(),
                                    position: location,
                                },
                            },
                            self.window_target(),
                        );
                    }

                    // 3. Mouse / Touchpad Buttons. Real ButtonPress/ButtonRelease actions are
                    // forwarded directly. A bare touchpad Down only arms tap recognition.
                    let mapped_button = match action {
                        MotionAction::ButtonPress | MotionAction::ButtonRelease => {
                            match motion_event.action_button() {
                                Button::Secondary | Button::StylusSecondary => MouseButton::Right,
                                Button::Tertiary => MouseButton::Middle,
                                Button::Back => MouseButton::Back,
                                Button::Forward => MouseButton::Forward,
                                _ => MouseButton::Left,
                            }
                        },
                        _ if button_state.secondary() => MouseButton::Right,
                        _ if button_state.teriary() => MouseButton::Middle,
                        _ if button_state.back() => MouseButton::Back,
                        _ if button_state.forward() => MouseButton::Forward,
                        _ if button_state.stylus_secondary() => MouseButton::Right,
                        _ => MouseButton::Left,
                    };

                    if is_touchpad {
                        match action {
                            MotionAction::ButtonPress => {
                                let forward_press = if mapped_button == MouseButton::Left {
                                    self.touchpad_gestures.physical_button_press()
                                } else {
                                    if self.touchpad_gestures.cancel()
                                        == Some(TouchpadGestureAction::DragEnd)
                                    {
                                        send_mouse_button(
                                            callback,
                                            self.window_target(),
                                            window_id,
                                            device_id,
                                            event::ElementState::Released,
                                            MouseButton::Left,
                                        );
                                    }
                                    true
                                };
                                if forward_press && self.pressed_mouse_buttons.insert(mapped_button)
                                {
                                    send_mouse_button(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        event::ElementState::Pressed,
                                        mapped_button,
                                    );
                                }
                            },
                            MotionAction::ButtonRelease => {
                                let forward_release = mapped_button != MouseButton::Left
                                    || self.touchpad_gestures.physical_button_release();
                                if forward_release
                                    && self.pressed_mouse_buttons.remove(&mapped_button)
                                {
                                    send_mouse_button(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        event::ElementState::Released,
                                        mapped_button,
                                    );
                                }
                            },
                            MotionAction::Down => {
                                let location = PhysicalPosition {
                                    x: pointer.x() as f64,
                                    y: pointer.y() as f64,
                                };
                                let has_non_primary_button = self
                                    .pressed_mouse_buttons
                                    .iter()
                                    .any(|button| *button != MouseButton::Left);
                                if !has_non_primary_button {
                                    // A new contact ends an orphaned synthetic drag first: the
                                    // previous contact died without a terminal event, so the
                                    // fresh contact arms normally and cannot inherit the
                                    // orphan's tap (ownership clearing drops its tap context).
                                    if self.touchpad_gestures.finish_touchpad_drag_if_owned()
                                        == Some(TouchpadGestureAction::DragEnd)
                                    {
                                        send_mouse_button(
                                            callback,
                                            self.window_target(),
                                            window_id,
                                            device_id,
                                            event::ElementState::Released,
                                            MouseButton::Left,
                                        );
                                    }
                                    self.touchpad_gestures.down(
                                        gesture_time,
                                        location.x,
                                        location.y,
                                    );
                                } else if self.touchpad_gestures.cancel()
                                    == Some(TouchpadGestureAction::DragEnd)
                                {
                                    send_mouse_button(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        event::ElementState::Released,
                                        MouseButton::Left,
                                    );
                                }
                            },
                            MotionAction::Up => {
                                if self.touchpad_gestures.end_scroll() {
                                    send_mouse_wheel(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        0.0,
                                        0.0,
                                        event::TouchPhase::Ended,
                                    );
                                }
                                let location = PhysicalPosition {
                                    x: pointer.x() as f64,
                                    y: pointer.y() as f64,
                                };
                                match self.touchpad_gestures.up(
                                    gesture_time,
                                    location.x,
                                    location.y,
                                ) {
                                    Some(TouchpadGestureAction::Click) => {
                                        send_mouse_button(
                                            callback,
                                            self.window_target(),
                                            window_id,
                                            device_id,
                                            event::ElementState::Pressed,
                                            MouseButton::Left,
                                        );
                                        send_mouse_button(
                                            callback,
                                            self.window_target(),
                                            window_id,
                                            device_id,
                                            event::ElementState::Released,
                                            MouseButton::Left,
                                        );
                                    },
                                    Some(TouchpadGestureAction::DragEnd) => send_mouse_button(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        event::ElementState::Released,
                                        MouseButton::Left,
                                    ),
                                    Some(TouchpadGestureAction::DragStart) | None => {},
                                }

                                // Some Android touchpad stacks end a primary click with Up
                                // instead of a separate ButtonRelease. Treat Up as a release
                                // fallback so physical or adopted synthetic grabs cannot stick.
                                let released: Vec<MouseButton> =
                                    self.pressed_mouse_buttons.drain().collect();
                                for button in released {
                                    send_mouse_button(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        event::ElementState::Released,
                                        button,
                                    );
                                }
                            },
                            MotionAction::PointerDown
                            | MotionAction::PointerUp
                            | MotionAction::Cancel => {
                                if self.touchpad_gestures.end_scroll() {
                                    send_mouse_wheel(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        0.0,
                                        0.0,
                                        if action == MotionAction::Cancel {
                                            event::TouchPhase::Cancelled
                                        } else {
                                            event::TouchPhase::Ended
                                        },
                                    );
                                }
                                if self.touchpad_gestures.cancel()
                                    == Some(TouchpadGestureAction::DragEnd)
                                {
                                    send_mouse_button(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        event::ElementState::Released,
                                        MouseButton::Left,
                                    );
                                }
                            },
                            MotionAction::HoverExit => {
                                // Abnormal contact end (finger left the pad with no ACTION_UP):
                                // finish an owned synthetic drag exactly once. Scroll ends
                                // first so a concurrent scroll still terminates cleanly.
                                if self.touchpad_gestures.end_scroll() {
                                    send_mouse_wheel(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        0.0,
                                        0.0,
                                        event::TouchPhase::Ended,
                                    );
                                }
                                if self.touchpad_gestures.hover_exit()
                                    == Some(TouchpadGestureAction::DragEnd)
                                {
                                    send_mouse_button(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        event::ElementState::Released,
                                        MouseButton::Left,
                                    );
                                }
                            },
                            MotionAction::HoverEnter => {
                                // A fresh hover cursor while Portal still owns a synthetic
                                // drag proves the drag contact died silently (no terminal
                                // event arrived): reconcile the orphan exactly once. All
                                // other states are left untouched.
                                if self.touchpad_gestures.hover_enter()
                                    == Some(TouchpadGestureAction::DragEnd)
                                {
                                    send_mouse_button(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        event::ElementState::Released,
                                        MouseButton::Left,
                                    );
                                }
                            },
                            _ => {},
                        }
                    } else if is_mouse {
                        match action {
                            MotionAction::ButtonPress => {
                                if self.pressed_mouse_buttons.insert(mapped_button) {
                                    send_mouse_button(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        event::ElementState::Pressed,
                                        mapped_button,
                                    );
                                }
                            },
                            MotionAction::ButtonRelease => {
                                if self.pressed_mouse_buttons.remove(&mapped_button) {
                                    send_mouse_button(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        event::ElementState::Released,
                                        mapped_button,
                                    );
                                }
                            },
                            MotionAction::Down => {
                                if self.pressed_mouse_buttons.is_empty() {
                                    self.pressed_mouse_buttons.insert(mapped_button);
                                    send_mouse_button(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        event::ElementState::Pressed,
                                        mapped_button,
                                    );
                                }
                            },
                            MotionAction::Up => {
                                let released: Vec<MouseButton> =
                                    self.pressed_mouse_buttons.drain().collect();
                                if released.is_empty() {
                                    send_mouse_button(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        event::ElementState::Released,
                                        mapped_button,
                                    );
                                } else {
                                    for btn in released {
                                        send_mouse_button(
                                            callback,
                                            self.window_target(),
                                            window_id,
                                            device_id,
                                            event::ElementState::Released,
                                            btn,
                                        );
                                    }
                                }
                            },
                            MotionAction::Cancel => {
                                let cancelled: Vec<MouseButton> =
                                    self.pressed_mouse_buttons.drain().collect();
                                for btn in cancelled {
                                    send_mouse_button(
                                        callback,
                                        self.window_target(),
                                        window_id,
                                        device_id,
                                        event::ElementState::Released,
                                        btn,
                                    );
                                }
                            },
                            _ => {},
                        }
                    }
                } else {
                    // Treat them as touch events
                    let window_id = window::WindowId(WindowId);
                    let device_id = event::DeviceId(DeviceId(motion_event.device_id()));

                    let phase = match action {
                        MotionAction::Down | MotionAction::PointerDown => {
                            Some(event::TouchPhase::Started)
                        },
                        MotionAction::Up | MotionAction::PointerUp => {
                            Some(event::TouchPhase::Ended)
                        },
                        MotionAction::Move => Some(event::TouchPhase::Moved),
                        MotionAction::Cancel => Some(event::TouchPhase::Cancelled),
                        _ => None,
                    };
                    if let Some(phase) = phase {
                        let pointers: Box<dyn Iterator<Item = input::Pointer<'_>>> = match phase {
                            event::TouchPhase::Started | event::TouchPhase::Ended => {
                                // Validated action pointer (Down/PointerDown/
                                // Up/PointerUp); empty only if the event
                                // somehow carries no pointer, which the
                                // entry guard above already excluded.
                                Box::new(action_pointer(motion_event).into_iter())
                            },
                            event::TouchPhase::Moved | event::TouchPhase::Cancelled => {
                                Box::new(motion_event.pointers())
                            },
                        };

                        for pointer in pointers {
                            let location =
                                PhysicalPosition { x: pointer.x() as _, y: pointer.y() as _ };
                            trace!(
                                "Input event {device_id:?}, {phase:?}, loc={location:?}, \
                             pointer={pointer:?}"
                            );
                            let event = Event::WindowEvent {
                                window_id,
                                event: WindowEvent::Touch(event::Touch {
                                    device_id,
                                    phase,
                                    location,
                                    id: pointer.pointer_id() as u64,
                                    force: Some(Force::Normalized(pointer.pressure() as f64)),
                                }),
                            };
                            callback(event, self.window_target());
                        }
                    }
                }
            },
            InputEvent::KeyEvent(key) => {
                match key.key_code() {
                    // Flag keys related to volume as unhandled. While winit does not have a way for
                    // applications to configure what keys to flag as handled,
                    // this appears to be a good default until winit
                    // can be configured.
                    Keycode::VolumeUp | Keycode::VolumeDown | Keycode::VolumeMute
                        if self.ignore_volume_keys =>
                    {
                        input_status = InputStatus::Unhandled
                    },
                    keycode => {
                        let state = match key.action() {
                            KeyAction::Down => event::ElementState::Pressed,
                            KeyAction::Up => event::ElementState::Released,
                            _ => event::ElementState::Released,
                        };

                        let key_char = keycodes::character_map_and_combine_key(
                            android_app,
                            key,
                            &mut self.combining_accent,
                        );

                        let event = Event::WindowEvent {
                            window_id: window::WindowId(WindowId),
                            event: WindowEvent::KeyboardInput {
                                device_id: event::DeviceId(DeviceId(key.device_id())),
                                event: event::KeyEvent {
                                    state,
                                    physical_key: keycodes::to_physical_key(keycode),
                                    logical_key: keycodes::to_logical(key_char, keycode),
                                    location: keycodes::to_location(keycode),
                                    repeat: key.repeat_count() > 0,
                                    text: None,
                                    platform_specific: KeyEventExtra {
                                        meta_state: key.meta_state().0,
                                    },
                                },
                                is_synthetic: false,
                            },
                        };
                        callback(event, self.window_target());
                    },
                }
            },
            _ => {
                warn!("Unknown android_activity input event {event:?}")
            },
        }

        input_status
    }

    pub fn run<F>(mut self, event_handler: F) -> Result<(), EventLoopError>
    where
        F: FnMut(Event<T>, &event_loop::ActiveEventLoop),
    {
        self.run_on_demand(event_handler)
    }

    pub fn run_on_demand<F>(&mut self, mut event_handler: F) -> Result<(), EventLoopError>
    where
        F: FnMut(Event<T>, &event_loop::ActiveEventLoop),
    {
        loop {
            match self.pump_events(None, &mut event_handler) {
                PumpStatus::Exit(0) => {
                    break Ok(());
                },
                PumpStatus::Exit(code) => {
                    break Err(EventLoopError::ExitFailure(code));
                },
                _ => {
                    continue;
                },
            }
        }
    }

    pub fn pump_events<F>(&mut self, timeout: Option<Duration>, mut callback: F) -> PumpStatus
    where
        F: FnMut(Event<T>, &RootAEL),
    {
        if !self.loop_running {
            self.loop_running = true;

            // Reset the internal state for the loop as we start running to
            // ensure consistent behaviour in case the loop runs and exits more
            // than once
            self.pending_redraw = false;
            self.cause = StartCause::Init;

            // run the initial loop iteration
            self.single_iteration(None, &mut callback);
        }

        // Consider the possibility that the `StartCause::Init` iteration could
        // request to Exit
        if !self.exiting() {
            self.poll_events_with_timeout(timeout, &mut callback);
        }
        if self.exiting() {
            self.loop_running = false;

            callback(Event::LoopExiting, self.window_target());

            PumpStatus::Exit(0)
        } else {
            PumpStatus::Continue
        }
    }

    fn poll_events_with_timeout<F>(&mut self, mut timeout: Option<Duration>, mut callback: F)
    where
        F: FnMut(Event<T>, &RootAEL),
    {
        let start = Instant::now();

        self.pending_redraw |= self.redraw_flag.get_and_reset();

        timeout =
            if self.running && (self.pending_redraw || self.user_events_receiver.has_incoming()) {
                // If we already have work to do then we don't want to block on the next poll
                Some(Duration::ZERO)
            } else {
                let control_flow_timeout = match self.control_flow() {
                    ControlFlow::Wait => None,
                    ControlFlow::Poll => Some(Duration::ZERO),
                    ControlFlow::WaitUntil(wait_deadline) => {
                        Some(wait_deadline.saturating_duration_since(start))
                    },
                };

                min_timeout(control_flow_timeout, timeout)
            };

        let app = self.android_app.clone(); // Don't borrow self as part of poll expression
        app.poll_events(timeout, |poll_event| {
            let mut main_event = None;

            match poll_event {
                android_activity::PollEvent::Wake => {
                    // In the X11 backend it's noted that too many false-positive wake ups
                    // would cause the event loop to run continuously. They handle this by
                    // re-checking for pending events (assuming they cover all
                    // valid reasons for a wake up).
                    //
                    // For now, user_events and redraw_requests are the only reasons to expect
                    // a wake up here so we can ignore the wake up if there are no events/requests.
                    // We also ignore wake ups while suspended.
                    self.pending_redraw |= self.redraw_flag.get_and_reset();
                    if !self.running
                        || (!self.pending_redraw && !self.user_events_receiver.has_incoming())
                    {
                        return;
                    }
                },
                android_activity::PollEvent::Timeout => {},
                android_activity::PollEvent::Main(event) => {
                    main_event = Some(event);
                },
                unknown_event => {
                    warn!("Unknown poll event {unknown_event:?} (ignored)");
                },
            }

            self.cause = match self.control_flow() {
                ControlFlow::Poll => StartCause::Poll,
                ControlFlow::Wait => StartCause::WaitCancelled { start, requested_resume: None },
                ControlFlow::WaitUntil(deadline) => {
                    if Instant::now() < deadline {
                        StartCause::WaitCancelled { start, requested_resume: Some(deadline) }
                    } else {
                        StartCause::ResumeTimeReached { start, requested_resume: deadline }
                    }
                },
            };

            self.single_iteration(main_event, &mut callback);
        });
    }

    pub fn window_target(&self) -> &event_loop::ActiveEventLoop {
        &self.window_target
    }

    pub fn create_proxy(&self) -> EventLoopProxy<T> {
        EventLoopProxy {
            user_events_sender: self.user_events_sender.clone(),
            waker: self.android_app.create_waker(),
        }
    }

    fn control_flow(&self) -> ControlFlow {
        self.window_target.p.control_flow()
    }

    fn exiting(&self) -> bool {
        self.window_target.p.exiting()
    }
}

pub struct EventLoopProxy<T: 'static> {
    user_events_sender: mpsc::Sender<T>,
    waker: AndroidAppWaker,
}

impl<T: 'static> Clone for EventLoopProxy<T> {
    fn clone(&self) -> Self {
        EventLoopProxy {
            user_events_sender: self.user_events_sender.clone(),
            waker: self.waker.clone(),
        }
    }
}

impl<T> EventLoopProxy<T> {
    pub fn send_event(&self, event: T) -> Result<(), event_loop::EventLoopClosed<T>> {
        self.user_events_sender.send(event).map_err(|err| event_loop::EventLoopClosed(err.0))?;
        self.waker.wake();
        Ok(())
    }
}

pub struct ActiveEventLoop {
    pub(crate) app: AndroidApp,
    control_flow: Cell<ControlFlow>,
    exit: Cell<bool>,
    redraw_requester: RedrawRequester,
}

impl ActiveEventLoop {
    pub fn primary_monitor(&self) -> Option<MonitorHandle> {
        Some(MonitorHandle::new(self.app.clone()))
    }

    pub fn create_custom_cursor(&self, source: CustomCursorSource) -> CustomCursor {
        let _ = source.inner;
        CustomCursor { inner: PlatformCustomCursor }
    }

    pub fn available_monitors(&self) -> VecDeque<MonitorHandle> {
        let mut v = VecDeque::with_capacity(1);
        v.push_back(MonitorHandle::new(self.app.clone()));
        v
    }

    #[inline]
    pub fn listen_device_events(&self, _allowed: DeviceEvents) {}

    #[cfg(feature = "rwh_05")]
    #[inline]
    pub fn raw_display_handle_rwh_05(&self) -> rwh_05::RawDisplayHandle {
        rwh_05::RawDisplayHandle::Android(rwh_05::AndroidDisplayHandle::empty())
    }

    #[inline]
    pub fn system_theme(&self) -> Option<Theme> {
        None
    }

    #[cfg(feature = "rwh_06")]
    #[inline]
    pub fn raw_display_handle_rwh_06(
        &self,
    ) -> Result<rwh_06::RawDisplayHandle, rwh_06::HandleError> {
        Ok(rwh_06::RawDisplayHandle::Android(rwh_06::AndroidDisplayHandle::new()))
    }

    pub(crate) fn set_control_flow(&self, control_flow: ControlFlow) {
        self.control_flow.set(control_flow)
    }

    pub(crate) fn control_flow(&self) -> ControlFlow {
        self.control_flow.get()
    }

    pub(crate) fn exit(&self) {
        self.exit.set(true)
    }

    pub(crate) fn clear_exit(&self) {
        self.exit.set(false)
    }

    pub(crate) fn exiting(&self) -> bool {
        self.exit.get()
    }

    pub(crate) fn owned_display_handle(&self) -> OwnedDisplayHandle {
        OwnedDisplayHandle
    }
}

#[derive(Clone)]
pub(crate) struct OwnedDisplayHandle;

impl OwnedDisplayHandle {
    #[cfg(feature = "rwh_05")]
    #[inline]
    pub fn raw_display_handle_rwh_05(&self) -> rwh_05::RawDisplayHandle {
        rwh_05::AndroidDisplayHandle::empty().into()
    }

    #[cfg(feature = "rwh_06")]
    #[inline]
    pub fn raw_display_handle_rwh_06(
        &self,
    ) -> Result<rwh_06::RawDisplayHandle, rwh_06::HandleError> {
        Ok(rwh_06::AndroidDisplayHandle::new().into())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct WindowId;

impl WindowId {
    pub const fn dummy() -> Self {
        WindowId
    }
}

impl From<WindowId> for u64 {
    fn from(_: WindowId) -> Self {
        0
    }
}

impl From<u64> for WindowId {
    fn from(_: u64) -> Self {
        Self
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DeviceId(i32);

impl DeviceId {
    pub const fn dummy() -> Self {
        DeviceId(0)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PlatformSpecificWindowAttributes;

pub(crate) struct Window {
    app: AndroidApp,
    redraw_requester: RedrawRequester,
}

impl Window {
    pub(crate) fn new(
        el: &ActiveEventLoop,
        _window_attrs: window::WindowAttributes,
    ) -> Result<Self, error::OsError> {
        // FIXME this ignores requested window attributes

        Ok(Self { app: el.app.clone(), redraw_requester: el.redraw_requester.clone() })
    }

    pub(crate) fn maybe_queue_on_main(&self, f: impl FnOnce(&Self) + Send + 'static) {
        f(self)
    }

    pub(crate) fn maybe_wait_on_main<R: Send>(&self, f: impl FnOnce(&Self) -> R + Send) -> R {
        f(self)
    }

    pub fn id(&self) -> WindowId {
        WindowId
    }

    pub fn primary_monitor(&self) -> Option<MonitorHandle> {
        Some(MonitorHandle::new(self.app.clone()))
    }

    pub fn available_monitors(&self) -> VecDeque<MonitorHandle> {
        let mut v = VecDeque::with_capacity(1);
        v.push_back(MonitorHandle::new(self.app.clone()));
        v
    }

    pub fn current_monitor(&self) -> Option<MonitorHandle> {
        Some(MonitorHandle::new(self.app.clone()))
    }

    pub fn scale_factor(&self) -> f64 {
        MonitorHandle::new(self.app.clone()).scale_factor()
    }

    pub fn request_redraw(&self) {
        self.redraw_requester.request_redraw()
    }

    pub fn pre_present_notify(&self) {}

    pub fn inner_position(&self) -> Result<PhysicalPosition<i32>, error::NotSupportedError> {
        Err(error::NotSupportedError::new())
    }

    pub fn outer_position(&self) -> Result<PhysicalPosition<i32>, error::NotSupportedError> {
        Err(error::NotSupportedError::new())
    }

    pub fn set_outer_position(&self, _position: Position) {
        // no effect
    }

    pub fn inner_size(&self) -> PhysicalSize<u32> {
        self.outer_size()
    }

    pub fn request_inner_size(&self, _size: Size) -> Option<PhysicalSize<u32>> {
        Some(self.inner_size())
    }

    pub fn outer_size(&self) -> PhysicalSize<u32> {
        MonitorHandle::new(self.app.clone()).size()
    }

    pub fn set_min_inner_size(&self, _: Option<Size>) {}

    pub fn set_max_inner_size(&self, _: Option<Size>) {}

    pub fn resize_increments(&self) -> Option<PhysicalSize<u32>> {
        None
    }

    pub fn set_resize_increments(&self, _increments: Option<Size>) {}

    pub fn set_title(&self, _title: &str) {}

    pub fn set_transparent(&self, _transparent: bool) {}

    pub fn set_blur(&self, _blur: bool) {}

    pub fn set_visible(&self, _visibility: bool) {}

    pub fn is_visible(&self) -> Option<bool> {
        None
    }

    pub fn set_resizable(&self, _resizeable: bool) {}

    pub fn is_resizable(&self) -> bool {
        false
    }

    pub fn set_enabled_buttons(&self, _buttons: WindowButtons) {}

    pub fn enabled_buttons(&self) -> WindowButtons {
        WindowButtons::all()
    }

    pub fn set_minimized(&self, _minimized: bool) {}

    pub fn is_minimized(&self) -> Option<bool> {
        None
    }

    pub fn set_maximized(&self, _maximized: bool) {}

    pub fn is_maximized(&self) -> bool {
        false
    }

    pub fn set_fullscreen(&self, _monitor: Option<Fullscreen>) {
        warn!("Cannot set fullscreen on Android");
    }

    pub fn fullscreen(&self) -> Option<Fullscreen> {
        None
    }

    pub fn set_decorations(&self, _decorations: bool) {}

    pub fn is_decorated(&self) -> bool {
        true
    }

    pub fn set_window_level(&self, _level: WindowLevel) {}

    pub fn set_window_icon(&self, _window_icon: Option<crate::icon::Icon>) {}

    pub fn set_ime_cursor_area(&self, _position: Position, _size: Size) {}

    pub fn set_ime_allowed(&self, allowed: bool) {
        if allowed {
            self.app.show_soft_input(true);
        } else {
            self.app.hide_soft_input(true);
        }
    }

    pub fn set_ime_purpose(&self, _purpose: ImePurpose) {}

    pub fn focus_window(&self) {}

    pub fn request_user_attention(&self, _request_type: Option<window::UserAttentionType>) {}

    pub fn set_cursor(&self, _: Cursor) {}

    pub fn set_cursor_position(&self, _: Position) -> Result<(), error::ExternalError> {
        Err(error::ExternalError::NotSupported(error::NotSupportedError::new()))
    }

    pub fn set_cursor_grab(&self, _: CursorGrabMode) -> Result<(), error::ExternalError> {
        Err(error::ExternalError::NotSupported(error::NotSupportedError::new()))
    }

    pub fn set_cursor_visible(&self, _: bool) {}

    pub fn drag_window(&self) -> Result<(), error::ExternalError> {
        Err(error::ExternalError::NotSupported(error::NotSupportedError::new()))
    }

    pub fn drag_resize_window(
        &self,
        _direction: ResizeDirection,
    ) -> Result<(), error::ExternalError> {
        Err(error::ExternalError::NotSupported(error::NotSupportedError::new()))
    }

    #[inline]
    pub fn show_window_menu(&self, _position: Position) {}

    pub fn set_cursor_hittest(&self, _hittest: bool) -> Result<(), error::ExternalError> {
        Err(error::ExternalError::NotSupported(error::NotSupportedError::new()))
    }

    #[cfg(feature = "rwh_04")]
    pub fn raw_window_handle_rwh_04(&self) -> rwh_04::RawWindowHandle {
        use rwh_04::HasRawWindowHandle;

        if let Some(native_window) = self.app.native_window().as_ref() {
            native_window.raw_window_handle()
        } else {
            panic!(
                "Cannot get the native window, it's null and will always be null before \
                 Event::Resumed and after Event::Suspended. Make sure you only call this function \
                 between those events."
            );
        }
    }

    #[cfg(feature = "rwh_05")]
    pub fn raw_window_handle_rwh_05(&self) -> rwh_05::RawWindowHandle {
        use rwh_05::HasRawWindowHandle;

        if let Some(native_window) = self.app.native_window().as_ref() {
            native_window.raw_window_handle()
        } else {
            panic!(
                "Cannot get the native window, it's null and will always be null before \
                 Event::Resumed and after Event::Suspended. Make sure you only call this function \
                 between those events."
            );
        }
    }

    #[cfg(feature = "rwh_05")]
    pub fn raw_display_handle_rwh_05(&self) -> rwh_05::RawDisplayHandle {
        rwh_05::RawDisplayHandle::Android(rwh_05::AndroidDisplayHandle::empty())
    }

    #[cfg(feature = "rwh_06")]
    // Allow the usage of HasRawWindowHandle inside this function
    #[allow(deprecated)]
    pub fn raw_window_handle_rwh_06(&self) -> Result<rwh_06::RawWindowHandle, rwh_06::HandleError> {
        use rwh_06::HasRawWindowHandle;

        if let Some(native_window) = self.app.native_window().as_ref() {
            native_window.raw_window_handle()
        } else {
            tracing::error!(
                "Cannot get the native window, it's null and will always be null before \
                 Event::Resumed and after Event::Suspended. Make sure you only call this function \
                 between those events."
            );
            Err(rwh_06::HandleError::Unavailable)
        }
    }

    #[cfg(feature = "rwh_06")]
    pub fn raw_display_handle_rwh_06(
        &self,
    ) -> Result<rwh_06::RawDisplayHandle, rwh_06::HandleError> {
        Ok(rwh_06::RawDisplayHandle::Android(rwh_06::AndroidDisplayHandle::new()))
    }

    pub fn config(&self) -> ConfigurationRef {
        self.app.config()
    }

    pub fn content_rect(&self) -> Rect {
        self.app.content_rect()
    }

    pub fn set_theme(&self, _theme: Option<Theme>) {}

    pub fn theme(&self) -> Option<Theme> {
        None
    }

    pub fn set_content_protected(&self, _protected: bool) {}

    pub fn has_focus(&self) -> bool {
        HAS_FOCUS.load(Ordering::Relaxed)
    }

    pub fn title(&self) -> String {
        String::new()
    }

    pub fn reset_dead_keys(&self) {}
}

#[derive(Default, Clone, Debug)]
pub struct OsError;

use std::fmt::{self, Display, Formatter};
impl Display for OsError {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        write!(fmt, "Android OS Error")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MonitorHandle {
    app: AndroidApp,
}
impl PartialOrd for MonitorHandle {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for MonitorHandle {
    fn cmp(&self, _other: &Self) -> std::cmp::Ordering {
        std::cmp::Ordering::Equal
    }
}

impl MonitorHandle {
    pub(crate) fn new(app: AndroidApp) -> Self {
        Self { app }
    }

    pub fn name(&self) -> Option<String> {
        Some("Android Device".to_owned())
    }

    pub fn size(&self) -> PhysicalSize<u32> {
        if let Some(native_window) = self.app.native_window() {
            PhysicalSize::new(native_window.width() as _, native_window.height() as _)
        } else {
            PhysicalSize::new(0, 0)
        }
    }

    pub fn position(&self) -> PhysicalPosition<i32> {
        (0, 0).into()
    }

    pub fn scale_factor(&self) -> f64 {
        self.app.config().density().map(|dpi| dpi as f64 / 160.0).unwrap_or(1.0)
    }

    pub fn refresh_rate_millihertz(&self) -> Option<u32> {
        // FIXME no way to get real refresh rate for now.
        None
    }

    pub fn video_modes(&self) -> impl Iterator<Item = VideoModeHandle> {
        let size = self.size().into();
        // FIXME this is not the real refresh rate
        // (it is guaranteed to support 32 bit color though)
        std::iter::once(VideoModeHandle {
            size,
            bit_depth: 32,
            refresh_rate_millihertz: 60000,
            monitor: self.clone(),
        })
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct VideoModeHandle {
    size: (u32, u32),
    bit_depth: u16,
    refresh_rate_millihertz: u32,
    monitor: MonitorHandle,
}

impl VideoModeHandle {
    pub fn size(&self) -> PhysicalSize<u32> {
        self.size.into()
    }

    pub fn bit_depth(&self) -> u16 {
        self.bit_depth
    }

    pub fn refresh_rate_millihertz(&self) -> u32 {
        self.refresh_rate_millihertz
    }

    pub fn monitor(&self) -> MonitorHandle {
        self.monitor.clone()
    }
}
