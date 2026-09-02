//! Platform-independent policy used by the Android/Wayland bridge.
//!
//! Keeping the arithmetic here (rather than in JNI or the compositor event loop) gives us a
//! small, deterministic surface that can be regression-tested on the host. Android reports
//! display values in physical pixels and millihertz; Wayland and Qt consume the same values but
//! are less forgiving of zero, negative, or accidentally rounded inputs.

/// Android's density-independent baseline. A display at 160 dpi is the 1x bucket.
pub const BASELINE_DPI: f64 = 160.0;

/// Return a safe, fractional UI scale factor for an Android density value.
pub fn density_scale_factor(density_dpi: i32) -> f64 {
    if density_dpi <= 0 {
        return 1.0;
    }
    (density_dpi as f64 / BASELINE_DPI).clamp(1.0, 8.0)
}

/// Format a Qt scale factor without locale-dependent decimal separators.
pub fn qt_scale_factor(scale_factor: f64) -> String {
    let scale = if scale_factor.is_nan() {
        1.0
    } else if scale_factor.is_infinite() {
        if scale_factor.is_sign_positive() {
            8.0
        } else {
            1.0
        }
    } else {
        scale_factor.clamp(1.0, 8.0)
    };
    format!("{scale:.3}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

/// Convert a fractional Qt scale to the traditional Xft dpi hint.
pub fn xft_dpi(scale_factor: f64) -> i32 {
    let scale = if scale_factor.is_finite() {
        scale_factor.clamp(1.0, 8.0)
    } else {
        1.0
    };
    (96.0 * scale).round().clamp(96.0, 768.0) as i32
}

/// Return one display refresh period in nanoseconds from Wayland's millihertz unit.
pub fn refresh_period_nanos(refresh_millihz: i32) -> u64 {
    let refresh = if refresh_millihz > 0 {
        refresh_millihz as u64
    } else {
        60_000
    };
    (1_000_000_000_000u64 / refresh).max(1)
}

/// Clamp an event coordinate to the compositor's normalized 0..1 range.
pub fn normalized_coordinate(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Return a valid physical window size, or `None` when Android has not supplied one yet.
pub fn physical_window_size(width: i32, height: i32) -> Option<(i32, i32)> {
    (width > 0 && height > 0).then_some((width, height))
}

/// Normalize Android's rotation values to the four rotations accepted by Wayland outputs.
pub fn normalized_rotation_degrees(degrees: i32) -> i32 {
    degrees.rem_euclid(360) / 90 * 90
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_fractional_density() {
        assert!((density_scale_factor(213) - 1.33125).abs() < 1e-9);
        assert_eq!(qt_scale_factor(density_scale_factor(213)), "1.331");
    }

    #[test]
    fn guards_invalid_and_extreme_density() {
        assert_eq!(density_scale_factor(0), 1.0);
        assert_eq!(density_scale_factor(-320), 1.0);
        assert_eq!(density_scale_factor(10_000), 8.0);
        assert_eq!(qt_scale_factor(f64::NAN), "1");
        assert_eq!(qt_scale_factor(f64::INFINITY), "8");
    }

    #[test]
    fn converts_scale_to_xft_dpi_without_integer_bucket_loss() {
        assert_eq!(xft_dpi(1.0), 96);
        assert_eq!(xft_dpi(1.5), 144);
        assert_eq!(xft_dpi(2.25), 216);
    }

    #[test]
    fn computes_refresh_period_from_millihertz() {
        assert_eq!(refresh_period_nanos(60_000), 16_666_666);
        assert_eq!(refresh_period_nanos(120_000), 8_333_333);
        assert_eq!(refresh_period_nanos(0), 16_666_666);
        assert_eq!(refresh_period_nanos(-1), 16_666_666);
    }

    #[test]
    fn clamps_pointer_coordinates_and_rejects_empty_windows() {
        assert_eq!(normalized_coordinate(-0.5), 0.0);
        assert_eq!(normalized_coordinate(1.5), 1.0);
        assert_eq!(normalized_coordinate(f64::NAN), 0.0);
        assert_eq!(physical_window_size(0, 100), None);
        assert_eq!(physical_window_size(2560, 1600), Some((2560, 1600)));
    }

    #[test]
    fn normalizes_arbitrary_rotation() {
        assert_eq!(normalized_rotation_degrees(0), 0);
        assert_eq!(normalized_rotation_degrees(90), 90);
        assert_eq!(normalized_rotation_degrees(-90), 270);
        assert_eq!(normalized_rotation_degrees(450), 90);
    }
}
