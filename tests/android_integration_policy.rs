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
const RENDERER_POLICY_SOURCE: &str = include_str!("../src/core/renderer_policy.rs");
const MESA_LAYER_SOURCE: &str = include_str!("../src/android/proot/mesa_layer.rs");
const ANDROID_SETUP_RUN_SOURCE: &str = include_str!("../src/android/app/run.rs");
const ANLAND_CONSUMER_SOURCE: &str = include_str!("../src/android/anland/consumer.rs");
const ANLAND_SYS_SOURCE: &str = include_str!("../src/android/anland/sys.rs");
const ANLAND_BROKER_SOURCE: &str = include_str!("../src/android/anland/broker.rs");
const ANLAND_PROTOCOL_SOURCE: &str = include_str!("../src/android/anland/protocol.rs");
const KWIN_ANLAND_PROTOCOL_SOURCE: &str =
    include_str!("../patches/kwin/anland-6.7.4/src/backends/anland/protocol.h");
const KWIN_ANLAND_BACKEND_SOURCE: &str =
    include_str!("../patches/kwin/anland-6.7.4/src/backends/anland/anland_backend.cpp");
const ANLAND_INPUT_SOURCE: &str =
    include_str!("../patches/kwin/anland-6.7.4/src/backends/anland/anland_input.cpp");
const ANLAND_INPUT_HEADER_SOURCE: &str =
    include_str!("../patches/kwin/anland-6.7.4/src/backends/anland/anland_input.h");
const ANLAND_EVENT_HANDLER_SOURCE: &str =
    include_str!("../src/android/backend/wayland/event_handler.rs");
const COMPOSE_OVERLAY_RUST_SOURCE: &str = include_str!("../src/android/utils/compose_overlay.rs");
const COMPOSE_OVERLAY_KOTLIN_SOURCE: &str =
    include_str!("../src/android/kotlin/app/polarbear/ComposeOverlay.kt");
const PORTAL_RETURN_SCREEN_SOURCE: &str =
    include_str!("../src/android/kotlin/app/polarbear/setup/PortalReturnScreen.kt");
const PORTAL_LAUNCH_TRANSITION_SOURCE: &str =
    include_str!("../src/android/kotlin/app/polarbear/setup/PortalLaunchTransition.kt");

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
    assert!(ANDROID_SETUP_SOURCE.contains("kwin-forky-anland-arm64/kwin_wayland"));
    assert!(ANDROID_SETUP_SOURCE.contains("kwin-forky-anland-arm64/libkwin.so.6.7.4"));
    assert!(!ANDROID_SETUP_SOURCE.contains("kwin-debian-arm64/libkwin.so.6.3.6"));
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
fn anland_hardware_acceleration_is_the_only_active_graphics_path() {
    // The production Anland path is hardware-accelerated. It uses KGSL-backed
    // surfaceless EGL because the Android app UID cannot open /dev/dri; the
    // old software/QPainter graphics stack is not active.
    assert!(ANLAND_ENV_SOURCE.contains("libgallium-26.3.0-devel.so"));
    assert!(!ANLAND_ENV_SOURCE.contains("libgallium-26.2.0-devel.so"));
    assert!(KWIN_WRAPPER_SOURCE.contains("kwin-glmode"));
    assert!(KWIN_WRAPPER_SOURCE.contains("mode=sw"));
    assert!(KWIN_WRAPPER_SOURCE.contains("LOCALDESKTOP_KWIN_GL_DEBUG"));
    assert!(KWIN_WRAPPER_SOURCE.contains("unset KWIN_GL_DEBUG"));
    assert!(KWIN_WRAPPER_SOURCE.contains("export ANLAND_NO_DRM_DEVICE=1"));
    assert!(!KWIN_WRAPPER_SOURCE.contains("unset ANLAND_NO_DRM_DEVICE"));
    // No software-forcing default in the session environment: the env
    // constructor body (up to the next item) must be free of it. The explicit
    // troubleshooting marker is interpreted only by the KWin wrapper and
    // mirrored by the Anland consumer.
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
        "mesa-for-android-container_26.3.0-devel-20260824_ubuntu_resolute_arm64.tar.gz",
        "12069639",
        "ee762f0855c47f9a245df3ce53a46d40b2240ede9b5c9ddf7606e724362fec77",
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
fn anland_presentation_is_work_driven_and_vsync_cannot_invent_work() {
    // A producer request is the only presentation source. The Android VSYNC
    // fd remains available for optional timeline telemetry, while the control
    // wake can authorize rendering as soon as FRAME_WANTED is accepted.
    assert!(ANLAND_CONSUMER_SOURCE.contains("sys::poll_two(&tick, &wake, -1)"));
    assert!(ANLAND_CONSUMER_SOURCE.contains("if !tick_ready && !wake_ready"));
    assert!(ANLAND_CONSUMER_SOURCE.contains("FRAME_WANTED"));
    assert!(ANLAND_CONSUMER_SOURCE.contains("inner.work.lock().unwrap().take(cur_gen)"));
    assert!(ANLAND_CONSUMER_SOURCE.contains("work-driven"));
    assert!(ANLAND_CONSUMER_SOURCE.contains("vsync_telemetry={tick_ready}"));
    assert!(ANLAND_CONSUMER_SOURCE.contains("pump.request()"));
    assert!(ANLAND_SYS_SOURCE.contains("VsyncPump::request"));
    assert!(ANLAND_SYS_SOURCE.contains("ALooper_addFd"));
    assert!(ANLAND_SYS_SOURCE.contains("request_pending"));
    assert!(ANLAND_SYS_SOURCE.contains("advance_timer_deadline"));
    assert!(!ANLAND_SYS_SOURCE.contains("pending_requests"));
    assert!(!ANLAND_SYS_SOURCE.contains("if !callback_pending.load(Ordering::Acquire)"));

    // Static desktop: no producer request means no dequeue/select/queue cycle,
    // regardless of how many display ticks arrive.
    assert!(ANLAND_CONSUMER_SOURCE.contains("no FRAME_WANTED"));

    // No CPU heuristic, burst deadline, heartbeat, or self-sustaining demand
    // gate may decide whether KWin gets another buffer.
    for obsolete in [
        "demand_until_ns",
        "HEARTBEAT_NS",
        "sample_client_activity",
        "proc_jiffies",
        "INPUT_BURST_MS",
        "SELF_SUSTAIN_MS",
        "demanding",
    ] {
        assert!(
            !ANLAND_CONSUMER_SOURCE.contains(obsolete),
            "obsolete Anland demand gate remains: {obsolete}"
        );
    }
}

#[test]
fn anland_diagnostics_cover_demand_damage_and_pacing() {
    // Inexpensive Stable counters plus Debug detail must exist for every
    // load-bearing outcome: requests, renders, queues, NO_DAMAGE, drops,
    // timeouts, deadline misses, generations, and stage latencies. Noisy
    // per-frame info logging is forbidden; the 10s alive line carries Stable.
    for counter in [
        "work_requested",
        "work_rejected",
        "work_consumed",
        "frames_no_damage",
        "unknown_slot_cancels",
        "gen_mismatch_cancels",
        "stale_gen_cancels",
        "dequeue_failures",
        "acquire_timeouts",
        "render_timeouts",
        "queue_failures",
        "cancel_failures",
        "timeline_hit",
        "timeline_miss",
        "deadline_misses",
        "rebinds_completed",
        "dequeue_us_total",
        "acquire_us_total",
        "render_us_total",
        "anland.render alive",
        "anland.lat",
        "anland.timeline",
    ] {
        assert!(
            ANLAND_CONSUMER_SOURCE.contains(counter),
            "Anland diagnostics are missing Stable/Debug coverage for: {counter}"
        );
    }
    assert!(ANLAND_CONSUMER_SOURCE.contains("pub struct AnlandDiagnostics"));
    assert!(ANLAND_CONSUMER_SOURCE.contains("pub fn diagnostics"));
}

#[test]
fn anland_finger_scroll_protocol_matches_kwin_backend() {
    // The Android sender and the Forky KWin producer must agree on the
    // finger-scroll wire contract; otherwise touchpad scroll is silently
    // dropped on the default path.
    for token in [
        "INPUT_TYPE_POINTER_AXIS_FINGER",
        "INPUT_TYPE_POINTER_AXIS_STOP",
    ] {
        assert!(
            ANLAND_PROTOCOL_SOURCE.contains(token),
            "Rust Anland protocol is missing {token}"
        );
        assert!(
            KWIN_ANLAND_PROTOCOL_SOURCE.contains(token),
            "KWin Anland protocol.h is missing {token}"
        );
    }
    for token in ["pointerAxisFinger", "pointerAxisStop"] {
        assert!(
            KWIN_ANLAND_BACKEND_SOURCE.contains(token),
            "KWin Anland backend does not handle finger scroll ({token})"
        );
    }
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
    // 2c. The active launcher always uses the Portal-built Forky KWin and
    // passes the Anland backend flag; there is no compositor A/B selector.
    assert!(KWIN_WRAPPER_SOURCE.contains("kwin_anland_dir=/usr/local/lib/portal-anland"));
    assert!(KWIN_WRAPPER_SOURCE.contains("set -- \"$@\" --anland"));
    assert!(!KWIN_WRAPPER_SOURCE.contains("kwin-variant"));
    assert!(!KWIN_WRAPPER_SOURCE.contains("falling back to stock"));
    assert!(KWIN_WRAPPER_SOURCE.contains("export ANLAND_NO_DRM_DEVICE=1"));

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
fn stock_xwayland_and_native_firefox_policy() {
    // XWayland stays the Forky Debian package with the KGSL surfaceless
    // forward-port (child of KWin, inherits the wrapper force flag); no
    // Portal-specific candidate/A-B selector is active. Firefox is forced
    // back through XWayland (MOZ_ENABLE_WAYLAND=0 + XInput2 + WR prefs) —
    // the known-good GPU path from `main`.
    assert!(KWIN_WRAPPER_SOURCE.contains("XWayland stays the Debian package"));
    assert!(ANLAND_ENV_SOURCE.contains("MOZ_ENABLE_WAYLAND"));
    assert!(ANLAND_ENV_SOURCE.contains("MOZ_USE_XINPUT2"));
    assert!(!ANLAND_ENV_SOURCE.contains("LOCALDESKTOP_XWAYLAND_VARIANT"));
    assert!(!ANDROID_SETUP_SOURCE.contains("sync_xwayland_candidate_overlay"));
    assert!(ANDROID_SETUP_SOURCE.contains("Firefox Portal GPU preference is missing"));
}

#[test]
fn anland_touchpad_exposes_scroll_settings() {
    // The Portal touchpad must be recognizable as a real touchpad with
    // NaturalScroll/ScrollFactor over the standard KWin input-device D-Bus
    // path, persisted in kcminputrc and applied to finger scroll only.
    for required in [
        "Portal Touchpad",
        "portal_touchpad",
        "org.kde.KWin.InputDevice",
        "org.kde.KWin.InputDeviceManager",
        "kcminputrc",
        "NaturalScroll",
        "ScrollFactor",
        "naturalScrollChanged",
        "scrollFactorChanged",
        "devicesSysNames",
        // Plasma 6.7 DevicesModel enumerates via ListPointers, not the
        // devicesSysNames property: without it the KCM shows zero rows.
        "ListPointers",
        "ExportScriptableContents",
    ] {
        assert!(
            ANLAND_INPUT_SOURCE.contains(required) || ANLAND_INPUT_HEADER_SOURCE.contains(required),
            "Anland touchpad is missing {required}"
        );
    }
    assert!(ANLAND_INPUT_SOURCE.contains("return true;"));
    assert!(!ANLAND_INPUT_SOURCE.contains("anland virtual input"));
    // Exactly one layer owns each transformation: factor+inversion live in
    // pointerAxisFinger; wheel/continuous and axis-stop stay raw.
    let finger = ANLAND_INPUT_SOURCE
        .find("pointerAxisFinger")
        .expect("finger scroll handler must exist");
    assert!(ANLAND_INPUT_SOURCE[finger..].contains("m_scrollFactor"));
    assert!(ANLAND_INPUT_SOURCE[finger..].contains("m_naturalScroll"));
}

const QPA_WINDOW_SOURCE: &str =
    include_str!("../patches/kwin/anland-6.7.4/src/plugins/qpa/window.cpp");
const QPA_BACKINGSTORE_SOURCE: &str =
    include_str!("../patches/kwin/anland-6.7.4/src/plugins/qpa/backingstore.cpp");
#[test]
fn anland_qpa_never_uses_invalid_buffers() {
    // GL QPA surfaces need a dma-buf allocator; Anland has no DRM device, so
    // the swapchain request must fail closed (EGLPlatformContext handles a
    // null swapchain). Handing it SHM would crash later inside
    // importDmaBufAsTexture(*buffer->dmabufAttributes()).
    assert!(QPA_WINDOW_SOURCE.contains("No DRM render device for GL surface"));
    assert!(!QPA_WINDOW_SOURCE.contains("using SHM backing store for GL surface"));
    // The null drmDevice() case returns before any allocator dereference;
    // the remaining drmDevice()->allocator() use is only reachable with a
    // valid device (guarded by the early return above it).
    let guard = QPA_WINDOW_SOURCE
        .find("!Compositor::self()->backend()->drmDevice()")
        .expect("null-DRM guard must exist");
    let guarded_return = QPA_WINDOW_SOURCE[guard..]
        .find("return nullptr")
        .expect("null-DRM path must fail closed");
    let deref = QPA_WINDOW_SOURCE
        .find("drmDevice()->allocator()")
        .expect("valid-device allocator path must remain");
    assert!(guarded_return < deref);
    // Raster backing stores stay fully supported via SHM.
    assert!(QPA_BACKINGSTORE_SOURCE.contains("DRM_FORMAT_ARGB8888"));
    // All three beginPaint() failure modes (no platform window,
    // swapchain/acquire failure, mapping failure) set the fallback flag, and
    // flush() drops the frame instead of dereferencing/presenting null.
    assert!(
        QPA_BACKINGSTORE_SOURCE
            .matches("m_usingFallback = true")
            .count()
            >= 3
    );
    assert!(QPA_BACKINGSTORE_SOURCE.contains("m_usingFallback || !m_buffer"));
    // A failed acquire keeps the last good buffer instead of going blank.
    assert!(QPA_BACKINGSTORE_SOURCE.contains("m_buffer = oldBuffer"));
}

const DEBUG_VEIL_KOTLIN_SOURCE: &str =
    include_str!("../src/android/kotlin/app/polarbear/ComposeOverlay.kt");
const DEBUG_VEIL_ACTIVITY_SOURCE: &str =
    include_str!("../src/android/kotlin/app/polarbear/PortalActivity.kt");

#[test]
fn debug_veil_hook_is_debug_only_and_ready_gated() {
    // Automated UI tests must dismiss the READY veil through a Debug-only
    // hook that mirrors the human reveal transition; Stable/release must be
    // unable to trigger it and it must refuse before desktop readiness.
    for required in [
        "ACTION_DEBUG_DISMISS_VEIL",
        "debugDismissVeilForAutomation",
        "isDebuggable",
        "FLAG_DEBUGGABLE",
        "desktopReadyState",
        "acknowledgeRevealCommitted",
    ] {
        assert!(
            DEBUG_VEIL_KOTLIN_SOURCE.contains(required),
            "debug veil hook is missing {required}"
        );
    }
    assert!(DEBUG_VEIL_ACTIVITY_SOURCE.contains("ACTION_DEBUG_DISMISS_VEIL"));
    assert!(DEBUG_VEIL_ACTIVITY_SOURCE.contains("FLAG_DEBUGGABLE"));
    // The hook refuses without readiness or an attached veil (fail closed).
    assert!(DEBUG_VEIL_KOTLIN_SOURCE.contains("debug veil dismiss refused"));
}

#[test]
fn fresh_renderer_selection_is_initialized_before_mesa_and_handoff() {
    let run = include_str!("../src/android/app/run.rs");

    // The policy is shared with the production provisioning writer, so the
    // renderer flag gets the same atomic replacement and parent durability as
    // the runtime markers. Only uninitialised/image-only state selects
    // Anland; historical renderer values remain parseable but do not select a
    // retired graphics backend.
    assert!(RENDERER_POLICY_SOURCE.contains("provisioning::write_atomic"));
    assert!(RENDERER_POLICY_SOURCE.contains("RuntimeClassification::LegacyPortal"));
    assert!(RENDERER_POLICY_SOURCE.contains("RendererSelection::Anland"));
    assert!(RENDERER_POLICY_SOURCE.contains("never select the"));

    // Setup establishes the mode after the image stage and before Mesa reads
    // it. Finalisation revalidates the same durable choice before writing the
    // completion marker, so a fresh install cannot finish while its Anland
    // selection is only an in-memory default.
    let renderer_stage = ANDROID_SETUP_SOURCE
        .find("(\"renderer-mode\", Box::new(setup_renderer_mode))")
        .expect("renderer mode setup stage must exist");
    let mesa_stage = ANDROID_SETUP_SOURCE
        .find("(\"mesa-kgsl-layer\", Box::new(setup_mesa_layer))")
        .expect("Mesa setup stage must exist");
    assert!(renderer_stage < mesa_stage);
    assert!(ANDROID_SETUP_SOURCE.contains("ensure_renderer_mode()"));
    assert!(ANDROID_SETUP_SOURCE.contains("RendererKind::Anland"));
    let finalise = ANDROID_SETUP_SOURCE
        .split("fn finalise_installation")
        .nth(1)
        .expect("installation finalisation must exist");
    assert!(finalise.contains("ensure_renderer_mode()"));
    assert!(finalise.contains("mark_installation_complete"));
    assert!(
        finalise.find("ensure_renderer_mode()").unwrap()
            < finalise.find("mark_installation_complete").unwrap()
    );

    // The committed first-install path validates the renderer state before
    // constructing Wayland, and the normal resume path consumes the same
    // persisted selection for Anland vs QPainter.
    assert!(ANDROID_SETUP_SOURCE.contains("build_committed_wayland_backend"));
    assert!(run.contains("is_anland_requested()"));
    assert!(ANLAND_ENV_SOURCE.contains("resolve_renderer_mode"));
    assert!(!ANLAND_ENV_SOURCE.contains("unwrap_or(RendererKind::Smithay)"));
}

#[test]
fn explicit_anland_repair_is_native_and_does_not_reprovision_debian() {
    let repair = ANDROID_SETUP_SOURCE
        .split("fn run_anland_repair_inner")
        .nth(1)
        .and_then(|source| source.split("fn run_anland_repair(").next())
        .expect("targeted Anland repair worker must exist");
    assert!(ANDROID_SETUP_SOURCE.contains("pub fn repair_enable_anland()"));
    assert!(ANDROID_SETUP_SOURCE.contains("pub fn take_anland_repair_result()"));
    assert!(ANDROID_SETUP_SOURCE.contains("force_anland_renderer()"));
    assert!(repair.contains("crate::android::proot::launch::stop()"));
    assert!(repair.contains("RuntimeClassification"));
    assert!(repair.contains("sync_anland_required_session_files"));
    assert!(repair.contains("validate_anland_repair_state"));
    assert!(repair.contains("mesa_layer::is_provisioned"));
    assert!(repair.contains("mesa_layer::provision_with_progress"));
    assert!(!repair.contains("RuntimeArtifact::provision"));
    assert!(!repair.contains("extract_inner"));
    // A legacy marker may be upgraded after the graphics transaction succeeds;
    // the repair path never marks a modern install incomplete or invalidates
    // its existing completion marker.
    assert!(repair.contains(
        "initial_classification == crate::core::provisioning::RuntimeClassification::LegacyPortal"
    ));
    assert!(repair.contains("artifact.mark_installation_complete(root)"));
}

#[test]
fn anland_repair_only_refreshes_portal_owned_session_assets() {
    let helper = ANDROID_SETUP_SOURCE
        .split("fn sync_anland_required_session_files")
        .nth(1)
        .and_then(|source| source.split("fn setup_firefox_config").next())
        .expect("narrow Anland session sync helper must exist");
    for required in [
        "sync_guest_session_directories",
        "sync_firefox_config",
        "sync_portal_runtime_assets",
        "sync_crash_handler",
        "validate_required_session_files",
    ] {
        assert!(
            helper.contains(required),
            "repair helper missing {required}"
        );
    }
    for forbidden in [
        "sync_debian_package_management",
        "sync_base_files_defaults",
        "migrate_konsole_profile",
        "upsert_kv_file",
        "panel-launchers-v2",
        "xresources",
    ] {
        assert!(
            !helper.contains(forbidden),
            "repair helper must not replay user setup: {forbidden}"
        );
    }
    // The one-shot token prevents launch() from immediately replaying the
    // broad normal sync. The next launch still takes the normal branch.
    let launch = include_str!("../src/android/proot/launch.rs");
    assert!(launch.contains("take_prepared_anland_launch()"));
    assert!(launch.contains("try_sync_session_runtime_files"));
}

#[test]
fn anland_repair_revalidates_mesa_kwin_firefox_and_session_contract() {
    assert!(ANDROID_SETUP_SOURCE.contains("validate_launch_contract()"));
    assert!(ANDROID_SETUP_SOURCE.contains("KWIN_ANLAND_LIBRARY"));
    assert!(!ANDROID_SETUP_SOURCE.contains("DRMSHIM_BINARY"));
    assert!(ANDROID_SETUP_SOURCE.contains("sync_kwin_anland_overlay_for_repair"));
    assert!(ANDROID_SETUP_SOURCE.contains("bytes == KWIN_ANLAND_LIBRARY"));
    assert!(ANDROID_SETUP_SOURCE.contains("validate_firefox_anland_config"));
    assert!(ANDROID_SETUP_SOURCE.contains("Firefox Portal GPU preference is missing"));
    assert!(!ANDROID_SETUP_SOURCE
        .contains("Firefox config still contains a Portal-specific XWayland/GPU override"));
    for required in [
        "MOZ_ENABLE_WAYLAND",
        "MOZ_USE_XINPUT2",
        "GTK_IM_MODULE",
        "MESA_LOADER_DRIVER_OVERRIDE",
        "GALLIUM_DRIVER",
        "FD_FORCE_KGSL",
        "FD_KGSL_ENABLE_DMABUF",
        "TURNIP_KMD",
        "validate_launch_contract",
    ] {
        assert!(ANLAND_ENV_SOURCE.contains(required));
    }
    // KWin wrapper keeps its local HW copy (plus KWin-only ANLAND/XWAYLAND
    // force flags); clients get the minimal KGSL set guest-wide via
    // guest_mesa_env. EGL_PLATFORM must stay unset everywhere.
    for required in [
        "MESA_LOADER_DRIVER_OVERRIDE",
        "GALLIUM_DRIVER",
        "FD_FORCE_KGSL",
        "FD_KGSL_ENABLE_DMABUF",
        "TURNIP_KMD",
        "XWAYLAND_FORCE_KGSL_SURFACELESS",
    ] {
        assert!(KWIN_WRAPPER_SOURCE.contains(required));
    }
    for required in [
        "usr/local/lib/portal-anland/kwin_wayland",
        "usr/local/lib/portal-anland/libkwin.so.6.7.4",
        "localdesktop-crash-handler.so",
        "portal-ibus-engine",
        "portal-ibus-lazy",
    ] {
        assert!(ANDROID_SETUP_SOURCE.contains(required));
    }
}

#[test]
fn anland_repair_handoff_restarts_the_session_and_routes_failures_to_runtime_recovery() {
    assert!(COMPOSE_OVERLAY_RUST_SOURCE.contains("nativeRepairEnableAnland"));
    assert!(COMPOSE_OVERLAY_KOTLIN_SOURCE.contains("nativeRepairEnableAnland"));
    assert!(ANDROID_SETUP_RUN_SOURCE.contains("handle_anland_repair_result"));
    assert!(ANDROID_SETUP_RUN_SOURCE.contains("take_anland_repair_result"));
    assert!(ANDROID_SETUP_RUN_SOURCE.contains("build_committed_wayland_backend"));
    assert!(ANDROID_SETUP_RUN_SOURCE.contains("!resume_wayland"));
    assert!(ANDROID_SETUP_RUN_SOURCE.contains("cancel_prepared_anland_launch"));
    assert!(ANDROID_SETUP_RUN_SOURCE.contains("mark_anland_repair_handoff_active"));
    assert!(ANDROID_SETUP_RUN_SOURCE.contains("anland_repair_handoff_active"));
    assert!(ANDROID_SETUP_RUN_SOURCE.contains("enter_committed_install_runtime_error"));
    // The worker result is consumed by the event loop, not by a setup/UI
    // coroutine, and the repair worker never publishes installation Complete.
    let repair = ANDROID_SETUP_SOURCE
        .split("fn publish_repair_success")
        .nth(1)
        .and_then(|source| source.split("fn run_anland_repair_inner").next())
        .expect("repair completion publisher must exist");
    assert!(repair.contains("ProvisioningPhase::Finalising"));
    assert!(!repair.contains("ProvisioningSnapshot::complete"));
}

#[test]
fn anland_repair_recreation_attaches_before_completed_setup_replay() {
    assert!(ANDROID_SETUP_SOURCE.contains("fn attach_to_anland_repair"));
    assert!(ANDROID_SETUP_SOURCE.contains("attach_to_anland_repair(&registration, &progress)"));
    let setup = ANDROID_SETUP_SOURCE
        .split("pub fn setup_with_completion")
        .nth(1)
        .and_then(|source| source.split("pub fn setup(").next())
        .expect("setup_with_completion must exist");
    let attach = setup
        .find("attach_to_anland_repair(&registration, &progress)")
        .expect("recreated setup must attach to an active repair");
    let completed_runtime = setup
        .find("if artifact.is_bootable(root) || artifact.is_legacy_complete(root)")
        .expect("completed-runtime setup branch must exist");
    assert!(
        attach < completed_runtime,
        "repair attachment must precede the normal completed-runtime setup replay"
    );
    assert!(ANDROID_SETUP_SOURCE.contains("active_registration.rebind_from(registration)"));
}

#[test]
fn anland_suspend_retires_only_the_surface_and_preserves_the_guest_session() {
    let suspended = ANDROID_SETUP_RUN_SOURCE
        .split("fn suspended")
        .nth(1)
        .and_then(|source| source.split("fn about_to_wait").next())
        .expect("Android suspended handler must exist");
    assert!(suspended.contains("backend.suspend_input_and_presentation()"));
    assert!(suspended.contains("session.suspend_surface()"));
    assert!(!suspended.contains("backend.anland.take()"));
    assert!(suspended.contains("pipewire_standalone_aaudio::shutdown()"));

    let suspend = ANLAND_CONSUMER_SOURCE
        .split("pub fn suspend_surface")
        .nth(1)
        .and_then(|source| source.split("/// Forward one fixed-size input").next())
        .expect("surface suspend operation must exist");
    for required in [
        "window_live.store(false",
        "release_surface_inputs",
        "join_surface_thread(handle, \"render\")",
        "pump.stop()",
        "teardown_generation",
        "join_surface_thread(handle, \"event\")",
        "release_window",
        "window_holder.take()",
        "surface_epoch.store(0",
        "broker=preserved guest=preserved",
    ] {
        assert!(
            suspend.contains(required),
            "surface suspend missing {required}"
        );
    }

    let stop = ANLAND_CONSUMER_SOURCE
        .split("pub fn stop(mut self)")
        .nth(1)
        .and_then(|source| source.split("/// Surface-bound threads").next())
        .expect("permanent Anland stop operation must exist");
    assert!(stop.contains("self.suspend_surface()"));
    assert!(stop.contains("inner.broker_stop.store(true"));
    assert!(stop.contains("join_surface_thread(h, \"broker\")"));

    // The broker/listener is session-scoped. Suspend withdraws a deposit but
    // does not stop the listener; permanent stop is the only broker teardown.
    assert!(ANLAND_BROKER_SOURCE.contains("pub fn serve"));
    assert!(ANLAND_BROKER_SOURCE.contains("pub fn withdraw"));
    assert!(ANLAND_BROKER_SOURCE.contains("pub fn deposit"));
}

#[test]
fn anland_resume_rebinds_a_fresh_surface_without_relaunching_plasma() {
    let resume = ANDROID_SETUP_RUN_SOURCE
        .split("fn resume_anland")
        .nth(1)
        .and_then(|source| source.split("/// Forward raw window input").next())
        .expect("Anland resume operation must exist");
    for required in [
        "let existing_session = backend.anland.is_some()",
        "surface_healthy",
        "session.resume_surface(raw, window, &config)",
        "surface_convergence.begin_epoch",
        "if !is_running()",
        "guest Plasma preserved; launch() skipped on surface resume",
        "launch();",
    ] {
        assert!(resume.contains(required), "resume path missing {required}");
    }
    let preserved = resume
        .split("// A normal Android resume must not call launch()")
        .nth(1)
        .and_then(|source| source.split("} else {").next())
        .expect("existing-session resume branch must exist");
    assert!(!preserved.contains("launch();"));
    assert!(resume.contains("AnlandSession::start"));

    let session_resume = ANLAND_CONSUMER_SOURCE
        .split("pub fn resume_surface")
        .nth(1)
        .and_then(|source| source.split("fn release_surface_inputs").next())
        .expect("surface resume operation must exist");
    for required in [
        "configure_window",
        "ensure_broker_thread",
        "mint_surface_epoch",
        "surface_epoch.store(epoch",
        "VsyncPump::start",
        "collect_buffers",
        "deposit_generation",
        "start_surface_threads",
        "guest session preserved",
    ] {
        assert!(
            session_resume.contains(required),
            "surface resume missing {required}"
        );
    }
    assert!(ANLAND_CONSUMER_SOURCE.contains("thread.is_finished()"));
    assert!(ANLAND_CONSUMER_SOURCE.contains("pub fn surface_healthy"));
    assert!(ANLAND_CONSUMER_SOURCE.contains("broker listener restarted before surface resume"));
}

#[test]
fn anland_generation_and_readiness_are_scoped_to_the_current_surface() {
    let suspend = ANLAND_CONSUMER_SOURCE
        .split("pub fn suspend_surface")
        .nth(1)
        .and_then(|source| source.split("/// Forward one fixed-size input").next())
        .expect("surface suspend operation must exist");
    let resume = ANLAND_CONSUMER_SOURCE
        .split("pub fn resume_surface")
        .nth(1)
        .and_then(|source| source.split("fn release_surface_inputs").next())
        .expect("surface resume operation must exist");
    for source in [suspend, resume] {
        assert!(source.contains("ready_surface_gen"));
        assert!(source.contains("last_pointer"));
        assert!(source.contains("finger_axes"));
    }
    assert!(ANLAND_CONSUMER_SOURCE.contains("generation_completion_allowed"));
    assert!(ANLAND_CONSUMER_SOURCE.contains("stale-generation cancel-back"));
    assert!(ANLAND_CONSUMER_SOURCE.contains("notify_desktop_ready_cached"));
    assert!(ANLAND_EVENT_HANDLER_SOURCE.contains("if !session.surface_active()"));
    assert!(ANLAND_EVENT_HANDLER_SOURCE.contains("confirm_converged(gen)"));
    // New lifecycle epochs are explicitly rebased; delayed observations from
    // the old winit/native window cannot complete the new epoch.
    assert!(ANDROID_SETUP_RUN_SOURCE.contains("surface-attach epoch="));
    assert!(ANDROID_SETUP_RUN_SOURCE.contains("begin_epoch("));
}

#[test]
fn anland_surface_resume_failures_use_committed_runtime_recovery() {
    let resume = ANDROID_SETUP_RUN_SOURCE
        .split("fn resume_anland")
        .nth(1)
        .and_then(|source| source.split("/// Forward raw window input").next())
        .expect("Anland resume operation must exist");
    assert!(resume.contains("anland.surface resume failed (guest session preserved)"));
    assert!(resume.contains("return false"));
    // The surrounding lifecycle handler classifies an Anland failure as a
    // runtime failure after installation, never as setup-incomplete state.
    let resumed = ANDROID_SETUP_RUN_SOURCE
        .split("fn resumed(&mut self")
        .nth(1)
        .and_then(|source| source.split("fn user_event").next())
        .expect("resumed handler must exist");
    assert!(resumed.contains("let anland_resume = matches!"));
    assert!(resumed.contains("enter_committed_install_runtime_error"));
    assert!(resumed.contains("Anland could not reattach"));
    assert!(ANDROID_SETUP_RUN_SOURCE
        .contains("Portal is installed, but Wayland lost its renderer. Tap Retry Plasma."));
    // The recovery helper stops/reaps the session and swaps to the existing
    // runtime-error page; it contains no provisioning-marker mutation.
    let recovery = ANDROID_SETUP_RUN_SOURCE
        .split("fn enter_committed_install_runtime_error")
        .nth(1)
        .and_then(|source| source.split("fn enter_runtime_error_with_mode").next())
        .expect("committed runtime recovery helper must exist");
    assert!(recovery.contains("enter_runtime_error_with_mode(reason.into(), true)"));
}

#[test]
fn anland_lifecycle_is_the_authoritative_graphics_path() {
    assert!(ANDROID_SETUP_RUN_SOURCE.contains("fn resume_anland"));
    assert!(ANDROID_SETUP_RUN_SOURCE.contains("anland.surface resume failed"));
    assert!(ANDROID_SETUP_RUN_SOURCE.contains("backend.anland.as_mut()"));
}

#[test]
fn normal_renderer_policy_always_selects_anland() {
    assert!(RENDERER_POLICY_SOURCE.contains("missing_mode_selection"));
    assert!(RENDERER_POLICY_SOURCE.contains("RuntimeClassification::BootablePortal"));
    assert!(RENDERER_POLICY_SOURCE.contains("Existing historical values resolve to Anland"));
    assert!(RENDERER_POLICY_SOURCE.contains("pub fn set_renderer_mode"));
    assert!(ANLAND_ENV_SOURCE.contains("pub fn force_anland_renderer"));
    let active = ANLAND_ENV_SOURCE
        .split("pub fn active_renderer")
        .nth(1)
        .and_then(|source| source.split("pub fn ensure_renderer_mode").next())
        .expect("active renderer policy must exist");
    assert!(!active.contains("force_anland_renderer"));
}

#[test]
fn return_to_plasma_anland_affordance_is_inline_and_native_state_driven() {
    for required in [
        "Accelerated graphics available →",
        "Enabling accelerated graphics…",
        "Preparing Anland and GPU acceleration",
        "ReturnAnlandAffordance",
        "ComposeOverlay.anlandRepairState()",
        "ComposeOverlay.repairEnableAnland()",
        "AnimatedVisibility(",
        "AnimatedContent(",
        "animateContentSize",
        "SizeTransform",
        "updateTransition",
    ] {
        assert!(
            PORTAL_RETURN_SCREEN_SOURCE.contains(required),
            "Return-to-Plasma affordance is missing {required}"
        );
    }
    // The user-facing Return screen must not expose implementation details
    // from the graphics transaction.
    for forbidden in ["Mesa", "KGSL", "drmshim", "diagnostic"] {
        assert!(
            !PORTAL_RETURN_SCREEN_SOURCE.contains(forbidden),
            "Return screen leaked implementation detail {forbidden}"
        );
    }
    assert!(COMPOSE_OVERLAY_KOTLIN_SOURCE.contains("updateAnlandRepairState"));
    assert!(COMPOSE_OVERLAY_RUST_SOURCE.contains("publish_current_anland_repair_state"));
    assert!(ANDROID_SETUP_SOURCE.contains("AnlandRepairAvailability::Unavailable"));
    assert!(ANDROID_SETUP_SOURCE.contains("is_trusted_recovery()"));
}

#[test]
fn return_repair_morph_gates_only_the_shared_reveal_and_reuses_runtime_recovery() {
    assert!(PORTAL_LAUNCH_TRANSITION_SOURCE
        .contains("eligible = resolved && setupReady && desktopReady && !returnRepairBlocked"));
    assert!(PORTAL_LAUNCH_TRANSITION_SOURCE.contains("onRepairBlockedChanged"));
    assert!(PORTAL_RETURN_SCREEN_SOURCE.contains("requestAnlandRepairRecovery()"));
    assert!(COMPOSE_OVERLAY_KOTLIN_SOURCE.contains("nativeRequestAnlandRepairRecovery"));
    assert!(COMPOSE_OVERLAY_RUST_SOURCE.contains("take_anland_repair_recovery_request"));
    assert!(ANDROID_SETUP_RUN_SOURCE.contains("handle_anland_repair_recovery_request"));
    assert!(ANDROID_SETUP_RUN_SOURCE
        .contains("Portal is installed, but Anland graphics repair failed. Tap Retry Plasma."));
    // There is one shared PortalRevealVeil: repair does not create a second
    // dismissal/reveal animation or bypass the real desktop-ready latch.
    assert_eq!(
        PORTAL_LAUNCH_TRANSITION_SOURCE
            .matches("PortalRevealVeil(")
            .count(),
        1
    );
    assert!(PORTAL_LAUNCH_TRANSITION_SOURCE.contains("desktopReady"));
}

#[test]
fn return_repair_progress_is_real_and_never_claims_completion() {
    assert!(COMPOSE_OVERLAY_KOTLIN_SOURCE.contains("progress.coerceIn(0, 100)"));
    assert!(PORTAL_RETURN_SCREEN_SOURCE.contains("progress.coerceIn(0, 99)"));
    assert!(ANDROID_SETUP_SOURCE.contains("ProvisioningPhase::Finalising"));
    assert!(ANDROID_SETUP_SOURCE.contains("99,"));
    assert!(ANDROID_SETUP_SOURCE.contains("publish_anland_repair_state"));
    // Graphics repair is a separate stream and never publishes the Debian
    // installation Complete snapshot.
    let repair_success = ANDROID_SETUP_SOURCE
        .split("fn publish_repair_success")
        .nth(1)
        .and_then(|source| source.split("fn run_anland_repair_inner").next())
        .expect("repair success publisher must exist");
    assert!(!repair_success.contains("ProvisioningSnapshot::complete"));
    assert!(repair_success.contains("Anland graphics ready. Restarting Plasma…"));
}

#[test]
fn return_repair_drops_the_old_compositor_before_rebinding_wayland() {
    let handoff = ANDROID_SETUP_RUN_SOURCE
        .split("fn handle_anland_repair_result")
        .nth(1)
        .and_then(|source| source.split("fn handle_setup_complete").next())
        .expect("repair handoff must exist");
    let readiness_clear = handoff
        .find("notify_desktop_suspended")
        .expect("repair handoff must clear old readiness");
    let backend_replace = handoff
        .find("std::mem::replace")
        .expect("repair handoff must drop the old backend before rebinding");
    let backend_build = handoff
        .find("build_committed_wayland_backend")
        .expect("repair handoff must build the committed backend");
    assert!(readiness_clear < backend_replace);
    assert!(backend_replace < backend_build);
    assert!(handoff.contains("drop(old_backend)"));
}
