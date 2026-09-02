#[path = "../src/core/wayland_protocol.rs"]
mod wayland_protocol;

use wayland_protocol::{FrameEvent, FrameTrace, ProtocolViolation};

#[test]
fn frame_done_and_presentation_follow_the_submitted_frame() {
    let mut trace = FrameTrace::new();
    for event in [
        FrameEvent::Dispatch,
        FrameEvent::Render,
        FrameEvent::Submit,
        FrameEvent::FrameDone,
        FrameEvent::Presented,
    ] {
        trace
            .record(event)
            .expect("valid nested compositor frame order");
    }

    assert!(trace.completed());
    assert_eq!(
        trace.events(),
        &[
            FrameEvent::Dispatch,
            FrameEvent::Render,
            FrameEvent::Submit,
            FrameEvent::FrameDone,
            FrameEvent::Presented,
        ]
    );
}

#[test]
fn old_render_dispatch_order_is_rejected() {
    let mut trace = FrameTrace::new();
    trace
        .record(FrameEvent::Render)
        .expect_err("render before dispatch");
    assert_eq!(trace.events(), &[]);

    trace.record(FrameEvent::Dispatch).unwrap();
    trace.record(FrameEvent::Render).unwrap();
    trace.record(FrameEvent::Submit).unwrap();
    trace.record(FrameEvent::Presented).unwrap();
}

#[test]
fn callbacks_and_feedback_cannot_precede_submit() {
    let mut trace = FrameTrace::new();
    trace.record(FrameEvent::Dispatch).unwrap();
    trace.record(FrameEvent::Render).unwrap();

    assert_eq!(
        trace.record(FrameEvent::FrameDone),
        Err(ProtocolViolation::MissingPrerequisite {
            event: FrameEvent::FrameDone,
            prerequisite: FrameEvent::Submit,
        })
    );
    assert_eq!(
        trace.record(FrameEvent::Presented),
        Err(ProtocolViolation::MissingPrerequisite {
            event: FrameEvent::Presented,
            prerequisite: FrameEvent::Submit,
        })
    );
}

#[test]
fn presentation_feedback_has_one_terminal_result() {
    let mut trace = FrameTrace::new();
    for event in [FrameEvent::Dispatch, FrameEvent::Render, FrameEvent::Submit] {
        trace.record(event).unwrap();
    }
    trace.record(FrameEvent::Discarded).unwrap();
    assert_eq!(
        trace.record(FrameEvent::Presented),
        Err(ProtocolViolation::ConflictingPresentation)
    );
    assert!(trace.completed());
}

#[test]
fn duplicate_frame_done_is_rejected() {
    let mut trace = FrameTrace::new();
    for event in [
        FrameEvent::Dispatch,
        FrameEvent::Render,
        FrameEvent::Submit,
        FrameEvent::FrameDone,
    ] {
        trace.record(event).unwrap();
    }

    assert_eq!(
        trace.record(FrameEvent::FrameDone),
        Err(ProtocolViolation::Duplicate(FrameEvent::FrameDone))
    );
}
