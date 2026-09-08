#[path = "../src/core/wayland_protocol.rs"]
mod wayland_protocol;

use wayland_protocol::{FrameEvent, FrameTrace, ProtocolViolation};

#[test]
fn next_frame_is_scheduled_before_blocking_submit() {
    let mut trace = FrameTrace::new();
    for event in [
        FrameEvent::Dispatch,
        FrameEvent::Render,
        FrameEvent::FrameDone,
        FrameEvent::Submit,
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
            FrameEvent::FrameDone,
            FrameEvent::Submit,
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
fn callbacks_can_precede_submit_but_presented_feedback_cannot() {
    let mut trace = FrameTrace::new();
    trace.record(FrameEvent::Dispatch).unwrap();
    trace.record(FrameEvent::Render).unwrap();

    trace.record(FrameEvent::FrameDone).unwrap();
    assert_eq!(
        trace.record(FrameEvent::Presented),
        Err(ProtocolViolation::MissingPrerequisite {
            event: FrameEvent::Presented,
            prerequisite: FrameEvent::Submit,
        })
    );
}

#[test]
fn undrawn_frame_can_advance_callback_and_discard_feedback() {
    let mut trace = FrameTrace::new();
    for event in [
        FrameEvent::Dispatch,
        FrameEvent::Render,
        FrameEvent::FrameDone,
        FrameEvent::Discarded,
    ] {
        trace.record(event).unwrap();
    }
    assert!(trace.completed());
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
        FrameEvent::FrameDone,
        FrameEvent::Submit,
    ] {
        trace.record(event).unwrap();
    }

    assert_eq!(
        trace.record(FrameEvent::FrameDone),
        Err(ProtocolViolation::Duplicate(FrameEvent::FrameDone))
    );
}
