use std::time::Duration;

const DEFAULT_DOUBLE_TAP_TIMEOUT: Duration = Duration::from_millis(300);
const DEFAULT_MOVEMENT_SLOP_PX: f64 = 8.0;

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
    TapCandidate { down_at: Duration, position: Point, second_tap: bool },
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
    movement_slop_squared: f64,
}

impl Default for TouchpadGestureStateMachine {
    fn default() -> Self {
        Self {
            state: GestureState::Idle,
            last_tap: None,
            double_tap_timeout: DEFAULT_DOUBLE_TAP_TIMEOUT,
            movement_slop_squared: DEFAULT_MOVEMENT_SLOP_PX * DEFAULT_MOVEMENT_SLOP_PX,
        }
    }
}

impl TouchpadGestureStateMachine {
    pub(crate) fn down(&mut self, now: Duration, x: f64, y: f64) {
        let position = Point { x, y };
        let second_tap = self.last_tap.is_some_and(|tap| {
            now.saturating_sub(tap.ended_at) <= self.double_tap_timeout
                && position.distance_squared(tap.position) <= self.movement_slop_squared
        });
        if !second_tap {
            self.last_tap = None;
        }
        self.state = GestureState::TapCandidate { down_at: now, position, second_tap };
    }

    pub(crate) fn movement(&mut self, x: f64, y: f64) -> Option<GestureAction> {
        let GestureState::TapCandidate { position, second_tap, .. } = self.state else {
            return None;
        };
        if (Point { x, y }).distance_squared(position) <= self.movement_slop_squared {
            return None;
        }

        self.last_tap = None;
        if second_tap {
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
            GestureState::TapCandidate { down_at, position, .. }
                if now.saturating_sub(down_at) <= self.double_tap_timeout
                    && (Point { x, y }).distance_squared(position)
                        <= self.movement_slop_squared =>
            {
                self.last_tap = Some(Tap { ended_at: now, position: Point { x, y } });
                Some(GestureAction::Click)
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
