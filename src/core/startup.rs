//! Generation-safe desktop readiness state.
//!
//! A process existing is not evidence that a nested desktop is usable.  The
//! host must observe the Wayland client, surface/configure exchange, committed
//! buffer and a successfully presented Android frame before allowing the UI to
//! leave its starting state.  Events are accepted only in order and every new
//! KWin connection starts a new generation, so stale evidence cannot satisfy
//! a later session.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StartupReadiness {
    /// Monotonic readiness stage: 0 empty, 1 connected, 2 surface, 3 buffer,
    /// 4 presented. This is intentionally private so callers use the guarded
    /// transition methods below.
    stage: u8,
    pub kwin_connected: bool,
    pub surface_created: bool,
    pub buffer_committed: bool,
    pub frame_presented: bool,
}

impl StartupReadiness {
    pub const fn new() -> Self {
        Self {
            stage: 0,
            kwin_connected: false,
            surface_created: false,
            buffer_committed: false,
            frame_presented: false,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn mark_kwin_connected(&mut self) {
        // A new KWin connection starts a fresh readiness generation. Any
        // buffers/frames observed before it belong to an older client.
        self.stage = 1;
        self.kwin_connected = true;
        self.surface_created = false;
        self.buffer_committed = false;
        self.frame_presented = false;
    }

    pub fn mark_surface_created(&mut self) {
        if self.stage == 1 {
            self.stage = 2;
            self.surface_created = true;
        }
    }

    pub fn mark_buffer_committed(&mut self) {
        if self.stage == 2 {
            self.stage = 3;
            self.buffer_committed = true;
        }
    }

    pub fn mark_frame_presented(&mut self) {
        if self.stage == 3 {
            self.stage = 4;
            self.frame_presented = true;
        }
    }

    pub const fn is_ready(&self) -> bool {
        self.stage == 4
            && self.kwin_connected
            && self.surface_created
            && self.buffer_committed
            && self.frame_presented
    }

    pub fn missing(&self) -> &'static [&'static str] {
        match self.stage {
            0 => &["kwin-connected"],
            1 => &["surface-created"],
            2 => &["buffer-committed"],
            3 => &["android-frame-presented"],
            _ => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StartupReadiness;

    #[test]
    fn readiness_requires_every_boundary_in_order() {
        let mut readiness = StartupReadiness::new();
        assert!(!readiness.is_ready());
        assert_eq!(readiness.missing(), &["kwin-connected"]);

        readiness.mark_kwin_connected();
        assert_eq!(readiness.missing(), &["surface-created"]);
        readiness.mark_surface_created();
        assert_eq!(readiness.missing(), &["buffer-committed"]);
        readiness.mark_buffer_committed();
        assert_eq!(readiness.missing(), &["android-frame-presented"]);
        readiness.mark_frame_presented();
        assert!(readiness.is_ready());
        assert!(readiness.missing().is_empty());
    }

    #[test]
    fn stale_out_of_order_events_never_make_a_session_ready() {
        let mut readiness = StartupReadiness::new();
        readiness.mark_frame_presented();
        readiness.mark_buffer_committed();
        readiness.mark_surface_created();
        assert!(!readiness.is_ready());
        assert_eq!(readiness.missing(), &["kwin-connected"]);

        // A new connection invalidates every downstream event from the old
        // generation; each stage must now arrive in sequence again.
        readiness.mark_kwin_connected();
        readiness.mark_frame_presented();
        assert!(!readiness.is_ready());
        assert_eq!(readiness.missing(), &["surface-created"]);
    }

    #[test]
    fn a_new_connection_discards_previous_session_evidence() {
        let mut readiness = StartupReadiness::new();
        readiness.mark_kwin_connected();
        readiness.mark_surface_created();
        readiness.mark_buffer_committed();
        readiness.mark_frame_presented();
        assert!(readiness.is_ready());

        readiness.mark_kwin_connected();
        assert!(!readiness.is_ready());
        assert_eq!(readiness.missing(), &["surface-created"]);
    }

    #[test]
    fn reset_never_leaves_a_stale_ready_state() {
        let mut readiness = StartupReadiness::new();
        readiness.mark_kwin_connected();
        readiness.mark_surface_created();
        readiness.mark_buffer_committed();
        readiness.mark_frame_presented();
        assert!(readiness.is_ready());
        readiness.reset();
        assert_eq!(readiness, StartupReadiness::new());
        assert!(!readiness.is_ready());
    }
}
