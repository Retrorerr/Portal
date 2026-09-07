use std::time::Duration;

const DEFAULT_DOUBLE_TAP_TIMEOUT: Duration = Duration::from_millis(300);
// Android's ViewConfiguration defaults, expressed in density-independent pixels.
const DEFAULT_DOUBLE_TAP_SLOP_DP: f64 = 100.0;
const DEFAULT_DRAG_SLOP_DP: f64 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Point {
    x: f64,
    y: f64,
}

impl Point {
    fn distance_squared(self, other: Self) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx * dx + dy * dy
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Tap {
    ended_at: Duration,
    position: Point,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum GestureState {
    Idle,
    TapCandidate {
        down_at: Duration,
        position: Point,
        second_tap: bool,
        button_seen: bool,
        physical_button_active: bool,
    },
    Moving,
    Scrolling,
    Dragging,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GestureAction {
    Click,
    DragStart,
    DragEnd,
}

/// Resolves Android touchpad motion actions without conflating contact with a button press.
/// Physical buttons remain outside this machine and are forwarded exactly as reported.
pub(crate) struct TouchpadGestureStateMachine {
    state: GestureState,
    last_tap: Option<Tap>,
    double_tap_timeout: Duration,
    double_tap_slop_squared: f64,
    drag_slop_squared: f64,
}

impl Default for TouchpadGestureStateMachine {
    fn default() -> Self {
        Self::with_density(1.0)
    }
}

impl TouchpadGestureStateMachine {
    pub(crate) fn with_density(density: f64) -> Self {
        let density = density.max(1.0);
        let double_tap_slop = DEFAULT_DOUBLE_TAP_SLOP_DP * density;
        let drag_slop = DEFAULT_DRAG_SLOP_DP * density;
        Self {
            state: GestureState::Idle,
            last_tap: None,
            double_tap_timeout: DEFAULT_DOUBLE_TAP_TIMEOUT,
            double_tap_slop_squared: double_tap_slop * double_tap_slop,
            drag_slop_squared: drag_slop * drag_slop,
        }
    }

    pub(crate) fn down(&mut self, now: Duration, x: f64, y: f64) {
        self.down_with_physical_button(now, x, y, false);
    }

    pub(crate) fn down_with_physical_button(
        &mut self,
        now: Duration,
        x: f64,
        y: f64,
        physical_button_active: bool,
    ) {
        let position = Point { x, y };
        let second_tap = self.last_tap.is_some_and(|tap| {
            now.saturating_sub(tap.ended_at) <= self.double_tap_timeout
                && position.distance_squared(tap.position) <= self.double_tap_slop_squared
        });
        if !second_tap {
            self.last_tap = None;
        }
        self.state = GestureState::TapCandidate {
            down_at: now,
            position,
            second_tap,
            button_seen: physical_button_active,
            physical_button_active,
        };
    }

    pub(crate) fn movement(&mut self, x: f64, y: f64) -> Option<GestureAction> {
        let GestureState::TapCandidate { position, second_tap, physical_button_active, .. } =
            self.state
        else {
            return None;
        };
        if (Point { x, y }).distance_squared(position) <= self.drag_slop_squared {
            return None;
        }

        self.last_tap = None;
        if second_tap && !physical_button_active {
            self.state = GestureState::Dragging;
            Some(GestureAction::DragStart)
        } else {
            self.state = GestureState::Moving;
            None
        }
    }

    pub(crate) fn scroll(&mut self) -> Option<GestureAction> {
        let action = (self.state == GestureState::Dragging).then_some(GestureAction::DragEnd);
        self.state = GestureState::Scrolling;
        self.last_tap = None;
        action
    }

    pub(crate) fn end_scroll(&mut self) -> bool {
        if self.state != GestureState::Scrolling {
            return false;
        }
        self.state = GestureState::Idle;
        true
    }

    pub(crate) fn up(&mut self, now: Duration, x: f64, y: f64) -> Option<GestureAction> {
        let state = std::mem::replace(&mut self.state, GestureState::Idle);
        match state {
            GestureState::TapCandidate { down_at, position, button_seen, .. }
                if now.saturating_sub(down_at) <= self.double_tap_timeout
                    && (Point { x, y }).distance_squared(position) <= self.drag_slop_squared =>
            {
                self.last_tap = Some(Tap { ended_at: now, position: Point { x, y } });
                (!button_seen).then_some(GestureAction::Click)
            },
            GestureState::Dragging => {
                self.last_tap = None;
                Some(GestureAction::DragEnd)
            },
            GestureState::Moving | GestureState::Scrolling | GestureState::TapCandidate { .. } => {
                self.last_tap = None;
                None
            },
            GestureState::Idle => None,
        }
    }

    /// Fold Android's auxiliary primary-button action into the contact sequence. If a
    /// synthetic drag is already held, the physical button adopts that grab without sending a
    /// duplicate press; its normal ButtonRelease will release it.
    pub(crate) fn physical_button_press(&mut self) -> bool {
        match &mut self.state {
            GestureState::TapCandidate { button_seen, physical_button_active, .. } => {
                *button_seen = true;
                *physical_button_active = true;
                true
            },
            GestureState::Dragging => {
                self.state = GestureState::Moving;
                self.last_tap = None;
                false
            },
            _ => true,
        }
    }

    pub(crate) fn physical_button_release(&mut self) {
        if let GestureState::TapCandidate { physical_button_active, .. } = &mut self.state {
            *physical_button_active = false;
        }
    }

    pub(crate) fn cancel(&mut self) -> Option<GestureAction> {
        let action = (self.state == GestureState::Dragging).then_some(GestureAction::DragEnd);
        self.state = GestureState::Idle;
        self.last_tap = None;
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn single_tap_clicks_only_on_release() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 100.0, 100.0);
        assert_eq!(gestures.movement(103.0, 104.0), None);
        assert_eq!(gestures.up(ms(80), 103.0, 104.0), Some(GestureAction::Click));
    }

    #[test]
    fn movement_and_scroll_cancel_tap_without_clicking() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 10.0, 10.0);
        assert_eq!(gestures.movement(30.0, 10.0), None);
        assert_eq!(gestures.up(ms(40), 30.0, 10.0), None);

        gestures.down(ms(100), 10.0, 10.0);
        assert_eq!(gestures.scroll(), None);
        assert_eq!(gestures.up(ms(150), 10.0, 40.0), None);
    }

    #[test]
    fn double_tap_hold_then_move_drags_until_release() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 50.0, 50.0);
        assert_eq!(gestures.up(ms(50), 50.0, 50.0), Some(GestureAction::Click));
        gestures.down(ms(180), 52.0, 51.0);
        assert_eq!(gestures.movement(70.0, 51.0), Some(GestureAction::DragStart));
        assert_eq!(gestures.movement(90.0, 51.0), None);
        assert_eq!(gestures.up(ms(400), 90.0, 51.0), Some(GestureAction::DragEnd));
    }

    #[test]
    fn stale_or_distant_second_tap_does_not_drag() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 0.0, 0.0);
        assert_eq!(gestures.up(ms(20), 0.0, 0.0), Some(GestureAction::Click));
        gestures.down(ms(400), 0.0, 0.0);
        assert_eq!(gestures.movement(20.0, 0.0), None);
        assert_eq!(gestures.up(ms(450), 20.0, 0.0), None);
    }

    #[test]
    fn cancellation_releases_an_active_synthetic_drag() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 0.0, 0.0);
        gestures.up(ms(20), 0.0, 0.0);
        gestures.down(ms(100), 0.0, 0.0);
        assert_eq!(gestures.movement(20.0, 0.0), Some(GestureAction::DragStart));
        assert_eq!(gestures.cancel(), Some(GestureAction::DragEnd));
    }
}
