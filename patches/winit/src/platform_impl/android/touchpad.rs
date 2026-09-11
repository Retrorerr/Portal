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
    // Explicit ownership of the synthetic left-button press Portal emits on
    // DragStart. Set exactly where DragStart is produced, cleared exactly
    // where the matching DragEnd is produced (see
    // finish_touchpad_drag_if_owned). Physical buttons are owned elsewhere
    // (the forwarded-button set in the event loop plus
    // suppress_primary_button_sequence below) and never consult this flag.
    synthetic_drag_owned: bool,
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
            synthetic_drag_owned: false,
            suppress_primary_button_sequence: false,
        }
    }

    /// Central termination for the synthetic drag: answers "has Portal
    /// emitted a synthetic left-button press that still requires a matching
    /// release?"
    ///
    /// Returns `DragEnd` only while Portal owns an active synthetic drag,
    /// clearing that ownership (and its dead tap context) atomically in the
    /// same operation. Idempotent: calling it twice yields exactly one
    /// `DragEnd`. Never releases a physical click: forwarded physical
    /// buttons live outside this flag and are unaffected.
    pub(crate) fn finish_touchpad_drag_if_owned(&mut self) -> Option<GestureAction> {
        if !self.synthetic_drag_owned {
            return None;
        }
        self.synthetic_drag_owned = false;
        self.last_tap = None;
        Some(GestureAction::DragEnd)
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
            // The DragStart below makes Portal emit a synthetic left press;
            // take ownership so exactly one release follows, however the
            // contact sequence terminates.
            self.synthetic_drag_owned = true;
            Some(GestureAction::DragStart)
        } else {
            self.state = GestureState::Moving;
            None
        }
    }

    pub(crate) fn scroll(&mut self) -> Option<GestureAction> {
        let action = self.finish_touchpad_drag_if_owned();
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
                self.finish_touchpad_drag_if_owned()
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
        let action = self.finish_touchpad_drag_if_owned();
        self.state = GestureState::Idle;
        self.last_tap = None;
        self.suppress_primary_button_sequence = false;
        action
    }

    /// Terminate on hover-exit: the contact left the pad without ACTION_UP
    /// (proven when a hover-dragged finger slides off the pad edge and the
    /// mapper then delivers no terminal event at all).
    ///
    /// Ends an owned synthetic drag exactly once. A pending double-tap
    /// window while Idle is preserved: re-entry always arrives via
    /// HoverEnter followed by HoverMove, which still needs `last_tap` to
    /// arm the second tap. Never touches physical button ownership.
    pub(crate) fn hover_exit(&mut self) -> Option<GestureAction> {
        let action = self.finish_touchpad_drag_if_owned();
        if self.state != GestureState::Idle {
            self.state = GestureState::Idle;
            self.last_tap = None;
        }
        action
    }

    /// Reconcile on hover-enter: a fresh hover cursor while Portal still
    /// owns a synthetic drag proves the drag contact died silently (no Up,
    /// Cancel, PointerUp, or HoverExit arrived). Normal operation never
    /// delivers HoverEnter mid-drag, so any other state is left untouched.
    /// Never touches physical button ownership.
    pub(crate) fn hover_enter(&mut self) -> Option<GestureAction> {
        let action = self.finish_touchpad_drag_if_owned();
        if action.is_some() {
            self.state = GestureState::Idle;
        }
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

    #[test]
    fn hover_started_drag_ends_exactly_once_on_hover_exit() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 100.0, 100.0);
        assert_eq!(gestures.up(ms(20), 100.0, 100.0), Some(GestureAction::Click));
        assert_eq!(gestures.movement(ms(200), 100.0, 100.0, false), None);
        assert_eq!(
            gestures.movement(ms(260), 125.0, 100.0, false),
            Some(GestureAction::DragStart)
        );
        assert_eq!(gestures.hover_exit(), Some(GestureAction::DragEnd));
        // Repeated termination must not double-release.
        assert_eq!(gestures.hover_exit(), None);
        assert_eq!(gestures.up(ms(400), 125.0, 100.0), None);
        assert_eq!(gestures.cancel(), None);
    }

    #[test]
    fn hover_started_drag_cancel_ends_exactly_once() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 0.0, 0.0);
        gestures.up(ms(20), 0.0, 0.0);
        assert_eq!(gestures.movement(ms(200), 0.0, 0.0, false), None);
        assert_eq!(
            gestures.movement(ms(240), 20.0, 0.0, false),
            Some(GestureAction::DragStart)
        );
        assert_eq!(gestures.cancel(), Some(GestureAction::DragEnd));
        assert_eq!(gestures.cancel(), None);
        assert_eq!(gestures.up(ms(400), 20.0, 0.0), None);
    }

    #[test]
    fn drag_into_two_finger_scroll_ends_exactly_once_before_scrolling() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 0.0, 0.0);
        gestures.up(ms(20), 0.0, 0.0);
        gestures.down(ms(100), 0.0, 0.0);
        assert_eq!(
            gestures.movement(ms(120), 20.0, 0.0, false),
            Some(GestureAction::DragStart)
        );
        assert_eq!(gestures.scroll(), Some(GestureAction::DragEnd));
        assert!(gestures.is_scrolling());
        assert_eq!(gestures.scroll(), None);
        assert!(gestures.end_scroll());
        // Lifting the scroll fingers must not click or re-release.
        assert_eq!(gestures.up(ms(300), 20.0, 40.0), None);
    }

    #[test]
    fn repeated_termination_never_double_releases() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 50.0, 50.0);
        assert_eq!(gestures.up(ms(30), 50.0, 50.0), Some(GestureAction::Click));
        gestures.down(ms(120), 51.0, 50.0);
        assert_eq!(
            gestures.movement(ms(150), 80.0, 50.0, false),
            Some(GestureAction::DragStart)
        );
        assert_eq!(gestures.hover_exit(), Some(GestureAction::DragEnd));
        assert_eq!(gestures.up(ms(300), 80.0, 50.0), None);
        assert_eq!(gestures.cancel(), None);
        assert_eq!(gestures.scroll(), None);
        assert_eq!(gestures.hover_enter(), None);
        // A fresh tap afterwards still clicks exactly once.
        gestures.down(ms(500), 80.0, 50.0);
        assert_eq!(gestures.up(ms(530), 80.0, 50.0), Some(GestureAction::Click));
    }

    #[test]
    fn new_contact_reconciles_silent_orphan_drag_before_rearming() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 100.0, 100.0);
        assert_eq!(gestures.up(ms(20), 100.0, 100.0), Some(GestureAction::Click));
        // Hover-armed second tap drags, then the contact dies silently.
        assert_eq!(gestures.movement(ms(200), 100.0, 100.0, false), None);
        assert_eq!(
            gestures.movement(ms(240), 125.0, 100.0, false),
            Some(GestureAction::DragStart)
        );
        // The next contact finishes the orphan exactly once, then arms fresh:
        // the orphan's tap context must not leak into the new contact.
        assert_eq!(
            gestures.finish_touchpad_drag_if_owned(),
            Some(GestureAction::DragEnd)
        );
        assert_eq!(gestures.finish_touchpad_drag_if_owned(), None);
        gestures.down(ms(500), 125.0, 100.0);
        assert_eq!(gestures.up(ms(530), 125.0, 100.0), Some(GestureAction::Click));
        assert_eq!(gestures.cancel(), None);
    }

    #[test]
    fn hover_enter_reconciles_silent_orphan_only_while_dragging() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 100.0, 100.0);
        assert_eq!(gestures.up(ms(20), 100.0, 100.0), Some(GestureAction::Click));
        assert_eq!(gestures.movement(ms(200), 100.0, 100.0, false), None);
        assert_eq!(
            gestures.movement(ms(240), 125.0, 100.0, false),
            Some(GestureAction::DragStart)
        );
        // Fresh hover while Portal still owns the drag: exactly one release.
        assert_eq!(gestures.hover_enter(), Some(GestureAction::DragEnd));
        assert_eq!(gestures.hover_enter(), None);
        assert_eq!(gestures.up(ms(500), 125.0, 100.0), None);

        // Hover-enter while Idle preserves the double-tap window.
        gestures.down(ms(600), 200.0, 200.0);
        assert_eq!(gestures.up(ms(620), 200.0, 200.0), Some(GestureAction::Click));
        assert_eq!(gestures.hover_enter(), None);
        gestures.down(ms(700), 201.0, 200.0);
        assert_eq!(gestures.up(ms(720), 201.0, 200.0), Some(GestureAction::Click));
    }

    #[test]
    fn hover_exit_in_idle_preserves_double_tap_window() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 100.0, 100.0);
        assert_eq!(gestures.up(ms(20), 100.0, 100.0), Some(GestureAction::Click));
        assert_eq!(gestures.hover_exit(), None);
        // Re-entry can still arm the hover-style second tap.
        assert_eq!(gestures.movement(ms(200), 100.0, 100.0, false), None);
        assert_eq!(
            gestures.movement(ms(260), 125.0, 100.0, false),
            Some(GestureAction::DragStart)
        );
        assert_eq!(gestures.up(ms(400), 125.0, 100.0), Some(GestureAction::DragEnd));
    }

    #[test]
    fn hover_enter_leaves_tap_candidate_undisturbed() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 10.0, 10.0);
        assert_eq!(gestures.hover_enter(), None);
        assert_eq!(gestures.up(ms(40), 10.0, 10.0), Some(GestureAction::Click));
    }

    #[test]
    fn forwarded_physical_press_is_not_released_by_synthetic_cleanup() {
        let mut gestures = TouchpadGestureStateMachine::default();
        gestures.down(ms(0), 10.0, 10.0);
        // Single-finger move without a second tap: plain movement, no drag.
        assert_eq!(gestures.movement(ms(20), 40.0, 10.0, false), None);
        // A physical press in Moving state is forwarded, not owned.
        assert!(gestures.physical_button_press());
        assert_eq!(gestures.cancel(), None);
        assert!(gestures.physical_button_release());
    }
}
