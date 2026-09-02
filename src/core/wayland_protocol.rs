//! Host-testable invariants for the nested Wayland frame lifecycle.
//!
//! The Android compositor is only compiled for Android, which makes it easy for a regression in
//! its request/render ordering to escape the normal host test suite.  This small, dependency-free
//! trace is used by the compositor as a debug guard and by host tests to exercise the ordering
//! contract without requiring an Android device or a GPU.

/// The observable stages of one host-composited frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameEvent {
    /// Wayland client requests (including commits) have been dispatched.
    Dispatch,
    /// The current committed surfaces have been imported and drawn.
    Render,
    /// The Android/EGL surface accepted the rendered frame.
    Submit,
    /// `wl_surface.frame` callbacks were sent for the submitted frame.
    FrameDone,
    /// Presentation feedback was completed as presented.
    Presented,
    /// Presentation feedback was completed as discarded.
    Discarded,
}

/// A violation of the host compositor's frame ordering contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolViolation {
    /// An event was recorded more than once in a frame.
    Duplicate(FrameEvent),
    /// An event was recorded before its prerequisite.
    MissingPrerequisite {
        event: FrameEvent,
        prerequisite: FrameEvent,
    },
    /// A second terminal presentation result was attempted.
    ConflictingPresentation,
}

impl std::fmt::Display for ProtocolViolation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duplicate(event) => write!(formatter, "duplicate frame event: {event:?}"),
            Self::MissingPrerequisite {
                event,
                prerequisite,
            } => write!(
                formatter,
                "frame event {event:?} requires {prerequisite:?} first"
            ),
            Self::ConflictingPresentation => {
                formatter.write_str("frame has both presented and discarded feedback")
            }
        }
    }
}

/// A bounded, allocation-free trace for one compositor frame.
///
/// A trace is intentionally local to a redraw.  That makes it impossible for a callback or
/// presentation event from one submitted frame to satisfy the prerequisites of another frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameTrace {
    events: [FrameEvent; 6],
    len: usize,
}

impl Default for FrameTrace {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameTrace {
    pub const fn new() -> Self {
        Self {
            events: [FrameEvent::Dispatch; 6],
            len: 0,
        }
    }

    /// Record a stage, rejecting duplicate or out-of-order protocol events.
    pub fn record(&mut self, event: FrameEvent) -> Result<(), ProtocolViolation> {
        if self.contains(event) {
            return Err(ProtocolViolation::Duplicate(event));
        }

        match event {
            FrameEvent::Dispatch => {
                if self.len != 0 {
                    return Err(ProtocolViolation::MissingPrerequisite {
                        event,
                        prerequisite: FrameEvent::Dispatch,
                    });
                }
            }
            FrameEvent::Render => self.require(FrameEvent::Dispatch, event)?,
            FrameEvent::Submit => self.require(FrameEvent::Render, event)?,
            FrameEvent::FrameDone => self.require(FrameEvent::Submit, event)?,
            FrameEvent::Presented | FrameEvent::Discarded => {
                self.require(FrameEvent::Submit, event)?;
                if self.contains(FrameEvent::Presented) || self.contains(FrameEvent::Discarded) {
                    return Err(ProtocolViolation::ConflictingPresentation);
                }
            }
        }

        // The trace has six distinct events, so this cannot overflow unless a new event is added
        // without extending the fixed capacity above.
        self.events[self.len] = event;
        self.len += 1;
        Ok(())
    }

    pub fn events(&self) -> &[FrameEvent] {
        &self.events[..self.len]
    }

    pub fn contains(&self, event: FrameEvent) -> bool {
        let mut index = 0;
        while index < self.len {
            if self.events[index] == event {
                return true;
            }
            index += 1;
        }
        false
    }

    pub fn completed(&self) -> bool {
        self.contains(FrameEvent::Presented) || self.contains(FrameEvent::Discarded)
    }

    fn require(
        &self,
        prerequisite: FrameEvent,
        event: FrameEvent,
    ) -> Result<(), ProtocolViolation> {
        if self.contains(prerequisite) {
            Ok(())
        } else {
            Err(ProtocolViolation::MissingPrerequisite {
                event,
                prerequisite,
            })
        }
    }
}
