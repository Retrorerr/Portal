//! Generation-safe desktop readiness state.
//!
//! A process existing is not evidence that a nested desktop is usable.  The
//! host must observe the Wayland client, surface/configure exchange, committed
//! buffer and a successfully presented Android frame before allowing the UI to
//! leave its starting state.  Events are accepted only in order and every new
//! KWin connection starts a new generation, so stale evidence cannot satisfy
//! a later session.

/// The title prefix emitted by KWin's nested Wayland output backend.
///
/// KWin 6.7 creates the output xdg-toplevel with a title beginning with this
/// string.  A compositor cannot otherwise identify a peer from Wayland
/// protocol objects alone, so the Android backend uses this prefix together
/// with the owning client and surface object ids.  The trailing space avoids
/// accepting similarly named recovery applications.
pub const KWIN_WAYLAND_TITLE_PREFIX: &str = "KDE Wayland Compositor ";

/// Return whether an xdg-toplevel title identifies KWin's nested output.
pub fn is_kwin_wayland_title(title: &str) -> bool {
    title.starts_with(KWIN_WAYLAND_TITLE_PREFIX)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StartupReadiness {
    /// Monotonic readiness stage: 0 empty, 1 connected, 2 surface, 3 buffer,
    /// 4 presented. This is intentionally private so callers use the guarded
    /// transition methods below.
    stage: u8,
    /// Monotonic attempt id.  It changes whenever a newly identified KWin
    /// client/surface is observed, so callbacks from an earlier connection
    /// cannot complete a later attempt.
    generation: u64,
    pub kwin_connected: bool,
    pub surface_created: bool,
    /// At least one configure serial for the identified surface was acked.
    /// This is kept separate from `surface_created` because xdg-shell requires
    /// the ack before a buffer commit can be accepted.
    pub configure_acked: bool,
    pub buffer_committed: bool,
    pub frame_presented: bool,
}

impl StartupReadiness {
    pub const fn new() -> Self {
        Self {
            stage: 0,
            generation: 0,
            kwin_connected: false,
            surface_created: false,
            configure_acked: false,
            buffer_committed: false,
            frame_presented: false,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Return the current attempt id, or zero before an attempt starts.
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Start a new attempt and invalidate all evidence from the previous one.
    ///
    /// A zero generation is reserved for the uninitialized state.  Wrapping
    /// is handled explicitly so a very long-lived process never reuses zero.
    pub fn begin_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.generation = 1;
        }
        self.stage = 0;
        self.kwin_connected = false;
        self.surface_created = false;
        self.configure_acked = false;
        self.buffer_committed = false;
        self.frame_presented = false;
        self.generation
    }

    /// Invalidate the current attempt while retaining its id for diagnostics.
    /// A later identified client must call [`begin_generation`] before any
    /// stage can advance again.
    pub fn invalidate(&mut self) {
        self.stage = 0;
        self.kwin_connected = false;
        self.surface_created = false;
        self.configure_acked = false;
        self.buffer_committed = false;
        self.frame_presented = false;
    }

    pub fn mark_kwin_connected(&mut self) {
        let generation = self.begin_generation();
        let _ = self.mark_kwin_connected_for(generation);
    }

    /// Mark the identified KWin connection for the supplied attempt.
    pub fn mark_kwin_connected_for(&mut self, generation: u64) -> bool {
        if generation == 0 || generation != self.generation || self.stage != 0 {
            return false;
        }
        self.stage = 1;
        self.kwin_connected = true;
        true
    }

    pub fn mark_surface_created(&mut self) {
        let generation = self.generation;
        let _ = self.mark_surface_created_for(generation);
    }

    /// Mark the identified KWin surface for the supplied attempt.
    pub fn mark_surface_created_for(&mut self, generation: u64) -> bool {
        if generation == 0 || generation != self.generation || self.stage != 1 {
            return false;
        }
        self.stage = 2;
        self.surface_created = true;
        true
    }

    /// Record an xdg-shell configure acknowledgement for the identified
    /// surface.  Buffer evidence is rejected until this bit is set.
    pub fn mark_configure_acked(&mut self) {
        let generation = self.generation;
        let _ = self.mark_configure_acked_for(generation);
    }

    pub fn mark_configure_acked_for(&mut self, generation: u64) -> bool {
        if generation != 0
            && generation == self.generation
            && self.stage == 2
            && !self.configure_acked
        {
            self.configure_acked = true;
            return true;
        }
        false
    }

    pub fn mark_buffer_committed(&mut self) {
        // Compatibility helper for callers that already enforce xdg-shell's
        // configure/ack contract themselves.  The Android compositor uses
        // `mark_buffer_committed_for`, which requires an observed ack.
        self.mark_configure_acked();
        let generation = self.generation;
        let _ = self.mark_buffer_committed_for(generation);
    }

    /// Record a newly committed buffer for the identified surface.  A stale
    /// generation or a commit preceding configure ack is ignored.
    pub fn mark_buffer_committed_for(&mut self, generation: u64) -> bool {
        if generation == 0
            || generation != self.generation
            || self.stage != 2
            || !self.configure_acked
        {
            return false;
        }
        self.stage = 3;
        self.buffer_committed = true;
        true
    }

    pub fn mark_frame_presented(&mut self) {
        let generation = self.generation;
        let _ = self.mark_frame_presented_for(generation);
    }

    /// Record successful presentation evidence for the same identified
    /// surface/attempt that supplied the buffer.
    pub fn mark_frame_presented_for(&mut self, generation: u64) -> bool {
        if generation == 0 || generation != self.generation || self.stage != 3 {
            return false;
        }
        self.stage = 4;
        self.frame_presented = true;
        true
    }

    pub const fn is_ready(&self) -> bool {
        self.stage == 4
            && self.kwin_connected
            && self.surface_created
            && self.configure_acked
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
    use super::{is_kwin_wayland_title, StartupReadiness};

    #[test]
    fn readiness_requires_every_boundary_in_order() {
        let mut readiness = StartupReadiness::new();
        assert!(!readiness.is_ready());
        assert_eq!(readiness.missing(), &["kwin-connected"]);

        readiness.mark_kwin_connected();
        assert_eq!(readiness.missing(), &["surface-created"]);
        readiness.mark_surface_created();
        assert_eq!(readiness.missing(), &["buffer-committed"]);
        assert!(!readiness.configure_acked);
        readiness.mark_buffer_committed();
        assert!(readiness.configure_acked);
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

    #[test]
    fn stale_generation_events_cannot_complete_a_new_attempt() {
        let mut readiness = StartupReadiness::new();
        let first = readiness.begin_generation();
        assert!(readiness.mark_kwin_connected_for(first));
        assert!(readiness.mark_surface_created_for(first));
        assert!(readiness.mark_configure_acked_for(first));

        let second = readiness.begin_generation();
        assert_ne!(first, second);
        assert!(!readiness.mark_buffer_committed_for(first));
        assert!(!readiness.mark_frame_presented_for(first));
        assert!(!readiness.is_ready());

        assert!(readiness.mark_kwin_connected_for(second));
        assert!(readiness.mark_surface_created_for(second));
        assert!(readiness.mark_configure_acked_for(second));
        assert!(readiness.mark_buffer_committed_for(second));
        assert!(readiness.mark_frame_presented_for(second));
        assert!(readiness.is_ready());
    }

    #[test]
    fn buffer_requires_a_configure_ack_in_strict_api() {
        let mut readiness = StartupReadiness::new();
        let generation = readiness.begin_generation();
        assert!(readiness.mark_kwin_connected_for(generation));
        assert!(readiness.mark_surface_created_for(generation));
        assert!(!readiness.mark_buffer_committed_for(generation));
        assert!(!readiness.buffer_committed);
        assert!(readiness.mark_configure_acked_for(generation));
        assert!(readiness.mark_buffer_committed_for(generation));
    }

    #[test]
    fn only_kwin_nested_title_is_an_identity_match() {
        assert!(is_kwin_wayland_title("KDE Wayland Compositor Local Desktop"));
        assert!(is_kwin_wayland_title("KDE Wayland Compositor Local Desktop - Output disabled"));
        assert!(!is_kwin_wayland_title("labwc"));
        assert!(!is_kwin_wayland_title("KDE Wayland Compositorist"));
    }
}
