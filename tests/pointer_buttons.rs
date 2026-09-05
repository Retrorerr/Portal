//! State-level pointer-button and touch-lift tests for letterboxed viewports.
//!
//! Mouse button events carry no position, so suppression must come from
//! tracked inside/outside state, not ingestion mapping. These tests drive
//! [`PointerButtonTracker`] and [`resolve_touch_lift`] directly on the host:
//! move-into-border-then-click, press-inside/release-border, border presses,
//! resize between press and release, drag-into-border release, return into
//! guest, stuck-button freedom, and no accidental clicks at last valid
//! coordinates.

use localdesktop::core::coordinate_transform::AuthoritativeDisplayState;
use localdesktop::core::pointer_buttons::{
    resolve_touch_lift, GuestTouchEnd, PointerButtonTracker, TouchLiftAction,
};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const PLASMA: f64 = 2.25;

/// A converged fullscreen state plus a tracker that already saw an interior
/// motion (the normal steady state before any test action).
fn converged_with_tracker() -> (AuthoritativeDisplayState, PointerButtonTracker) {
    let mut state = AuthoritativeDisplayState::new(3392, 2400, 420, 144_000);
    state.update_kwin_scale(PLASMA);
    state.note_kwin_commit(None, Some((1130.0, 800.0)), Some(3));
    assert!(state.presentation_snapshot().converged);
    let mut tracker = PointerButtonTracker::new();
    tracker.note_motion(true, (1696.0, 1200.0));
    (state, tracker)
}

/// Resize the state to a letterboxed transitional host and reevaluate the
/// tracker, returning the new snapshot.
fn resize_to_letterboxed(
    state: &mut AuthoritativeDisplayState,
    tracker: &mut PointerButtonTracker,
    host: (i32, i32),
) -> localdesktop::core::presentation::PresentationSnapshot {
    state.try_update_physical_size(host.0, host.1).unwrap();
    let snap = state.presentation_snapshot();
    assert!(!snap.converged);
    tracker.reevaluate(&snap);
    snap
}

#[test]
fn move_into_border_then_click_is_suppressed() {
    let (_state, mut tracker) = converged_with_tracker();
    // Cursor slides into the black FIT border.
    tracker.note_motion(false, (5.0, 5.0));
    assert!(!tracker.is_inside());
    // Click there must not reach the guest — not even at last valid coords.
    assert!(!tracker.press(BTN_LEFT));
    assert!(!tracker.release(BTN_LEFT));
    assert_eq!(tracker.pressed_count(), 0);
}

#[test]
fn press_inside_release_border_releases_cleanly() {
    let (mut state, mut tracker) = converged_with_tracker();
    assert!(tracker.press(BTN_LEFT));
    assert!(tracker.is_pressed(BTN_LEFT));
    // Host resizes under the held button; the stored physical is now outside.
    let _snap = resize_to_letterboxed(&mut state, &mut tracker, (1100, 900));
    assert!(!tracker.is_inside());
    // Release must still reach the guest exactly once (no stuck button)…
    assert!(tracker.release(BTN_LEFT));
    assert_eq!(tracker.pressed_count(), 0);
    // …and never again.
    assert!(!tracker.release(BTN_LEFT));
}

#[test]
fn border_press_never_arms_a_button() {
    let (_state, mut tracker) = converged_with_tracker();
    tracker.note_motion(false, (2.0, 1199.0));
    assert!(!tracker.press(BTN_LEFT));
    assert!(!tracker.press(BTN_RIGHT));
    assert_eq!(tracker.pressed_count(), 0);
    // Later releases for never-pressed buttons stay suppressed…
    assert!(!tracker.release(BTN_LEFT));
    // …until the cursor returns into the guest, when presses work again.
    tracker.note_motion(true, (1696.0, 1200.0));
    assert!(tracker.is_inside());
    assert!(tracker.press(BTN_LEFT));
    assert!(tracker.release(BTN_LEFT));
}

#[test]
fn resize_between_press_and_release_keeps_release() {
    let (mut state, mut tracker) = converged_with_tracker();
    assert!(tracker.press(BTN_RIGHT));
    // Any resize — even one that keeps the stored point inside — must not
    // disarm the held button.
    let snap = resize_to_letterboxed(&mut state, &mut tracker, (2100, 1600));
    let _ = snap;
    assert!(tracker.release(BTN_RIGHT));
    assert_eq!(tracker.pressed_count(), 0);
}

#[test]
fn drag_into_border_release_has_no_edge_motion_and_no_stuck_button() {
    // Touch-drag release in a border resolves to releasing at the current
    // guest location; the carried coordinates are ignored downstream.
    assert_eq!(
        resolve_touch_lift(true, GuestTouchEnd::Drag, false),
        TouchLiftAction::ReleaseAtCurrent
    );
    assert_eq!(
        resolve_touch_lift(true, GuestTouchEnd::Tap, false),
        TouchLiftAction::ReleaseAtCurrent
    );
    // …and the mouse tracker mirrors it: held button releases once.
    let (mut state, mut tracker) = converged_with_tracker();
    assert!(tracker.press(BTN_LEFT));
    resize_to_letterboxed(&mut state, &mut tracker, (1100, 900));
    assert!(tracker.release(BTN_LEFT));
    assert!(!tracker.release(BTN_LEFT));
}

#[test]
fn touch_lift_matrix_never_clicks_from_borders() {
    // In-guest lifts behave exactly like today.
    assert_eq!(
        resolve_touch_lift(false, GuestTouchEnd::Tap, true),
        TouchLiftAction::ClickLeft
    );
    assert_eq!(
        resolve_touch_lift(false, GuestTouchEnd::LongPress, true),
        TouchLiftAction::ClickRight
    );
    assert_eq!(
        resolve_touch_lift(false, GuestTouchEnd::Scroll, true),
        TouchLiftAction::Ignore
    );
    assert_eq!(
        resolve_touch_lift(false, GuestTouchEnd::Drag, true),
        TouchLiftAction::Ignore
    );
    // Border lifts never become edge clicks…
    assert_eq!(
        resolve_touch_lift(false, GuestTouchEnd::Tap, false),
        TouchLiftAction::Ignore
    );
    assert_eq!(
        resolve_touch_lift(false, GuestTouchEnd::LongPress, false),
        TouchLiftAction::Ignore
    );
    // …scroll/drag ends do nothing either way when nothing is held.
    assert_eq!(
        resolve_touch_lift(false, GuestTouchEnd::Scroll, false),
        TouchLiftAction::Ignore
    );
    assert_eq!(
        resolve_touch_lift(false, GuestTouchEnd::Drag, false),
        TouchLiftAction::Ignore
    );
}

#[test]
fn suspend_drains_without_stuck_buttons() {
    let (_state, mut tracker) = converged_with_tracker();
    assert!(tracker.press(BTN_LEFT));
    assert!(tracker.press(BTN_RIGHT));
    let mut drained = tracker.drain_pressed();
    drained.sort_unstable();
    assert_eq!(drained, vec![BTN_LEFT, BTN_RIGHT]);
    assert_eq!(tracker.pressed_count(), 0);
    assert_eq!(tracker.clear(), 0);
    // Cursor side is untouched by button draining.
    assert!(tracker.is_inside());
}

#[test]
fn tracker_reevaluate_tracks_viewport_truth() {
    let (mut state, mut tracker) = converged_with_tracker();
    // A point on the right edge of the converged desktop…
    tracker.note_motion(true, (3390.0, 1200.0));
    // …falls into the pillarbox after shrinking to a narrow popup, and comes
    // back after returning fullscreen and moving inside again.
    let snap = resize_to_letterboxed(&mut state, &mut tracker, (900, 1600));
    assert!(!tracker.is_inside());
    assert!(snap.physical_to_logical(3390.0, 1200.0).is_none());
    tracker.note_motion(true, (450.0, 800.0));
    assert!(tracker.is_inside());
}
