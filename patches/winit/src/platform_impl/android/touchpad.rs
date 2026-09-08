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
    double_tap_slop_squared: f64,
    drag_slop_squared: f64,
    // Android may report tap-to-click as auxiliary ACTION_BUTTON_PRESS /
    // ACTION_BUTTON_RELEASE events around the contact sequence. Portal owns
    // taps, so those button actions must not become a second guest click.
    suppress_primary_button_sequence: bool,
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
            suppress_primary_button_sequence: false,
        }
    }

    pub(crate) fn down(&mut self, now: Duration, x: f64, y: f64) {
        // A new contact is a new ownership decision. A late auxiliary button
        // sequence from the previous tap has either completed or will be
        // harmless because it was never entered in the forwarded-button set.
        self.suppress_primary_button_sequence = false;
        let position = Point { x, y };
        let second_tap = self.last_tap.is_some_and(|tap| {
            now.saturating_sub(tap.ended_at) <= self.double_tap_timeout
                && position.distance_squared(tap.position) <= self.double_tap_slop_squared
        });
        if !second_tap {
            self.last_tap = None;
        }
        self.state = GestureState::TapCandidate { down_at: now, position, second_tap };
    }

    pub(crate) fn movement(
        &mut self,
        now: Duration,
        x: f64,
        y: f64,
        primary_button_active: bool,
    ) -> Option<GestureAction> {
        let current = Point { x, y };

        // Android's touchpad mapper does not emit a second ACTION_DOWN for a
        // tap-hold. It emits the first tap's click sequence, then resumes
        // HoverMove when the second contact moves. Arm that contact from the
        // first hover sample inside the system double-tap window.
        if self.state == GestureState::Idle {
            let Some(tap) = self.last_tap else {
                return None;
            };
            if now.saturating_sub(tap.ended_at) > self.double_tap_timeout
                || current.distance_squared(tap.position) > self.double_tap_slop_squared
            {
                self.last_tap = None;
                return None;
            }
            self.last_tap = None;
            self.state =
                GestureState::TapCandidate { down_at: now, position: current, second_tap: true };
            return None;
        }

        let GestureState::TapCandidate { position, second_tap, .. } = self.state else {
            return None;
        };
        if (Point { x, y }).distance_squared(position) <= self.drag_slop_squared {
            return None;
        }

        self.last_tap = None;
        if second_tap || primary_button_active {
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

    pub(crate) fn is_scrolling(&self) -> bool {
        self.state == GestureState::Scrolling
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
                    && (Point { x, y }).distance_squared(position) <= self.drag_slop_squared =>
            {
                self.last_tap = Some(Tap { ended_at: now, position: Point { x, y } });
                // Keep ownership through a possible late auxiliary primary
                // button pair emitted after ACTION_UP.
                self.suppress_primary_button_sequence = true;
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

    /// Decide ownership of an Android primary-button press.
    ///
    /// Android reports PRIMARY already active on ACTION_DOWN for both a tap
    /// and a physical click. Defer ownership while a contact is a tap
    /// candidate; movement plus live PRIMARY state adopts a physical drag.
    pub(crate) fn physical_button_press(&mut self) -> bool {
        if self.suppress_primary_button_sequence {
            return false;
        }
        match &mut self.state {
            GestureState::TapCandidate { .. } => {
                self.suppress_primary_button_sequence = true;
                false
            },
            GestureState::Dragging => {
                self.suppress_primary_button_sequence = true;
                false
            },
            _ => true,
        }
    }

    /// Returns whether the release belongs to a forwarded physical press.
    pub(crate) fn physical_button_release(&mut self) -> bool {
        if self.suppress_primary_button_sequence {
            self.suppress_primary_button_sequence = false;
            return false;
        }
        true
    }

    pub(crate) fn cancel(&mut self) -> Option<GestureAction> {
        let action = (self.state == GestureState::Dragging).then_some(GestureAction::DragEnd);
        self.state = GestureState::Idle;
        self.last_tap = None;
        self.suppress_primary_button_sequence = false;
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
        assert_eq!(gestures.movement(ms(20), 103.0, 104.0, false), None);
        assert_eq!(gestures.up(ms(80), 103.0, 104.0), Some(GestureAction::Click));
    }

    #[test]
    fn movement_and_scroll_cancel_tap_without_clicking() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 10.0, 10.0);
        assert_eq!(gestures.movement(ms(20), 30.0, 10.0, false), None);
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
        assert_eq!(gestures.movement(ms(200), 70.0, 51.0, false), Some(GestureAction::DragStart));
        assert_eq!(gestures.movement(ms(220), 90.0, 51.0, false), None);
        assert_eq!(gestures.up(ms(400), 90.0, 51.0), Some(GestureAction::DragEnd));
    }

    #[test]
    fn stale_or_distant_second_tap_does_not_drag() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 0.0, 0.0);
        assert_eq!(gestures.up(ms(20), 0.0, 0.0), Some(GestureAction::Click));
        gestures.down(ms(400), 0.0, 0.0);
        assert_eq!(gestures.movement(ms(420), 20.0, 0.0, false), None);
        assert_eq!(gestures.up(ms(450), 20.0, 0.0), None);
    }

    #[test]
    fn cancellation_releases_an_active_synthetic_drag() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 0.0, 0.0);
        gestures.up(ms(20), 0.0, 0.0);
        gestures.down(ms(100), 0.0, 0.0);
        assert_eq!(gestures.movement(ms(120), 20.0, 0.0, false), Some(GestureAction::DragStart));
        assert_eq!(gestures.cancel(), Some(GestureAction::DragEnd));
    }

    #[test]
    fn auxiliary_primary_actions_do_not_duplicate_a_portal_tap() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 10.0, 10.0);
        assert!(!gestures.physical_button_press());
        assert!(!gestures.physical_button_release());
        assert_eq!(gestures.up(ms(40), 10.0, 10.0), Some(GestureAction::Click));

        // Some Android stacks finish the auxiliary pair after ACTION_UP.
        assert!(!gestures.physical_button_press());
        assert!(!gestures.physical_button_release());
    }

    #[test]
    fn auxiliary_primary_state_does_not_cancel_double_tap_drag() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 10.0, 10.0);
        assert_eq!(gestures.up(ms(30), 10.0, 10.0), Some(GestureAction::Click));
        gestures.down(ms(120), 11.0, 10.0);
        assert!(!gestures.physical_button_press());
        assert_eq!(gestures.movement(ms(150), 30.0, 10.0, false), Some(GestureAction::DragStart));
        assert!(!gestures.physical_button_release());
        assert_eq!(gestures.up(ms(240), 30.0, 10.0), Some(GestureAction::DragEnd));

        gestures.down(ms(300), 30.0, 10.0);
        assert_eq!(gestures.up(ms(320), 30.0, 10.0), Some(GestureAction::Click));
        gestures.down(ms(400), 31.0, 10.0);
        assert_eq!(gestures.movement(ms(420), 50.0, 10.0, true), Some(GestureAction::DragStart));
    }

    #[test]
    fn physical_click_ownership_remains_independent() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 10.0, 10.0);
        assert!(!gestures.physical_button_press());
        assert_eq!(gestures.movement(ms(20), 30.0, 10.0, true), Some(GestureAction::DragStart));
        assert!(!gestures.physical_button_release());
        assert_eq!(gestures.up(ms(50), 30.0, 10.0), Some(GestureAction::DragEnd));
    }

    #[test]
    fn android_hover_sequence_arms_second_tap_drag() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 100.0, 100.0);
        assert!(!gestures.physical_button_press());
        assert!(!gestures.physical_button_release());
        assert_eq!(gestures.up(ms(2), 100.0, 100.0), Some(GestureAction::Click));

        // The OnePlus mapper resumes HoverMove instead of issuing a second Down.
        assert_eq!(gestures.movement(ms(260), 100.0, 100.0, false), None);
        assert_eq!(gestures.movement(ms(470), 125.0, 100.0, false), Some(GestureAction::DragStart));
    }
}
