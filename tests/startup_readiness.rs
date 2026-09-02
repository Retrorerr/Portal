#[path = "../src/core/startup.rs"]
mod startup;

use startup::{is_kwin_wayland_title, StartupReadiness};

#[test]
fn plasma_does_not_become_ready_from_process_liveness_alone() {
    let readiness = StartupReadiness::new();
    assert!(!readiness.is_ready());
    assert_eq!(readiness.missing(), &["kwin-connected"]);
}

#[test]
fn readiness_requires_host_presented_frame_after_guest_commit() {
    let mut readiness = StartupReadiness::new();
    readiness.mark_kwin_connected();
    readiness.mark_surface_created();
    readiness.mark_buffer_committed();
    assert!(!readiness.is_ready());
    assert_eq!(readiness.missing(), &["android-frame-presented"]);
    readiness.mark_frame_presented();
    assert!(readiness.is_ready());
}

#[test]
fn strict_readiness_keeps_attempts_and_configure_ack_together() {
    let mut readiness = StartupReadiness::new();
    let generation = readiness.begin_generation();
    assert!(readiness.mark_kwin_connected_for(generation));
    assert!(readiness.mark_surface_created_for(generation));
    assert!(!readiness.mark_buffer_committed_for(generation));
    assert!(readiness.mark_configure_acked_for(generation));
    assert!(readiness.mark_buffer_committed_for(generation));
    assert!(readiness.mark_frame_presented_for(generation));
    assert!(readiness.is_ready());
}

#[test]
fn recovery_surface_title_does_not_identify_kwin() {
    assert!(is_kwin_wayland_title("KDE Wayland Compositor wayland-0"));
    assert!(!is_kwin_wayland_title("labwc"));
    assert!(!is_kwin_wayland_title("Konsole"));
}
