//! Android stylus samples in the terms of the Anland Portal Stylus
//! (`patches/kwin/anland-6.3.6/0003`), which feeds KWin's zwp_tablet_v2 tool.

/// Hovering within range, or touching. Mirrors KWin's `protocol.h`.
pub const TABLET_TOOL_IN_PROXIMITY: u32 = 1 << 0;
/// Touching the screen.
pub const TABLET_TOOL_TIP_DOWN: u32 = 1 << 1;
/// The eraser end.
pub const TABLET_TOOL_ERASER: u32 = 1 << 2;
/// `BUTTON_STYLUS_PRIMARY`, reported to clients as `BTN_STYLUS`.
pub const TABLET_TOOL_BUTTON1: u32 = 1 << 3;
/// `BUTTON_STYLUS_SECONDARY`, reported to clients as `BTN_STYLUS2`.
pub const TABLET_TOOL_BUTTON2: u32 = 1 << 4;

/// State flags for one pen sample.
pub fn tool_flags(
    in_range: bool,
    down: bool,
    eraser: bool,
    primary_button: bool,
    secondary_button: bool,
) -> u32 {
    let mut flags = 0;
    for (set, bit) in [
        (in_range, TABLET_TOOL_IN_PROXIMITY),
        (down, TABLET_TOOL_TIP_DOWN),
        (eraser, TABLET_TOOL_ERASER),
        (primary_button, TABLET_TOOL_BUTTON1),
        (secondary_button, TABLET_TOOL_BUTTON2),
    ] {
        if set {
            flags |= bit;
        }
    }
    flags
}

/// Android's pen tilt as zwp_tablet_tool_v2 tilt, in degrees.
///
/// Android gives `AXIS_TILT` (radians away from perpendicular) and
/// `AXIS_ORIENTATION` (the direction the pen points: 0 up, π/2 right). The
/// Wayland tilt is the angle of the pen's top along each axis, positive towards
/// +x (right) and +y (down), and the top leans away from where the pen points.
/// This is Chromium's conversion for Android pointer events.
pub fn tilt_degrees(tilt: f32, orientation: f32) -> (f32, f32) {
    if !tilt.is_finite() || !orientation.is_finite() {
        return (0.0, 0.0);
    }
    let (lean, normal) = tilt.sin_cos();
    let tilt_x = (-orientation.sin() * lean).atan2(normal);
    let tilt_y = (orientation.cos() * lean).atan2(normal);
    (tilt_x.to_degrees(), tilt_y.to_degrees())
}

/// Clamp a 0..1 axis (pressure, hover distance); non-finite reads as 0.
pub fn unit(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, FRAC_PI_6, PI};

    fn close(actual: (f32, f32), expected: (f32, f32)) {
        assert!(
            (actual.0 - expected.0).abs() < 1e-3 && (actual.1 - expected.1).abs() < 1e-3,
            "{actual:?} != {expected:?}"
        );
    }

    #[test]
    fn upright_pen_has_no_tilt() {
        close(tilt_degrees(0.0, 1.0), (0.0, 0.0));
    }

    #[test]
    fn pen_pointing_up_leans_towards_the_bottom() {
        close(tilt_degrees(FRAC_PI_4, 0.0), (0.0, 45.0));
    }

    #[test]
    fn pen_pointing_right_leans_left() {
        close(tilt_degrees(FRAC_PI_6, FRAC_PI_2), (-30.0, 0.0));
    }

    #[test]
    fn pen_pointing_down_leans_towards_the_top() {
        close(tilt_degrees(FRAC_PI_4, PI), (0.0, -45.0));
    }

    #[test]
    fn diagonal_tilt_splits_across_both_axes() {
        // Pointing up-left, the usual right-handed grip: top leans down-right.
        let (x, y) = tilt_degrees(FRAC_PI_4, -FRAC_PI_4);
        assert!(x > 0.0 && y > 0.0 && (x - y).abs() < 1e-3);
    }

    #[test]
    fn garbage_axes_read_as_upright_and_clamped() {
        close(tilt_degrees(f32::NAN, 0.0), (0.0, 0.0));
        assert_eq!(unit(1.7), 1.0);
        assert_eq!(unit(-0.2), 0.0);
        assert_eq!(unit(f32::NAN), 0.0);
    }

    #[test]
    fn flags_match_kwin_protocol_bits() {
        assert_eq!(tool_flags(false, false, false, false, false), 0);
        assert_eq!(tool_flags(true, true, false, false, false), 0b11);
        assert_eq!(tool_flags(true, false, true, true, true), 0b11101);
    }
}
