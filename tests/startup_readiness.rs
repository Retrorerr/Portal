#[path = "../src/core/startup.rs"]
mod startup;

use startup::StartupReadiness;

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
