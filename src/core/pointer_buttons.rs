//! Host-testable guest pointer-button and touch-lift policy for viewports.
//!
//! Mouse button events (`winit` `MouseInput`) carry no position, so border
//! filtering cannot happen at ingestion the way motion/touch can. Instead the
//! Android bridge tracks whether the physical pointer is currently inside the
//! active guest viewport and consults this tracker before forwarding:
//!
//! - press inside the viewport → forward, remember the button
//! - press in a letterbox/pillarbox border → suppress (never synthesize a
//!   guest click at the last valid desktop position)
//! - release for a remembered button → always forward (never leave a guest
//!   button stuck, even if the release physical is outside after a resize)
//! - release for a button never forwarded → suppress
//!
//! Touch lifts use [`resolve_touch_lift`], a pure function over the gesture
//! outcome, so drag releases never snap to a fake viewport edge.

use std::collections::HashSet;

/// Which guest buttons were forwarded and are still held.
#[derive(Debug, Clone, Default)]
pub struct PointerButtonTracker {
    pressed: HashSet<u32>,
    inside: bool,
    last_physical: Option<(f64, f64)>,
}

impl PointerButtonTracker {
    /// New tracker. The cursor starts at the guest origin (inside), so a
    /// press with no prior motion forwards — best effort, matching where the
    /// guest cursor visibly sits.
    pub fn new() -> Self {
        Self {
            pressed: HashSet::new(),
            inside: true,
            last_physical: None,
        }
    }

    /// Record a motion outcome: `inside` is whether the motion's physical
    /// position mapped into the active viewport.
    pub fn note_motion(&mut self, inside: bool, physical: (f64, f64)) {
        self.inside = inside;
        self.last_physical = Some(physical);
    }

    /// Recompute `inside` after the viewport changed under a stationary
    /// cursor (host resize). Uses the same snapshot mapping as input so the
    /// flag can never disagree with the next ingestion decision.
    pub fn reevaluate(&mut self, snapshot: &crate::core::presentation::PresentationSnapshot) {
        if let Some((px, py)) = self.last_physical {
            self.inside = snapshot.physical_to_logical(px, py).is_some();
        }
    }

    /// A physical button press. Returns true when the guest should receive it.
    pub fn press(&mut self, button: u32) -> bool {
        if !self.inside {
            return false;
        }
        self.pressed.insert(button);
        true
    }

    /// A physical button release. Returns true when the guest should receive
    /// it — exactly the buttons previously forwarded, regardless of where
    /// the release physical lands (resize/drag-into-border must still
    /// release cleanly, with no synthesized edge motion).
    pub fn release(&mut self, button: u32) -> bool {
        self.pressed.remove(&button)
    }

    /// Forget all tracked buttons (suspend/focus-loss). Returns how many were
    /// still held so callers can log it; callers release them guest-side first
    /// via [`drain_pressed`].
    pub fn clear(&mut self) -> usize {
        let held = self.pressed.len();
        self.pressed.clear();
        held
    }

    /// Take all still-held buttons for guest-side release (suspend path).
    pub fn drain_pressed(&mut self) -> Vec<u32> {
        self.pressed.drain().collect()
    }

    /// Whether the physical pointer is currently inside the guest viewport.
    pub fn is_inside(&self) -> bool {
        self.inside
    }

    /// Whether `button` was forwarded and is still held.
    pub fn is_pressed(&self, button: u32) -> bool {
        self.pressed.contains(&button)
    }

    /// How many guest buttons are currently held.
    pub fn pressed_count(&self) -> usize {
        self.pressed.len()
    }
}

/// Touch-gesture outcome at lift time, mapped from the platform gesture mode
/// at the Android ingestion boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestTouchEnd {
    Tap,
    LongPress,
    Scroll,
    Drag,
}

/// What the compositor should do for a touch lift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchLiftAction {
    /// Do nothing (border tap, or scroll/drag end with nothing held).
    Ignore,
    /// Release the held drag button at the current guest pointer location —
    /// no synthesized movement to the lift coordinates.
    ReleaseAtCurrent,
    /// Left click at the lift coordinates (known in-guest).
    ClickLeft,
    /// Right click at the lift coordinates (known in-guest).
    ClickRight,
}

/// Pure touch-lift decision.
///
/// - A held drag button always releases cleanly at the current guest location,
///   even when the physical lift is in a border or after a resize. Coordinates
///   carried by a border lift are never used (no fake edge snap).
/// - Taps/long-presses lift outside the guest are ignored, never clamped
///   into edge clicks.
/// - Scroll/Drag ends with nothing held do nothing.
pub fn resolve_touch_lift(
    pressed_for_drag: bool,
    end: GuestTouchEnd,
    in_guest: bool,
) -> TouchLiftAction {
    if pressed_for_drag {
        return TouchLiftAction::ReleaseAtCurrent;
    }
    match (end, in_guest) {
        (GuestTouchEnd::Tap, true) => TouchLiftAction::ClickLeft,
        (GuestTouchEnd::LongPress, true) => TouchLiftAction::ClickRight,
        (GuestTouchEnd::Tap, false) | (GuestTouchEnd::LongPress, false) => TouchLiftAction::Ignore,
        (GuestTouchEnd::Scroll, _) | (GuestTouchEnd::Drag, _) => TouchLiftAction::Ignore,
    }
}
