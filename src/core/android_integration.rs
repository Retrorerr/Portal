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

/// Portal's interactive desktop frame-rate hint in millihertz.
///
/// Fallback when Android's supported-mode list is unavailable: requested
/// through the supported `ANativeWindow_setFrameRate[WithChangeStrategy]`
/// path as a hint only. The live path prefers
/// [`select_preferred_refresh_millihz`] over the reported modes; this stays
/// as the sane default on devices where enumeration fails.
pub const DESIRED_REFRESH_MILLIHZ: i32 = 120_000;

/// Stable nominal refresh advertised to nested KWin via `wl_output` (millihertz).
///
/// Fallback when Android's supported-mode list is unavailable. The live path
/// resolves the nominal target with [`select_preferred_refresh_millihz`] over
/// `Display.getSupportedModes()` instead of using this fixed value directly,
/// so a 144 Hz panel advertises 144 Hz while 120/90/60 Hz devices keep their
/// own maximum.
///
/// This is intentionally NOT the instantaneous Android physical/VRR scanout
/// rate (which legitimately varies between 50/60/90/120/144 Hz on the OnePlus
/// Pad 3). KWin's virtual output must see a stable intended mode so cold-start
/// fullscreen boots at the preferred rate without requiring a popup transition
/// to re-sample Android while it happens to be at that rate. The live physical
/// rate is tracked separately for diagnostics/pacing and never rewrites this
/// mode. Physical presentation timing itself is communicated through
/// frame/presentation feedback, not mode rewrites.
pub const NOMINAL_OUTPUT_REFRESH_MILLIHZ: i32 = 120_000;

/// Select the preferred stable high-refresh target from Android's reported
/// supported display modes (millihertz).
///
/// Portable policy: prefer the highest *valid* supported rate so a 144 Hz
/// panel resolves to 144 Hz while 120/90/60 Hz devices resolve to their own
/// maximum. Invalid readings (zero/negative/absurd) are ignored; an empty or
/// fully-invalid list falls back to [`NOMINAL_OUTPUT_REFRESH_MILLIHZ`] so
/// callers always get a usable nominal mode. Pure and host-testable: JNI
/// enumeration lives in `ndk`, selection lives here.
pub fn select_preferred_refresh_millihz(supported_millihz: &[i32]) -> i32 {
    supported_millihz
        .iter()
        .copied()
        .filter(|rate| is_valid_refresh_millihz(*rate))
        .max()
        .unwrap_or(NOMINAL_OUTPUT_REFRESH_MILLIHZ)
}

/// Hysteresis for host refresh-change detection (millihertz).
///
/// Fractional modes (e.g. 59.94 Hz vs 60 Hz differ by ~60 millihertz) must not
/// flap the Wayland output. Real OnePlus Pad 3 steps (50/60/90/120/144 Hz) differ
/// by >= 10_000 millihertz, so 500 cleanly separates noise from real switches.
pub const REFRESH_CHANGE_THRESHOLD_MILLIHZ: i32 = 500;

/// Whether a millihertz refresh value is usable for Wayland output timing.
pub fn is_valid_refresh_millihz(refresh_millihz: i32) -> bool {
    (1_000..=1_000_000).contains(&refresh_millihz)
}

/// Whether an observed host refresh differs enough from the tracked physical
/// rate to be worth logging.
///
/// This is for physical/VRR diagnostics only: it must NEVER gate a `wl_output`
/// mode rewrite. The Wayland output mode is the stable preferred target from
/// [`select_preferred_refresh_millihz`]; transient 50/60/90/120/144 Hz scanout
/// changes leave Plasma's configured mode untouched.
///
/// Returns false when the new reading is invalid (preserve last valid) or when
/// the delta is within hysteresis. Pure, host-testable; callers must only use
/// it to update separately-tracked physical state, never `wl_output`.
pub fn refresh_changed(current_millihz: i32, observed_millihz: i32) -> bool {
    if !is_valid_refresh_millihz(observed_millihz) {
        return false;
    }
    if !is_valid_refresh_millihz(current_millihz) {
        return true;
    }
    (observed_millihz - current_millihz).abs() >= REFRESH_CHANGE_THRESHOLD_MILLIHZ
}

/// Clamp an event coordinate to the compositor's normalized 0..1 range.
pub fn normalized_coordinate(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Clamp a physical screen coordinate to the valid pixel range `[0.0, max_bound]`.
/// Infinite or NaN coordinates default to 0.0.
pub fn clamp_physical_coordinate(coord: f64, max_bound: i32) -> f64 {
    if !coord.is_finite() {
        return 0.0;
    }
    let max = (max_bound.max(0)) as f64;
    coord.clamp(0.0, max)
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
    fn refresh_change_detection_ignores_fractional_noise() {
        // 59.94 Hz (59940) vs 60 Hz must not flap the output.
        assert!(!refresh_changed(60_000, 59_940));
        assert!(!refresh_changed(60_000, 60_000));
        // Real OnePlus Pad 3 VRR steps must trigger.
        assert!(refresh_changed(60_000, 50_000));
        assert!(refresh_changed(60_000, 120_000));
        assert!(refresh_changed(50_000, 60_000));
        assert!(refresh_changed(120_000, 144_000));
        // Invalid readings never trigger (preserve last valid).
        assert!(!refresh_changed(60_000, 0));
        assert!(!refresh_changed(60_000, -1));
        assert!(!refresh_changed(60_000, 2_000_000));
        // Invalid current with a valid observation adopts the observation.
        assert!(refresh_changed(0, 120_000));
        assert!(is_valid_refresh_millihz(DESIRED_REFRESH_MILLIHZ));
        assert_eq!(DESIRED_REFRESH_MILLIHZ, 120_000);
    }

    #[test]
    fn preferred_refresh_selects_highest_valid_supported_mode() {
        // OnePlus Pad 3: full VRR/mode list resolves to 144 Hz.
        assert_eq!(
            select_preferred_refresh_millihz(&[50_000, 60_000, 90_000, 120_000, 144_000]),
            144_000
        );
        // Order-independent.
        assert_eq!(
            select_preferred_refresh_millihz(&[144_000, 60_000, 120_000]),
            144_000
        );
        // Portable fallback: 120/90/60 Hz devices keep their own maximum.
        assert_eq!(select_preferred_refresh_millihz(&[60_000, 120_000]), 120_000);
        assert_eq!(select_preferred_refresh_millihz(&[60_000, 90_000]), 90_000);
        assert_eq!(select_preferred_refresh_millihz(&[60_000]), 60_000);
        assert_eq!(select_preferred_refresh_millihz(&[59_940, 60_000]), 60_000);
        // Invalid entries are ignored, never selected.
        assert_eq!(select_preferred_refresh_millihz(&[0, -1, 144_000]), 144_000);
        assert_eq!(select_preferred_refresh_millihz(&[0, -1, 2_000_000]), 120_000);
        // Empty/unusable lists fall back to the sane default.
        assert_eq!(select_preferred_refresh_millihz(&[]), 120_000);
        assert_eq!(select_preferred_refresh_millihz(&[0]), 120_000);
        assert_eq!(
            select_preferred_refresh_millihz(&[]),
            NOMINAL_OUTPUT_REFRESH_MILLIHZ
        );
    }

    #[test]
    fn clamps_pointer_coordinates_and_rejects_empty_windows() {
        assert_eq!(normalized_coordinate(-0.5), 0.0);
        assert_eq!(normalized_coordinate(1.5), 1.0);
        assert_eq!(normalized_coordinate(f64::NAN), 0.0);
        assert_eq!(physical_window_size(0, 100), None);
        assert_eq!(physical_window_size(2560, 1600), Some((2560, 1600)));
        assert_eq!(physical_window_size(3392, 2400), Some((3392, 2400)));
    }

    #[test]
    fn clamps_physical_coordinates_for_high_res_displays() {
        // OnePlus Pad 3 physical bounds: 3392 x 2400
        assert_eq!(clamp_physical_coordinate(1500.0, 3392), 1500.0);
        assert_eq!(clamp_physical_coordinate(-10.0, 3392), 0.0);
        assert_eq!(clamp_physical_coordinate(3400.0, 3392), 3392.0);
        assert_eq!(clamp_physical_coordinate(f64::NAN, 3392), 0.0);
        assert_eq!(clamp_physical_coordinate(f64::INFINITY, 3392), 0.0);
        assert_eq!(clamp_physical_coordinate(f64::NEG_INFINITY, 3392), 0.0);

        assert_eq!(clamp_physical_coordinate(2400.0, 2400), 2400.0);
        assert_eq!(clamp_physical_coordinate(2400.1, 2400), 2400.0);
        assert_eq!(clamp_physical_coordinate(-0.1, 2400), 0.0);
    }

    #[test]
    fn normalizes_arbitrary_rotation() {
        assert_eq!(normalized_rotation_degrees(0), 0);
        assert_eq!(normalized_rotation_degrees(90), 90);
        assert_eq!(normalized_rotation_degrees(-90), 270);
        assert_eq!(normalized_rotation_degrees(450), 90);
    }
}
