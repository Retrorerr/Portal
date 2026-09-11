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
#[path = "../src/core/tablet_mode.rs"]
mod tablet_mode;

const ANDROID_CLIPBOARD_SOURCE: &str = include_str!("../src/android/clipboard.rs");
const ANDROID_COMPOSITOR_SOURCE: &str =
    include_str!("../src/android/backend/wayland/compositor.rs");
const ANDROID_TEXT_INPUT_V2_SOURCE: &str =
    include_str!("../src/android/backend/wayland/text_input_v2.rs");
const ANDROID_KEYBOARD_BRIDGE_SOURCE: &str =
    include_str!("../src/android/java/app/polarbear/SoftKeyboardBridge.java");
const ANDROID_SETUP_SOURCE: &str = include_str!("../src/android/proot/setup.rs");
const ANDROID_IME_SOURCE: &str = include_str!("../src/android/ime.rs");
const KWIN_WRAPPER_SOURCE: &str = include_str!("../assets/localdesktop-kwin-wrapper-v2.sh");
const STARTPLASMA_SOURCE: &str = include_str!("../assets/localdesktop-startplasma.sh");
const PORTAL_IME_BRIDGE_SOURCE: &str = include_str!("../assets/portal-ime-bridge.py");
const PORTAL_IBUS_LAZY_SOURCE: &str = include_str!("../assets/portal-ibus-lazy.sh");
const ANLAND_ENV_SOURCE: &str = include_str!("../src/android/anland/mod.rs");
const MESA_LAYER_SOURCE: &str = include_str!("../src/android/proot/mesa_layer.rs");
const XWAYLAND_SHA256SUMS_SOURCE: &str =
    include_str!("../assets/xwayland-candidate/SHA256SUMS");

use android_input::{android_keycode_to_scancode, committed_ascii_to_key_events};
use android_integration::{
    clamp_physical_coordinate, density_scale_factor, is_valid_refresh_millihz,
    normalized_coordinate, normalized_rotation_degrees, physical_window_size, qt_scale_factor,
    refresh_changed, refresh_period_nanos, select_preferred_refresh_millihz, xft_dpi,
    DESIRED_REFRESH_MILLIHZ, NOMINAL_OUTPUT_REFRESH_MILLIHZ,
};
use clipboard_policy::{
    choose_text_mime, is_valid_clip_text, supports_mime_type, validate_clip_text,
    MAX_CLIPBOARD_BYTES, TEXT_MIME, UTF8_TEXT_MIME,
};

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
fn nominal_output_refresh_is_independent_of_physical_vrr() {
    // Fallback constants stay sane when mode enumeration is unavailable.
    assert_eq!(NOMINAL_OUTPUT_REFRESH_MILLIHZ, 120_000);
    assert_eq!(DESIRED_REFRESH_MILLIHZ, 120_000);
    assert_eq!(
        refresh_period_nanos(NOMINAL_OUTPUT_REFRESH_MILLIHZ),
        8_333_333
    );
    // Dynamic selection: OnePlus Pad 3's reported modes resolve to 144 Hz.
    assert_eq!(
        select_preferred_refresh_millihz(&[50_000, 60_000, 90_000, 120_000, 144_000]),
        144_000
    );
    assert_eq!(refresh_period_nanos(144_000), 6_944_444);
    // Portable fallback: 120/90/60 Hz devices keep their own maximum.
    assert_eq!(
        select_preferred_refresh_millihz(&[60_000, 120_000]),
        120_000
    );
    assert_eq!(select_preferred_refresh_millihz(&[60_000, 90_000]), 90_000);
    assert_eq!(select_preferred_refresh_millihz(&[60_000]), 60_000);
    // Empty/unusable lists fall back to the sane default.
    assert_eq!(select_preferred_refresh_millihz(&[]), 120_000);
    assert_eq!(select_preferred_refresh_millihz(&[0, -1]), 120_000);
    // Effective refresh changes are observed ...
    assert!(refresh_changed(60_000, 50_000));
    assert!(refresh_changed(60_000, 120_000));
    assert!(refresh_changed(120_000, 144_000));
    assert!(refresh_changed(50_000, 60_000));
    // ... but fractional noise never counts as a physical change.
    assert!(!refresh_changed(60_000, 59_940));
    assert!(!refresh_changed(60_000, 60_000));
    assert!(!refresh_changed(60_000, 0));
    assert!(is_valid_refresh_millihz(50_000));
    assert!(is_valid_refresh_millihz(60_000));
    assert!(is_valid_refresh_millihz(120_000));
    assert!(is_valid_refresh_millihz(144_000));

    // Requested maximum and effective output are distinct; the output follows Android.
    const EVENT_HANDLER: &str = include_str!("../src/android/backend/wayland/event_handler.rs");
    const RUN: &str = include_str!("../src/android/app/run.rs");
    const BACKEND_MOD: &str = include_str!("../src/android/backend/wayland/mod.rs");
    const NDK: &str = include_str!("../src/android/utils/ndk.rs");
    const FRAME_RATE: &str = include_str!("../src/android/utils/frame_rate.rs");
    const SETUP: &str = include_str!("../src/android/proot/setup.rs");
    // Physical rate is tracked on its own field.
    assert!(BACKEND_MOD.contains("physical_refresh_millihz"));
    assert!(EVENT_HANDLER.contains("physical_refresh_millihz"));
    assert!(RUN.contains("physical_refresh_millihz"));
    // Nominal comes from the supported-mode enumeration, consistently used
    // for the output mode and the frame-rate hint.
    assert!(NDK.contains("getSupportedModes"));
    assert!(NDK.contains("preferred_high_refresh_millihz"));
    assert!(NDK.contains("select_preferred_refresh_millihz"));
    assert!(RUN.contains("ndk::refresh_rate_millihz"));
    assert!(RUN.contains("preferred_high_refresh_millihz"));
    assert!(RUN.contains("ensure_high_refresh_rate_hz"));
    assert!(SETUP.contains("ndk::preferred_high_refresh_millihz"));
    assert!(FRAME_RATE.contains("preferred_frame_rate_hz"));
    assert!(FRAME_RATE.contains("ensure_high_refresh_rate_hz"));
    // No device-name checks, OEM APIs, or global-setting writes: selection is
    // purely from the supported-mode list. (`Build`/`MODEL` gating would show
    // up as these strings; doc-comment device mentions are fine.)
    for src in [NDK, FRAME_RATE, RUN, SETUP] {
        assert!(!src.contains("os/Build"));
        assert!(!src.contains("MANUFACTURER"));
        assert!(!src.contains("Build.MODEL"));
        assert!(!src.contains("Settings.Global"));
        assert!(!src.contains("Settings.System"));
    }
    // The periodic poll must never publish physical VRR as a nominal mode.
    let poll_start = EVENT_HANDLER
        .find("fn maybe_poll_refresh_rate")
        .expect("physical sampler is present");
    let dispatch_start = EVENT_HANDLER[poll_start..]
        .find("fn dispatch_wayland")
        .map(|i| poll_start + i)
        .expect("dispatch follows sampler");
    let poll_body = &EVENT_HANDLER[poll_start..dispatch_start];
    assert!(poll_body.contains("physical_refresh_millihz"));
    assert!(!poll_body.contains("set_preferred"));
    assert!(!poll_body.contains("change_current_state"));
    for source in [RUN, EVENT_HANDLER] {
        assert!(!source.contains("refresh_rate_millihz = observed"));
    }
}

#[test]
fn oneplus_pad_3_physical_bounds_and_coordinate_clamping() {
    // OnePlus Pad 3 has a 3392 x 2400 physical display at 144Hz
    let (w, h) = (3392, 2400);
    assert_eq!(physical_window_size(w, h), Some((3392, 2400)));
    assert_eq!(refresh_period_nanos(144_000), 6_944_444);

    // In-bounds touch coordinates pass through unaltered
    assert_eq!(clamp_physical_coordinate(0.0, w), 0.0);
    assert_eq!(clamp_physical_coordinate(3392.0, w), 3392.0);
    assert_eq!(clamp_physical_coordinate(1696.0, w), 1696.0);
    assert_eq!(clamp_physical_coordinate(0.0, h), 0.0);
    assert_eq!(clamp_physical_coordinate(2400.0, h), 2400.0);
    assert_eq!(clamp_physical_coordinate(1200.0, h), 1200.0);

    // Out-of-bounds coordinates (e.g. touches beyond display or edge gestures) are clamped
    assert_eq!(clamp_physical_coordinate(-25.0, w), 0.0);
    assert_eq!(clamp_physical_coordinate(3500.0, w), 3392.0);
    assert_eq!(clamp_physical_coordinate(-1.0, h), 0.0);
    assert_eq!(clamp_physical_coordinate(2450.0, h), 2400.0);

    // Malformed coordinates (NaN / Inf) are clamped safely to 0
    assert_eq!(clamp_physical_coordinate(f64::NAN, w), 0.0);
    assert_eq!(clamp_physical_coordinate(f64::INFINITY, w), 0.0);
    assert_eq!(clamp_physical_coordinate(f64::NEG_INFINITY, h), 0.0);
}

#[test]
fn tablet_native_resolution_and_density_conversions() {
    let scale = density_scale_factor(300);
    assert_eq!(physical_window_size(3392, 2400), Some((3392, 2400)));
    assert_eq!(xft_dpi(scale), 180);
    assert_eq!(qt_scale_factor(scale), "1.875");
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
fn software_keyboard_tab_and_terminal_shortcuts() {
    // Tab key is essential for terminal completion in Konsole
    assert_eq!(committed_ascii_to_key_events("\t"), vec![(15, false)]);
    assert_eq!(
        committed_ascii_to_key_events("cd /sdcard\t\n"),
        vec![
            (46, false), // c
            (32, false), // d
            (57, false), // space
            (53, false), // /
            (31, false), // s
            (32, false), // d
            (46, false), // c
            (30, false), // a
            (19, false), // r
            (32, false), // d
            (15, false), // tab
            (28, false), // enter
        ]
    );
}

#[test]
fn software_keyboard_mixed_and_edge_case_commits() {
    // Unsupported Unicode characters (e.g. CJK, emoji) are dropped without crashing,
    // while the valid ASCII characters are cleanly extracted
    assert_eq!(
        committed_ascii_to_key_events("echo 🦀 > out.txt\r\n"),
        vec![
            (18, false), // e
            (46, false), // c
            (35, false), // h
            (24, false), // o
            (57, false), // space
            (57, false), // space (after dropped emoji)
            (52, true),  // > (shift + .)
            (57, false), // space
            (24, false), // o
            (22, false), // u
            (20, false), // t
            (52, false), // .
            (20, false), // t
            (45, false), // x
            (20, false), // t
            (28, false), // \r
            (28, false), // \n
        ]
    );
    assert!(committed_ascii_to_key_events("").is_empty());
    assert!(committed_ascii_to_key_events("你好世界").is_empty());
}

#[test]
fn software_keyboard_delete_commit_maps_to_backspace() {
    assert_eq!(committed_ascii_to_key_events("\u{8}"), vec![(14, false)]);
}

#[test]
fn nested_kwin_text_input_uses_protocol_commits_and_authoritative_hotplug() {
    assert!(ANDROID_TEXT_INPUT_V2_SOURCE.contains("ZwpTextInputManagerV2"));
    assert!(ANDROID_TEXT_INPUT_V2_SOURCE.contains("input.commit_string"));
    assert!(ANDROID_TEXT_INPUT_V2_SOURCE.contains("input.delete_surrounding_text"));
    assert!(ANDROID_KEYBOARD_BRIDGE_SOURCE.contains("InputManager.InputDeviceListener"));
    assert!(ANDROID_KEYBOARD_BRIDGE_SOURCE.contains("InputDevice.KEYBOARD_TYPE_ALPHABETIC"));
    assert!(ANDROID_KEYBOARD_BRIDGE_SOURCE.contains("device.isExternal()"));
    assert!(!ANDROID_KEYBOARD_BRIDGE_SOURCE.contains("OnePlus Pad 3 Keyboard"));
}

#[test]
fn nested_android_owned_settings_are_truthful() {
    // Firefox must use its normal GTK/Wayland client-side chrome; no Portal
    // titlebar workaround may force a KWin/Breeze server-side decoration.
    assert!(!ANDROID_SETUP_SOURCE.contains("browser.tabs.inTitlebar"));
    assert!(ANDROID_SETUP_SOURCE.contains("sync_firefox_config"));
    // Sandbox/audio/runtime compatibility prefs must remain.
    assert!(ANDROID_SETUP_SOURCE.contains("media.cubeb.sandbox"));
    assert!(ANDROID_SETUP_SOURCE.contains("security.sandbox.content.level"));
    assert!(ANDROID_SETUP_SOURCE.contains("get_timezone_id()"));
    assert!(ANDROID_SETUP_SOURCE.contains("systemsettings/kcm_touchscreen.so"));
    assert!(ANDROID_SETUP_SOURCE.contains("systemsettings/kcm_tablet.so"));
    assert!(ANDROID_SETUP_SOURCE.contains("systemsettings/kcm_mouse.so"));
    assert!(ANDROID_SETUP_SOURCE.contains("Failed to restore Portal touchpad settings module"));
    assert!(ANDROID_SETUP_SOURCE.contains("kwin-debian-arm64/libkwin.so.6.3.6"));
    assert!(ANDROID_SETUP_SOURCE.contains("systemsettings_qwidgets/kcm_clock.so"));
    assert!(ANDROID_SETUP_SOURCE.contains("with_extension(\"so.portal-disabled\")"));
    assert!(ANDROID_SETUP_SOURCE.contains("org.kde.dolphin.desktop"));
    assert!(ANDROID_SETUP_SOURCE.contains("LocalDesktop.profile"));
    assert!(ANDROID_SETUP_SOURCE.contains("konsole-profile-v2"));
    assert!(ANDROID_SETUP_SOURCE.contains("sync_debian_package_management"));
    assert!(ANDROID_SETUP_SOURCE.contains("APT::Sandbox::User \\\"root\\\""));
    assert!(ANDROID_SETUP_SOURCE.contains("policy-rc.d"));
    assert!(ANDROID_SETUP_SOURCE.contains("exit 101"));
    assert!(ANDROID_SETUP_SOURCE.contains("var/lib/dpkg/info"));
    assert!(ANDROID_SETUP_SOURCE.contains("format"));
}

#[test]
fn debian_package_management_and_tablet_mode_policy() {
    const PLASMA_LAUNCHER_SOURCE: &str = include_str!("../assets/localdesktop-startplasma.sh");
    assert!(!PLASMA_LAUNCHER_SOURCE.contains("TabletMode auto"));
    assert!(PLASMA_LAUNCHER_SOURCE.contains("TabletMode off"));
    assert!(PLASMA_LAUNCHER_SOURCE.contains("update-mime-database"));
    assert!(PLASMA_LAUNCHER_SOURCE.contains("update-desktop-database"));
    assert!(PLASMA_LAUNCHER_SOURCE.contains("kbuildsycoca6 --noincremental"));
}

#[test]
fn ibus_autostart_never_blocks_session_startup() {
    // No package management anywhere on the splash->desktop path.
    assert!(!STARTPLASMA_SOURCE.contains("apt-get"));
    // The autostart entry delegates to the lazy launcher (returns in
    // milliseconds), starts after the panel, and carries no fixed sleep.
    assert!(STARTPLASMA_SOURCE.contains("Exec=/usr/local/bin/portal-ibus-lazy"));
    assert!(STARTPLASMA_SOURCE.contains("X-KDE-autostart-after=panel"));
    assert!(!STARTPLASMA_SOURCE.contains("sleep 4; ibus engine portal"));
    // The lazy launcher detaches all work with bounded waits: no apt-get,
    // no blocking sleep on the critical path.
    assert!(PORTAL_IBUS_LAZY_SOURCE.contains(") >/dev/null 2>&1 < /dev/null &"));
    assert!(PORTAL_IBUS_LAZY_SOURCE.contains("exit 0"));
    assert!(!PORTAL_IBUS_LAZY_SOURCE.contains("apt-get"));
    assert!(!PORTAL_IBUS_LAZY_SOURCE.contains("sleep 4"));
    // Setup provisions IBus packages pre-session (detached, marker-gated)
    // and deploys the lazy launcher into the guest.
    assert!(ANDROID_SETUP_SOURCE.contains("provision_ibus_packages"));
    assert!(ANDROID_SETUP_SOURCE.contains("ibus-provisioned-v1"));
    assert!(ANDROID_SETUP_SOURCE.contains("usr/local/bin/portal-ibus-lazy"));
    assert!(ANDROID_SETUP_SOURCE.contains("PORTAL_IBUS_LAZY"));
}

#[test]
fn anland_hardware_acceleration_is_default_with_explicit_software_fallback() {
    // The production Anland path is hardware-accelerated: the default
    // session env must not force software rendering, the Mesa layer binds
    // track the pinned 26.3 layer, and the emergency software fallback is
    // an explicit guest flag file mirrored host-side (never silent).
    assert!(ANLAND_ENV_SOURCE.contains("software_gl_fallback_requested"));
    assert!(ANLAND_ENV_SOURCE.contains("kwin-glmode"));
    assert!(ANLAND_ENV_SOURCE.contains("libgallium-26.3.0-devel.so"));
    assert!(!ANLAND_ENV_SOURCE.contains("libgallium-26.2.0-devel.so"));
    // No software-forcing default in the session environment: the env
    // constructor body (up to the next item) must be free of it; docs and
    // fallback plumbing elsewhere may still mention it.
    let env_fn = ANLAND_ENV_SOURCE
        .find("pub fn guest_mesa_env()")
        .expect("guest_mesa_env must exist");
    let next_item = ANLAND_ENV_SOURCE[env_fn..]
        .find("\npub fn ")
        .expect("item after guest_mesa_env")
        + env_fn;
    assert!(
        !ANLAND_ENV_SOURCE[env_fn..next_item].contains("ANLAND_NO_DRM_DEVICE"),
        "hardware must be the default session environment"
    );
}

#[test]
fn mesa_kgsl_layer_is_pinned_and_matches_session_binds() {
    // The kgsl winsys lives only in the lfdevs Mesa layer (stock Mesa has
    // no kgsl winsys at all), so the layer is downloaded once, verified
    // (size + SHA-256, fail closed), and subset-extracted. The extraction
    // set must stay identical to what session_binds mounts, or the layer
    // silently stops engaging.
    for pin in [
        "mesa-for-android-container_26.3.0-devel-20260824_debian_trixie_arm64.tar.gz",
        "11648933",
        "c014cf66bdbff96417ee30d34f006cf51df64ae04893d599711b0b6b73b52ccf",
        "mesa-kgsl-layer.complete",
        "libgallium-26.3.0-devel.so",
        "libgbm.so.1",
        "libEGL_mesa.so.0",
        "is_provisioned",
    ] {
        assert!(
            MESA_LAYER_SOURCE.contains(pin),
            "mesa layer source of truth missing: {pin}"
        );
    }
    for path in [
        "libgallium-26.3.0-devel.so",
        "libgbm.so.1",
        "libEGL_mesa.so.0",
        "libGLX_mesa.so.0",
    ] {
        assert!(
            ANLAND_ENV_SOURCE.contains(path),
            "session binds must mount the pinned layer file: {path}"
        );
    }
}

#[test]
fn anland_session_forces_wayland_qpa_for_plasma_clients() {
    // On the Anland path, Plasma/KDE Qt clients must select the Wayland
    // backend explicitly; otherwise ksmserver/plasmashell can fall back to
    // xcb, fail to start, and plasma_session waits forever for
    // org.kde.ksmserver. DISPLAY stays set for XWayland/Firefox (X11).
    assert!(STARTPLASMA_SOURCE.contains("QT_QPA_PLATFORM=wayland"));
}

#[test]
fn automatic_tablet_and_laptop_mode_switching_policy() {
    use tablet_mode::{
        is_desktop_pointer, is_physical_alphabetic_keyboard, InputDeviceDescriptor,
        SystemInputState, KEYBOARD_TYPE_ALPHABETIC, KEYBOARD_TYPE_NONE,
        KEYBOARD_TYPE_NON_ALPHABETIC, SOURCE_KEYBOARD, SOURCE_MOUSE, SOURCE_TOUCHPAD,
        SOURCE_TOUCHSCREEN,
    };

    // 1. External physical alphabetic keyboard -> desktop mode, IME suppressed
    let ext_keyboard =
        InputDeviceDescriptor::new(true, false, SOURCE_KEYBOARD, KEYBOARD_TYPE_ALPHABETIC);
    assert!(is_physical_alphabetic_keyboard(&ext_keyboard));
    assert!(!is_desktop_pointer(&ext_keyboard));
    let kb_state = SystemInputState::evaluate([&ext_keyboard]);
    assert!(kb_state.physical_keyboard_present);
    assert!(kb_state.desktop_input_present);
    assert_eq!(kb_state.kwin_tablet_mode(), "off");
    assert!(kb_state.should_suppress_soft_keyboard());

    // 2. External pointer/touchpad -> desktop mode, IME NOT suppressed
    let ext_touchpad = InputDeviceDescriptor::new(true, false, SOURCE_TOUCHPAD, KEYBOARD_TYPE_NONE);
    assert!(!is_physical_alphabetic_keyboard(&ext_touchpad));
    assert!(is_desktop_pointer(&ext_touchpad));
    let tp_state = SystemInputState::evaluate([&ext_touchpad]);
    assert!(!tp_state.physical_keyboard_present);
    assert!(tp_state.desktop_input_present);
    assert_eq!(tp_state.kwin_tablet_mode(), "off");
    assert!(!tp_state.should_suppress_soft_keyboard());

    let ext_mouse = InputDeviceDescriptor::new(true, false, SOURCE_MOUSE, KEYBOARD_TYPE_NONE);
    assert!(is_desktop_pointer(&ext_mouse));
    let mouse_state = SystemInputState::evaluate([&ext_mouse]);
    assert!(mouse_state.desktop_input_present);
    assert_eq!(mouse_state.kwin_tablet_mode(), "off");
    assert!(!mouse_state.should_suppress_soft_keyboard());

    // 3. Internal tablet touchscreen alone -> tablet mode, IME NOT suppressed
    let touchpanel =
        InputDeviceDescriptor::new(false, false, SOURCE_TOUCHSCREEN, KEYBOARD_TYPE_NONE);
    assert!(!is_physical_alphabetic_keyboard(&touchpanel));
    assert!(!is_desktop_pointer(&touchpanel));
    let touch_state = SystemInputState::evaluate([&touchpanel]);
    assert!(!touch_state.physical_keyboard_present);
    assert!(!touch_state.desktop_input_present);
    assert_eq!(touch_state.kwin_tablet_mode(), "on");
    assert!(!touch_state.should_suppress_soft_keyboard());

    // 4. Internal non-alphabetic keys (gpio-keys, power, volume) -> tablet mode
    let power_key =
        InputDeviceDescriptor::new(false, false, SOURCE_KEYBOARD, KEYBOARD_TYPE_NON_ALPHABETIC);
    let gpio_keys =
        InputDeviceDescriptor::new(false, false, SOURCE_KEYBOARD, KEYBOARD_TYPE_NON_ALPHABETIC);
    let internal_state = SystemInputState::evaluate([&touchpanel, &power_key, &gpio_keys]);
    assert!(!internal_state.physical_keyboard_present);
    assert!(!internal_state.desktop_input_present);
    assert_eq!(internal_state.kwin_tablet_mode(), "on");
    assert!(!internal_state.should_suppress_soft_keyboard());

    // 5. Combined OnePlus Pad keyboard case (keyboard + touchpad) attached -> desktop mode
    let attached_state = SystemInputState::evaluate([&touchpanel, &ext_keyboard, &ext_touchpad]);
    assert!(attached_state.physical_keyboard_present);
    assert!(attached_state.desktop_input_present);
    assert_eq!(attached_state.kwin_tablet_mode(), "off");
    assert!(attached_state.should_suppress_soft_keyboard());

    // 6. Detached keyboard case -> transitions to tablet mode
    let detached_state = SystemInputState::evaluate([&touchpanel]);
    assert!(!detached_state.physical_keyboard_present);
    assert!(!detached_state.desktop_input_present);
    assert_eq!(detached_state.kwin_tablet_mode(), "on");
    assert!(!detached_state.should_suppress_soft_keyboard());
}

#[test]
fn soft_keyboard_bridge_publishes_both_states_without_device_name_heuristics() {
    assert!(ANDROID_KEYBOARD_BRIDGE_SOURCE.contains("hasDesktopInput"));
    assert!(ANDROID_KEYBOARD_BRIDGE_SOURCE.contains("nativeOnInputDevicesChanged"));
    assert!(ANDROID_KEYBOARD_BRIDGE_SOURCE.contains("InputDevice.SOURCE_MOUSE"));
    assert!(ANDROID_KEYBOARD_BRIDGE_SOURCE.contains("InputDevice.SOURCE_TOUCHPAD"));
    assert!(!ANDROID_KEYBOARD_BRIDGE_SOURCE.contains("OnePlus Pad 3 Keyboard"));
    assert!(!ANDROID_KEYBOARD_BRIDGE_SOURCE.contains("pogo_touchpad"));
}

#[test]
fn clipboard_bridge_accepts_text_only_and_prefers_utf8() {
    assert!(supports_mime_type(TEXT_MIME));
    assert!(supports_mime_type(UTF8_TEXT_MIME));
    assert!(supports_mime_type("text/plain; charset=utf-8"));
    assert!(supports_mime_type("UTF8_STRING"));
    assert!(supports_mime_type("STRING"));
    assert!(supports_mime_type("TEXT"));
    assert!(!supports_mime_type("text/html"));
    assert!(!supports_mime_type("application/octet-stream"));
    assert_eq!(
        choose_text_mime(["text/html", TEXT_MIME, UTF8_TEXT_MIME]),
        Some(UTF8_TEXT_MIME)
    );
    assert_eq!(
        choose_text_mime(["text/plain", "text/plain; charset=utf-8"]),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(
        choose_text_mime(["STRING", "UTF8_STRING"]),
        Some("UTF8_STRING")
    );
}

#[test]
fn clipboard_bridge_ignores_empty_or_invalid_clips() {
    assert!(!is_valid_clip_text(""));
    assert_eq!(validate_clip_text(""), None);

    let valid_text = "Portal Wayland clipboard content";
    assert!(is_valid_clip_text(valid_text));
    assert_eq!(validate_clip_text(valid_text), Some(valid_text));

    let oversized = "a".repeat(MAX_CLIPBOARD_BYTES + 1);
    assert!(!is_valid_clip_text(&oversized));
    assert_eq!(validate_clip_text(&oversized), None);
}

#[test]
fn android_clipboard_path_applies_byte_limit_before_wayland_selection() {
    let read_path = ANDROID_CLIPBOARD_SOURCE
        .split_once("fn read_text_inner")
        .map(|(_, body)| body)
        .expect("Android clipboard read path is present");
    assert!(read_path.contains("validate_clip_text(&text)"));
    assert!(read_path.contains("MAX_CLIPBOARD_BYTES"));
    assert!(read_path.contains("coerceToText"));
    assert!(read_path.contains("text.is_empty()"));

    let process_path = ANDROID_COMPOSITOR_SOURCE
        .split_once("pub fn process_android_clipboard")
        .map(|(_, body)| body)
        .expect("Android clipboard compositor path is present");
    assert!(process_path.contains("publish_android_clipboard(value.as_deref())"));
    assert!(!process_path.contains("set_data_device_selection"));
}

#[test]
fn input_method_bridge_and_fallback_policy() {
    // 1. Setup installs portal-ime-bridge and portal-ime.desktop
    assert!(ANDROID_SETUP_SOURCE.contains("usr/local/bin/portal-ime-bridge"));
    assert!(ANDROID_SETUP_SOURCE.contains("usr/share/applications/portal-ime.desktop"));

    // 1b. Setup syncs the Anland unified libkwin to its own dir every launch.
    assert!(ANDROID_SETUP_SOURCE.contains("usr/local/lib/portal-anland"));

    // 2. KWin wrapper passes --inputmethod to launch portal-ime-bridge
    assert!(KWIN_WRAPPER_SOURCE.contains("--inputmethod /usr/local/bin/portal-ime-bridge"));

    // 2b. Anland sessions load the unified Anland libkwin (Anland backend +
    // Portal Touchpad) from its own dir; the stub is never preloaded there.
    assert!(KWIN_WRAPPER_SOURCE.contains("/usr/local/lib/portal-anland"));
    // 2c. Deterministic KWin A/B without APK rebuilds: kwin-variant selects
    // unified/stock/ab candidates, every run logs SHA256 attributions, and
    // failed sanity checks fall back to stock (never unloadable).
    assert!(KWIN_WRAPPER_SOURCE.contains("kwin-variant"));
    assert!(KWIN_WRAPPER_SOURCE.contains("sha256sum"));
    assert!(KWIN_WRAPPER_SOURCE.contains("falling back to stock"));
    // 2c-ii. Canonical default is the unified Portal lib (Anland backend
    // plus the physically validated Portal Touchpad input path); stock
    // stays selectable for A/B/recovery. Pinned as the exact
    // default-assignment block (fallbacks elsewhere reuse similar words,
    // so a bare substring would prove nothing).
    assert!(KWIN_WRAPPER_SOURCE.contains(
        "kwin_variant=unified\n    if [ -r /var/lib/localdesktop/kwin-variant ]"
    ));
    // 2d. --anland is probe-gated: stock distro kwin_wayland exits(1) on
    // unknown options (compositor restart loop), so the flag is only passed
    // when the binary advertises it; env + unified libkwin otherwise.
    assert!(KWIN_WRAPPER_SOURCE.contains("grep -q"));
    assert!(KWIN_WRAPPER_SOURCE.contains("binary lacks --anland"));
    // 2e. Renderer mode defaults to hardware acceleration; the emergency
    // software fallback is an explicit guest flag file (kwin-glmode == sw)
    // that forces surfaceless software rendering for KWin only. Forcing
    // freedreno inside the software EGL stack demands the KGSL winsys and
    // kills EGL init (proven by matrix), so the fallback also drops
    // GALLIUM_DRIVER for kwin_wayland.
    assert!(KWIN_WRAPPER_SOURCE.contains("kwin-glmode"));
    assert!(KWIN_WRAPPER_SOURCE.contains("ANLAND_NO_DRM_DEVICE=1"));
    assert!(KWIN_WRAPPER_SOURCE.contains("unset GALLIUM_DRIVER"));
    assert!(KWIN_WRAPPER_SOURCE.contains("unset ANLAND_NO_DRM_DEVICE"));

    // 3. Startplasma sets kwinrc InputMethod and VirtualKeyboardMode
    assert!(STARTPLASMA_SOURCE.contains("InputMethod=/usr/share/applications/portal-ime.desktop"));
    assert!(STARTPLASMA_SOURCE.contains("VirtualKeyboardMode=1"));

    // 4. Portal IME Bridge speaks zwp_input_method_v1 with commit_string (1) and delete_surrounding_text (5)
    assert!(PORTAL_IME_BRIDGE_SOURCE.contains("zwp_input_method_v1"));
    assert!(PORTAL_IME_BRIDGE_SOURCE.contains("active_context_id, (req_size << 16) | 1"));
    assert!(PORTAL_IME_BRIDGE_SOURCE.contains("active_context_id, (req_size << 16) | 5"));
    assert!(PORTAL_IME_BRIDGE_SOURCE.contains("/tmp/portal-ime-events.fifo"));
    assert!(PORTAL_IME_BRIDGE_SOURCE.contains("/tmp/portal-ime-commands.fifo"));

    // 5. Host IME dispatch prioritizes protocol when active, and only falls back to evdev when unready/inactive
    assert!(ANDROID_IME_SOURCE.contains("is_ime_context_active()"));
    assert!(ANDROID_IME_SOURCE.contains("send_ime_command(&format!(\"DELETE:{count}\\n\"))"));
    assert!(ANDROID_IME_SOURCE.contains("send_ime_command(\"ENTER\\n\")"));
    assert!(ANDROID_IME_SOURCE.contains("send_ime_command(&format!(\"COMMIT:{b64}\\n\"))"));
    // 5b. X11/GTK commits (Firefox) route through the Portal IBus engine on
    // the same active flag: dual send, each bridge self-gates on real focus.
    assert!(ANDROID_IME_SOURCE.contains("send_engine_delete(count)"));
    assert!(ANDROID_IME_SOURCE.contains("send_engine_enter()"));
    assert!(ANDROID_IME_SOURCE.contains("send_engine_text(&text)"));
    assert!(PORTAL_IME_BRIDGE_SOURCE.contains("send_enter()"));
    assert!(PORTAL_IME_BRIDGE_SOURCE.contains("0xff0d"));
    assert!(ANDROID_IME_SOURCE.contains("Falling back to evdev key synthesis"));
    assert!(ANDROID_IME_SOURCE.contains("start_ime_fifo_listener"));

    // 6. SoftKeyboardBridge handles text commit, backspace, and action down
    assert!(ANDROID_KEYBOARD_BRIDGE_SOURCE.contains("nativeOnTextCommit"));
    assert!(ANDROID_KEYBOARD_BRIDGE_SOURCE.contains("commitText"));
    assert!(ANDROID_KEYBOARD_BRIDGE_SOURCE.contains("deleteSurroundingText"));
}

#[test]
fn xwayland_variant_selection_policy() {
    // 1. Setup stages the Phase B candidate + xinput into their own dir
    // (never /usr/bin, never the loader path) on every launch.
    assert!(ANDROID_SETUP_SOURCE.contains("usr/local/lib/portal-xwayland"));
    assert!(ANDROID_SETUP_SOURCE.contains("sync_xwayland_candidate_overlay"));
    // 2. The staged candidate SHA is pinned identically in setup.rs, the
    // guest-side selection gate, and the asset manifest (transcription
    // slips fail closed here, not on the tablet).
    for pinned in [
        "b91f55794942a9efd66300e352cea8c671cdb6ff0c3243a0cc176525cb96812d",
    ] {
        assert!(ANDROID_SETUP_SOURCE.contains(pinned));
        assert!(KWIN_WRAPPER_SOURCE.contains(pinned));
        assert!(XWAYLAND_SHA256SUMS_SOURCE.contains(pinned));
    }
    // 3. files/xwayland-variant reaches the guest as an explicit env var.
    assert!(ANLAND_ENV_SOURCE.contains("LOCALDESKTOP_XWAYLAND_VARIANT"));
    assert!(ANLAND_ENV_SOURCE.contains("xwayland-variant"));
    // 4. Wrapper default is stock, pinned as the exact assignment block so a
    // bare substring (also used by fallback lines) proves nothing.
    assert!(KWIN_WRAPPER_SOURCE.contains(
        "xwayland_variant=stock\n    case \"${LOCALDESKTOP_XWAYLAND_VARIANT:-}\" in"
    ));
    // 5. Every staging failure falls back to stock; stock never touches PATH.
    assert!(KWIN_WRAPPER_SOURCE.contains("status=rejected-bad-sha falling back to stock"));
    assert!(KWIN_WRAPPER_SOURCE.contains("status=incomplete-staging falling back to stock"));
    assert!(KWIN_WRAPPER_SOURCE.contains("PATH=\"$xwayland_cand_dir:$PATH\""));
    // 6. Runs are attributed (which binary KWin actually resolved + SHAs).
    assert!(KWIN_WRAPPER_SOURCE.contains("resolved="));
    // 7. The XI2 proof harness is inert without its explicit flag file and
    // resolves the touchpad device by property, never by name alone.
    assert!(KWIN_WRAPPER_SOURCE.contains("xwayland-xinput-probe"));
    assert!(KWIN_WRAPPER_SOURCE.contains("xwayland-xinput.log"));
    assert!(KWIN_WRAPPER_SOURCE.contains("libinput Tapping Enabled"));
    // 7b. The probe authenticates like KWin's own X clients: display and
    // cookie come from our argv (--xwayland-display/--xwayland-xauthority),
    // with a bounded socket scan only as fallback.
    assert!(KWIN_WRAPPER_SOURCE.contains("--xwayland-xauthority"));
}
