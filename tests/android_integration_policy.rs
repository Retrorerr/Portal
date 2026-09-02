//! Host-side regression tests for Android display and input policy.
//!
//! The Android modules themselves are behind `cfg(target_os = "android")`, so these tests include
//! the platform-independent policy directly. This keeps CI useful even when an ARM64 device is
//! not attached.

#[path = "../src/core/android_input.rs"]
mod android_input;
#[path = "../src/core/android_integration.rs"]
mod android_integration;
#[path = "../src/core/clipboard_policy.rs"]
mod clipboard_policy;

use android_input::{android_keycode_to_scancode, committed_ascii_to_key_events};
use android_integration::{
    density_scale_factor, normalized_coordinate, normalized_rotation_degrees, physical_window_size,
    qt_scale_factor, refresh_period_nanos, xft_dpi,
};
use clipboard_policy::{choose_text_mime, supports_mime_type, TEXT_MIME, UTF8_TEXT_MIME};

#[test]
fn oneplus_pad_like_metrics_keep_fractional_scale_and_refresh_period() {
    let scale = density_scale_factor(280);
    assert!((scale - 1.75).abs() < f64::EPSILON);
    assert_eq!(qt_scale_factor(scale), "1.75");
    assert_eq!(xft_dpi(scale), 168);
    assert_eq!(refresh_period_nanos(144_000), 6_944_444);
    assert_eq!(physical_window_size(2560, 1600), Some((2560, 1600)));
}

#[test]
fn malformed_android_events_are_safe_to_forward_or_drop() {
    assert_eq!(normalized_coordinate(f64::NAN), 0.0);
    assert_eq!(normalized_coordinate(2.0), 1.0);
    assert_eq!(physical_window_size(0, 1600), None);
    assert_eq!(normalized_rotation_degrees(-90), 270);
    assert_eq!(android_keycode_to_scancode(0), None);
    assert_eq!(android_keycode_to_scancode(67), Some(14));
}

#[test]
fn software_keyboard_ascii_commit_maps_to_physical_keys() {
    assert_eq!(
        committed_ascii_to_key_events("Konsole\n"),
        vec![
            (37, true),
            (24, false),
            (49, false),
            (31, false),
            (24, false),
            (38, false),
            (18, false),
            (28, false),
        ]
    );
}

#[test]
fn software_keyboard_delete_commit_maps_to_backspace() {
    assert_eq!(committed_ascii_to_key_events("\u{8}"), vec![(14, false)]);
}

#[test]
fn clipboard_bridge_accepts_text_only_and_prefers_utf8() {
    assert!(supports_mime_type(TEXT_MIME));
    assert!(supports_mime_type(UTF8_TEXT_MIME));
    assert!(!supports_mime_type("text/html"));
    assert_eq!(
        choose_text_mime(["text/html", TEXT_MIME, UTF8_TEXT_MIME]),
        Some(UTF8_TEXT_MIME)
    );
}
