//! Host-testable policy for the external Android clipboard → Wayland path.
//!
//! The Android `ClipboardManager` is owned by the system, while the nested
//! compositor owns the Wayland selection. A background worker observes Android
//! (Binder reads) and the event loop applies completed observations to
//! `wl_data_device` and flushes before forwarding input. This module owns the
//! change detection, one-shot echo suppression, explicit resume resync, and
//! pending-queue ordering so the same logic is exercised on the host and on
//! the device.
//!
//! Design notes:
//! - `None` means cleared/unavailable and is distinct from "no change".
//! - Empty text normalizes to `None` so a guest-initiated clear can suppress
//!   its own echo (the Binder read coerces empty to `None`).
//! - Binder failures (`Err`) never manufacture a clear and never clear a
//!   pending resync request; Android denies reads while backgrounded.
//! - `request_resync` does not force a duplicate event. It only ensures the
//!   next successful read is evaluated promptly (the worker wakes early
//!   instead of waiting for its interval). If the value is unchanged, no
//!   event is queued.
//! - The pending queue must be drained and flushed to Wayland *before* later
//!   keyboard/paste input is forwarded, otherwise Ctrl+V reaches KWin with a
//!   stale selection.

use std::collections::VecDeque;

/// Normalize an Android observation: empty text is a clear.
fn normalize(value: Option<String>) -> Option<String> {
    match value {
        Some(text) if text.is_empty() => None,
        other => other,
    }
}

/// Short single-line preview for diagnostic logging: first 24 chars with
/// newlines escaped, plus byte length available at the call site. Keeps
/// logcat/host.log bounded while still identifying test strings.
pub fn clip_preview(text: &str) -> String {
    text.chars()
        .take(24)
        .collect::<String>()
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}
/// One-shot echo + change detector + pending queue for Android → Wayland.
///
/// `last_seen` tracks the last successfully observed Android value:
/// - outer `None` = never observed (startup),
/// - outer `Some(inner)` = last observed value (`inner == None` is cleared).
///
/// `pending_echo` tracks a guest → Android write awaiting its echo:
/// - outer `None` = no echo pending,
/// - outer `Some(inner)` = suppress exactly one observation of `inner`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ExternalClipboardSync {
    last_seen: Option<Option<String>>,
    pending_echo: Option<Option<String>>,
    resync_requested: bool,
    pending: VecDeque<Option<String>>,
}

impl ExternalClipboardSync {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a guest → Android write so its echo is suppressed exactly once.
    ///
    /// Empty text normalizes to a clear (`None`) to match the Binder read
    /// path, which coerces empty to `None`.
    pub fn mark_guest_write(&mut self, value: Option<String>) {
        self.pending_echo = Some(normalize(value));
    }

    /// Clear a pending echo only when it matches `value`.
    ///
    /// Used when a guest → Android Binder write fails: the write did not
    /// land, so a matching echo must not suppress the next real observation.
    /// A concurrent newer guest write is left untouched.
    pub fn clear_echo_for(&mut self, value: Option<String>) {
        let normalized = normalize(value);
        if self.pending_echo.as_ref() == Some(&normalized) {
            self.pending_echo = None;
        }
    }

    /// Request an immediate resync (Activity resume / focus regain).
    ///
    /// Non-blocking and idempotent. The worker wakes early for one prompt
    /// read; the flag survives Binder denials and is cleared by the next
    /// successful read.
    pub fn request_resync(&mut self) {
        self.resync_requested = true;
    }

    pub fn needs_resync(&self) -> bool {
        self.resync_requested
    }

    pub fn last_seen(&self) -> Option<Option<String>> {
        self.last_seen.clone()
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Observe one worker poll result.
    ///
    /// - `Err(())` = Binder denied/unavailable: preserve `last_seen` and any
    ///   pending resync; queue nothing, request no wake.
    /// - `Ok(current)` = successful read: consume a matching echo (no queue),
    ///   otherwise queue when changed since `last_seen` (including the first
    ///   non-clear observation). The initial cleared state never queues a
    ///   spurious clear.
    ///
    /// Returns `true` when a new update was queued and the event loop must
    /// wake immediately with the dedicated clipboard event.
    pub fn observe_poll(&mut self, result: Result<Option<String>, ()>) -> bool {
        let current = match result {
            Err(()) => return false,
            Ok(value) => normalize(value),
        };
        // A successful read satisfies any outstanding resync, even when the
        // value is unchanged (no duplicate event is queued in that case).
        if self.resync_requested {
            self.resync_requested = false;
        }
        // One-shot echo: consume exactly one matching observation, including
        // a clear echo. A later legitimate copy of the same text is still
        // delivered because the marker is gone.
        if self.pending_echo.as_ref() == Some(&current) {
            self.pending_echo = None;
            self.last_seen = Some(current);
            return false;
        }
        match &self.last_seen {
            // Startup with a cleared clipboard is already the Wayland state:
            // record it without queuing a spurious clear.
            None if current.is_none() => {
                self.last_seen = Some(current);
                false
            }
            Some(previous) if *previous == current => false,
            _ => {
                self.last_seen = Some(current.clone());
                self.pending.push_back(current);
                true
            }
        }
    }

    /// Drain queued Android updates in order.
    ///
    /// The caller must apply each value to `wl_data_device` (set for
    /// `Some`, clear for `None`), flush Wayland, and only then forward
    /// later keyboard/paste input. Draining clears the queue.
    pub fn drain_pending(&mut self) -> Vec<Option<String>> {
        self.pending.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_cleared_observation_queues_nothing_but_first_text_does() {
        let mut sync = ExternalClipboardSync::new();
        assert!(!sync.observe_poll(Ok(None)));
        assert!(!sync.has_pending());
        assert_eq!(sync.last_seen(), Some(None));

        assert!(sync.observe_poll(Ok(Some("hello".to_owned()))));
        assert_eq!(sync.drain_pending(), vec![Some("hello".to_owned())]);
        assert!(!sync.has_pending());
    }

    #[test]
    fn unchanged_values_never_queue() {
        let mut sync = ExternalClipboardSync::new();
        assert!(sync.observe_poll(Ok(Some("a".to_owned()))));
        assert_eq!(sync.drain_pending(), vec![Some("a".to_owned())]);
        assert!(!sync.observe_poll(Ok(Some("a".to_owned()))));
        assert!(!sync.has_pending());
    }

    #[test]
    fn guest_echo_is_suppressed_exactly_once() {
        let mut sync = ExternalClipboardSync::new();
        assert!(sync.observe_poll(Ok(Some("old".to_owned()))));
        sync.drain_pending();

        sync.mark_guest_write(Some("guest-text".to_owned()));
        // Echo of our own write: consumed, not queued.
        assert!(!sync.observe_poll(Ok(Some("guest-text".to_owned()))));
        assert!(!sync.has_pending());
        // A later legitimate copy of the same text is not discarded.
        assert!(sync.observe_poll(Ok(Some("other".to_owned()))));
        sync.drain_pending();
        assert!(sync.observe_poll(Ok(Some("guest-text".to_owned()))));
        assert_eq!(
            sync.drain_pending(),
            vec![Some("guest-text".to_owned())]
        );
    }

    #[test]
    fn empty_guest_clear_suppresses_binder_clear_echo() {
        let mut sync = ExternalClipboardSync::new();
        assert!(sync.observe_poll(Ok(Some("old".to_owned()))));
        sync.drain_pending();

        sync.mark_guest_write(Some(String::new()));
        assert!(!sync.observe_poll(Ok(None)));
        assert!(!sync.has_pending());
    }

    #[test]
    fn failed_write_does_not_suppress_next_real_observation() {
        let mut sync = ExternalClipboardSync::new();
        assert!(sync.observe_poll(Ok(Some("old".to_owned()))));
        sync.drain_pending();

        sync.mark_guest_write(Some("unsent".to_owned()));
        sync.clear_echo_for(Some("unsent".to_owned()));
        assert!(sync.observe_poll(Ok(Some("unsent".to_owned()))));
        assert_eq!(sync.drain_pending(), vec![Some("unsent".to_owned())]);
    }

    #[test]
    fn binder_denial_preserves_state_and_resync() {
        let mut sync = ExternalClipboardSync::new();
        assert!(sync.observe_poll(Ok(Some("old".to_owned()))));
        sync.drain_pending();

        sync.request_resync();
        assert!(sync.needs_resync());
        // Denied while backgrounded: nothing queued, resync survives.
        assert!(!sync.observe_poll(Err(())));
        assert!(sync.needs_resync());
        assert!(!sync.has_pending());

        // Unchanged after resume: resync clears, still nothing queued.
        assert!(!sync.observe_poll(Ok(Some("old".to_owned()))));
        assert!(!sync.needs_resync());
        assert!(!sync.has_pending());
    }
}
