//! Focused regression coverage for the external Android clipboard paste path.
//!
//! Bug: Android clipboard polling queued `ClipboardEvent::AndroidChanged`,
//! but `process_android_clipboard()` ran only from `redraw()`. With
//! event-driven rendering, external changes never woke the event loop, so
//! Ctrl+V could reach KWin before the new selection was imported.
//!
//! These host tests drive the platform-independent policy in
//! `src/core/clipboard_sync.rs` (the same state machine the Android worker
//! and compositor use) plus source wiring guards so the fix cannot regress
//! to a timing hack.

use localdesktop::core::clipboard_sync::ExternalClipboardSync;

const ACCESSIBILITY: &str = include_str!("../src/android/accessibility.rs");
const CLIPBOARD: &str = include_str!("../src/android/clipboard.rs");
const COMPOSITOR: &str = include_str!("../src/android/backend/wayland/compositor.rs");
const EVENT_HANDLER: &str = include_str!("../src/android/backend/wayland/event_handler.rs");
const RUN: &str = include_str!("../src/android/app/run.rs");
const CORE_SYNC: &str = include_str!("../src/core/clipboard_sync.rs");
const SMITHAY_KEYBOARD: &str = include_str!("../patches/smithay/src/input/keyboard/mod.rs");
const ANDROID_BROKER: &str = include_str!("../src/android/clipboard_broker.rs");
const SETUP: &str = include_str!("../src/android/proot/setup.rs");
const START_PLASMA: &str = include_str!("../assets/localdesktop-startplasma.sh");
const CLIPBOARD_SYNC: &str = include_str!("../assets/localdesktop-clipboard-sync.sh");
const CLIPBOARD_PUSH: &str = include_str!("../assets/localdesktop-clipboard-push.sh");
const PROCESS: &str = include_str!("../src/android/proot/process.rs");
const LAUNCH: &str = include_str!("../src/android/proot/launch.rs");

/// Copy in another app → return to Portal → first Ctrl+V sees the new text.
///
/// Models Android denying reads while backgrounded (`Err`), an explicit
/// resync on resume, and the mandatory drain-plus-flush before input.
#[test]
fn external_copy_then_resume_delivers_before_first_paste() {
    let mut sync = ExternalClipboardSync::new();
    // Portal starts with the old selection already imported.
    assert!(sync.observe_poll(Ok(Some("old text".to_owned()))));
    let mut wayland_selection: Option<String> = None;
    for pending in sync.drain_pending() {
        wayland_selection = pending;
    }
    assert_eq!(wayland_selection.as_deref(), Some("old text"));

    // Backgrounded: Binder reads are denied, nothing is queued.
    assert!(!sync.observe_poll(Err(())));
    assert!(!sync.has_pending());

    // User copies "new text" in another Android app while Portal is away.
    // Portal has not observed it yet; the Wayland selection is still stale.
    assert_eq!(wayland_selection.as_deref(), Some("old text"));

    // Activity resume / focus regain explicitly resyncs instead of waiting
    // for the next poll interval.
    sync.request_resync();
    assert!(sync.needs_resync());

    // The resync read observes the new value and queues it with a wake.
    assert!(sync.observe_poll(Ok(Some("new text".to_owned()))));
    assert!(!sync.needs_resync());
    assert!(sync.has_pending());

    // The event loop must apply + flush the queued update BEFORE forwarding
    // the first Ctrl+V. Simulate that ordering here.
    let queued = sync.drain_pending();
    assert_eq!(queued, vec![Some("new text".to_owned())]);
    for pending in queued {
        wayland_selection = pending;
    }
    assert!(!sync.has_pending());

    // First paste now observes the fresh selection.
    let pasted = wayland_selection.clone();
    assert_eq!(pasted.as_deref(), Some("new text"));
}

/// Without the drain-before-input ordering the first paste would be stale.
///
/// This documents the bug being fixed: if input were forwarded before the
/// queued update is applied, KWin would still see the old selection.
#[test]
fn forwarding_input_before_draining_would_paste_stale_text() {
    let mut sync = ExternalClipboardSync::new();
    assert!(sync.observe_poll(Ok(Some("old text".to_owned()))));
    let mut wayland_selection: Option<String> = None;
    for pending in sync.drain_pending() {
        wayland_selection = pending;
    }
    sync.request_resync();
    assert!(sync.observe_poll(Ok(Some("new text".to_owned()))));

    // Buggy order: read the selection for Ctrl+V before draining.
    let stale_paste = wayland_selection.clone();
    assert_eq!(stale_paste.as_deref(), Some("old text"));

    // Fixed order recovers.
    for pending in sync.drain_pending() {
        wayland_selection = pending;
    }
    assert_eq!(wayland_selection.as_deref(), Some("new text"));
}

#[test]
fn resync_without_change_queues_nothing() {
    let mut sync = ExternalClipboardSync::new();
    assert!(sync.observe_poll(Ok(Some("same".to_owned()))));
    sync.drain_pending();
    sync.request_resync();
    assert!(!sync.observe_poll(Ok(Some("same".to_owned()))));
    assert!(!sync.has_pending());
}

#[test]
fn guest_write_echo_does_not_reenter_wayland() {
    let mut sync = ExternalClipboardSync::new();
    assert!(sync.observe_poll(Ok(Some("old".to_owned()))));
    sync.drain_pending();

    sync.mark_guest_write(Some("from-guest".to_owned()));
    assert!(!sync.observe_poll(Ok(Some("from-guest".to_owned()))));
    assert!(!sync.has_pending());

    // A later external copy of the same text is still a real change.
    assert!(sync.observe_poll(Ok(Some("other".to_owned()))));
    sync.drain_pending();
    assert!(sync.observe_poll(Ok(Some("from-guest".to_owned()))));
    assert_eq!(sync.drain_pending(), vec![Some("from-guest".to_owned())]);
}

#[test]
fn clipboard_wake_and_ordering_are_wired_without_timing_hacks() {
    // Dedicated wake event exists and is distinct from the other user events.
    assert!(ACCESSIBILITY.contains("AndroidClipboardChanged"));
    assert!(ACCESSIBILITY.contains("AccessibilityInputReady"));
    assert!(ACCESSIBILITY.contains("WaylandTraffic"));

    // The worker wakes the loop immediately with the dedicated event; the
    // shared tracker owns change detection so host tests cover device logic.
    assert!(CLIPBOARD.contains("AndroidClipboardChanged"));
    assert!(CLIPBOARD.contains("ExternalClipboardSync"));
    assert!(CLIPBOARD.contains("request_resync"));
    assert!(CLIPBOARD.contains("observe_poll"));
    assert!(CLIPBOARD.contains("drain_pending"));
    assert!(CLIPBOARD.contains("send_event"));
    // Signal-driven wait: clipboard reads happen at startup/resume/focus only.
    assert!(CLIPBOARD.contains("cvar.wait(requested)"));
    assert!(!CLIPBOARD.contains("wait_timeout"));
    assert!(!CLIPBOARD.contains("thread::sleep"));

    // The compositor drains without Binder work and publishes to the inner
    // KWin broker. Reads stay on the worker.
    assert!(COMPOSITOR.contains("process_android_clipboard"));
    assert!(COMPOSITOR.contains("request_android_clipboard_resync"));
    assert!(COMPOSITOR.contains("publish_android_clipboard"));
    assert!(!COMPOSITOR.contains("read_text"));

    // Keyboard/paste input drains first; focus regain resyncs.
    assert!(EVENT_HANDLER.contains("process_android_clipboard"));
    assert!(EVENT_HANDLER.contains("request_android_clipboard_resync"));
    assert!(!EVENT_HANDLER.contains("read_text"));

    // Resume and the dedicated wake both synchronize before input.
    assert!(RUN.contains("AndroidClipboardChanged"));
    assert!(RUN.contains("process_android_clipboard"));
    assert!(!RUN.contains("request_android_clipboard_resync"));

    // No timing hacks, synthetic paste, or vendor workarounds. The one
    // background listener below is the authenticated Android <-> inner-KWin
    // transport required by the nested compositor topology.
    // ("No debounce sleeps" comments document the absence of such hacks and
    // are allowed; a real hack would appear as an identifier like
    // `debounce_ms`, `Debouncer`, or a `debounce(` call.)
    for src in [CLIPBOARD, COMPOSITOR, EVENT_HANDLER, RUN] {
        let lower = src.to_lowercase();
        assert!(
            !lower.contains("debouncer")
                && !lower.contains("debounce_ms")
                && !lower.contains("debounce("),
            "debounce hack in clipboard path"
        );
        assert!(
            !src.contains("Thread::sleep") && !src.contains("thread::sleep"),
            "sleep in clipboard/input path"
        );
    }
    assert!(!CLIPBOARD.to_lowercase().contains("synthetic"));
    assert!(!RUN.to_lowercase().contains("synthetic"));
    assert!(!CLIPBOARD.contains("KDE"));
    assert!(!RUN.contains("daemon"));

    // The pure policy itself never touches threads, time, JNI, or FDs.
    assert!(!CORE_SYNC.contains("thread::sleep"));
    assert!(!CORE_SYNC.contains("wait_timeout"));
    assert!(!CORE_SYNC.contains("JNI"));
    assert!(!CORE_SYNC.contains("OwnedFd"));
}

#[test]
fn inner_kwin_clipboard_uses_the_existing_authenticated_broker_path() {
    assert!(ANDROID_BROKER.contains("TcpListener::bind(\"127.0.0.1:0\")"));
    assert!(ANDROID_BROKER.contains("constant_time_eq_token"));
    assert!(ANDROID_BROKER.contains("BrokerState"));
    assert!(ANDROID_BROKER.contains("RequestKind::Subscribe"));
    assert!(ANDROID_BROKER.contains("BrokerEvent::Value"));

    assert!(SETUP.contains("CLIPBOARD_SYNC"));
    assert!(SETUP.contains("WL_COPY_BINARY"));
    assert!(SETUP.contains("localdesktop-clipboard-sync"));
    assert!(SETUP.contains("usr/local/bin/wl-copy"));
    assert!(SETUP.contains("usr/local/bin/wl-paste"));
    assert!(START_PLASMA.contains("WAYLAND_DISPLAY=wayland-1"));
    assert!(START_PLASMA.contains("localdesktop-clipboard-sync"));
    assert!(CLIPBOARD_PUSH.contains("LDCL/1 CLEAR"));
    assert!(CLIPBOARD_PUSH.contains("LOCALDESKTOP_CLIPBOARD_INITIAL_GATE"));
    assert!(CLIPBOARD_SYNC.contains("ignore-initial"));
    assert!(PROCESS.contains("run_with_cancel_and_env"));
    assert!(LAUNCH.contains("ClipboardBridge::broker_environment"));
    assert!(COMPOSITOR.contains("publish_android_clipboard"));
}

#[test]
fn clipboard_and_keyboard_focus_target_the_identified_kwin_surface() {
    // A recovery or stale xdg-toplevel must not receive KWin's clipboard offer
    // or replace the keyboard target merely because it appears first.
    let surface_helper_start = EVENT_HANDLER
        .find("fn get_surface")
        .expect("input surface helper is present");
    let surface_helper_end = EVENT_HANDLER[surface_helper_start..]
        .find("fn pointer_focus")
        .map(|offset| surface_helper_start + offset)
        .expect("input surface helper has a bounded body");
    let surface_helper = &EVENT_HANDLER[surface_helper_start..surface_helper_end];
    assert!(surface_helper.contains("is_known_kwin_surface"));
    assert!(!surface_helper.contains(".iter().next()"));

    assert!(COMPOSITOR.contains("pub fn sync_kwin_seat_focus"));
    let data_focus_start = COMPOSITOR
        .find("pub fn sync_data_device_focus")
        .expect("data-device focus helper is present");
    let data_focus_end = COMPOSITOR[data_focus_start..]
        .find("pub fn sync_kwin_seat_focus")
        .map(|offset| data_focus_start + offset)
        .expect("data-device focus helper has a bounded body");
    let data_focus = &COMPOSITOR[data_focus_start..data_focus_end];
    assert!(data_focus.contains("kwin_surface"));
    assert!(data_focus.contains("set_data_device_focus::<State>"));
    assert!(!data_focus.contains("toplevel_surfaces"));

    let input_start = EVENT_HANDLER
        .find("CentralizedEvent::Input(event) =>")
        .expect("centralized input branch is present");
    let input_end = EVENT_HANDLER[input_start..]
        .find("\n        CentralizedEvent::Focus(")
        .map(|offset| input_start + offset)
        .expect("centralized input branch has a bounded body");
    let input_branch = &EVENT_HANDLER[input_start..input_end];
    assert!(
        input_branch
            .find("sync_kwin_seat_focus")
            .expect("input restores seat focus")
            < input_branch
                .find("process_android_clipboard")
                .expect("input drains clipboard before forwarding")
    );
    assert!(EVENT_HANDLER.contains("keyboard_focus"));
    assert!(SMITHAY_KEYBOARD.contains("No client currently focused"));

    let dispatch_start = EVENT_HANDLER
        .find("pub fn dispatch_wayland")
        .expect("Wayland dispatch helper is present");
    let dispatch = &EVENT_HANDLER[dispatch_start..];
    assert!(
        dispatch
            .find("sync_kwin_seat_focus")
            .expect("dispatch synchronizes seat focus")
            < dispatch
                .find(".flush_clients()")
                .expect("dispatch flushes after seat focus")
    );
}
