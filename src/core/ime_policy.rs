//! Host-testable policy for bounding Android software-keyboard commits and for recognizing the
//! desktop taps that bring a dismissed keyboard back.
//!
//! The JNI callback runs on an Android-attached thread while the compositor drains commits from
//! the winit event loop.  Keeping the queue policy independent of Android makes its memory and
//! UTF-8 guarantees testable in the normal host suite as well.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Maximum number of pending commits retained while the event loop is suspended.
pub const MAX_PENDING_COMMITS: usize = 128;
/// Maximum total UTF-8 bytes retained by the pending commit queue.
pub const MAX_PENDING_BYTES: usize = 256 * 1024;
/// Maximum size of one commit, measured in UTF-8 bytes.
pub const MAX_COMMIT_BYTES: usize = 64 * 1024;

/// Bounded FIFO of committed text from the Android software keyboard.
#[derive(Debug, Default)]
pub struct CommitQueue {
    entries: VecDeque<String>,
    total_bytes: usize,
}

impl CommitQueue {
    /// Add a commit after truncating it at a UTF-8 boundary.  Old entries are evicted first when
    /// either the entry or total-byte limit would be exceeded.  Empty commits are ignored.
    pub fn push(&mut self, mut text: String) -> bool {
        if text.len() > MAX_COMMIT_BYTES {
            let mut end = MAX_COMMIT_BYTES;
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        if text.is_empty() {
            return false;
        }

        let text_bytes = text.len();
        while self.entries.len() >= MAX_PENDING_COMMITS
            || self.total_bytes.saturating_add(text_bytes) > MAX_PENDING_BYTES
        {
            let Some(oldest) = self.entries.pop_front() else {
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(oldest.len());
        }
        self.total_bytes = self.total_bytes.saturating_add(text_bytes);
        self.entries.push_back(text);
        true
    }

    /// Remove all pending commits in FIFO order.
    pub fn drain(&mut self) -> Vec<String> {
        self.total_bytes = 0;
        self.entries.drain(..).collect()
    }

    /// Drop all pending commits.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    #[cfg(test)]
    fn front(&self) -> Option<&str> {
        self.entries.front().map(String::as_str)
    }
}

/// Longest press still treated as a tap on a text field.
pub const TAP_MAX_DURATION: Duration = Duration::from_millis(500);
/// Farthest a tapping finger may wander, in physical pixels.
pub const TAP_SLOP_PX: f64 = 48.0;
/// Pointers without an end event for this long are taken as lost.
const STALE_POINTER: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy)]
struct TapCandidate {
    id: u64,
    x: f64,
    y: f64,
    started: Instant,
}

/// Recognizes single-pointer taps (not drags, scrolls, long presses or multi-finger gestures),
/// after which a keyboard the user dismissed over a focused guest field comes back.
#[derive(Debug)]
pub struct TapDetector {
    down: Vec<(u64, Instant)>,
    candidate: Option<TapCandidate>,
}

impl Default for TapDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl TapDetector {
    pub const fn new() -> Self {
        Self {
            down: Vec::new(),
            candidate: None,
        }
    }

    pub fn down(&mut self, id: u64, x: f64, y: f64, now: Instant) {
        // A lost end event must not block taps for the rest of the session.
        self.down
            .retain(|&(down_id, at)| down_id != id && now.duration_since(at) < STALE_POINTER);
        self.down.push((id, now));
        self.candidate = (self.down.len() == 1).then_some(TapCandidate {
            id,
            x,
            y,
            started: now,
        });
    }

    pub fn moved(&mut self, id: u64, x: f64, y: f64) {
        if let Some(candidate) = self.candidate {
            if candidate.id == id && (x - candidate.x).hypot(y - candidate.y) > TAP_SLOP_PX {
                self.candidate = None;
            }
        }
    }

    /// The pointer lifted; true when this completed a tap.
    pub fn up(&mut self, id: u64, now: Instant) -> bool {
        self.down.retain(|&(down_id, _)| down_id != id);
        match self.candidate {
            Some(candidate) if candidate.id == id => {
                self.candidate = None;
                now.duration_since(candidate.started) <= TAP_MAX_DURATION
            }
            _ => false,
        }
    }

    pub fn cancel(&mut self, id: u64) {
        self.down.retain(|&(down_id, _)| down_id != id);
        if self.candidate.is_some_and(|candidate| candidate.id == id) {
            self.candidate = None;
        }
    }

    pub fn clear(&mut self) {
        self.down.clear();
        self.candidate = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_still_press_is_a_tap() {
        let t0 = Instant::now();
        let mut taps = TapDetector::new();
        taps.down(0, 100.0, 100.0, t0);
        taps.moved(0, 110.0, 95.0);
        assert!(taps.up(0, t0 + Duration::from_millis(120)));
    }

    #[test]
    fn drag_long_press_and_two_fingers_are_not_taps() {
        let t0 = Instant::now();
        let mut taps = TapDetector::new();
        taps.down(0, 100.0, 100.0, t0);
        taps.moved(0, 100.0, 100.0 + TAP_SLOP_PX + 1.0);
        assert!(!taps.up(0, t0 + Duration::from_millis(100)));

        taps.down(0, 100.0, 100.0, t0);
        assert!(!taps.up(0, t0 + TAP_MAX_DURATION + Duration::from_millis(1)));

        taps.down(0, 100.0, 100.0, t0);
        taps.down(1, 300.0, 100.0, t0);
        assert!(!taps.up(1, t0 + Duration::from_millis(80)));
        assert!(!taps.up(0, t0 + Duration::from_millis(90)));
    }

    #[test]
    fn lost_end_event_does_not_block_later_taps() {
        let t0 = Instant::now();
        let mut taps = TapDetector::new();
        taps.down(5, 0.0, 0.0, t0);
        let later = t0 + STALE_POINTER;
        taps.down(0, 10.0, 10.0, later);
        assert!(taps.up(0, later + Duration::from_millis(50)));

        // A cancelled gesture is not a tap either.
        taps.down(0, 10.0, 10.0, later);
        taps.cancel(0);
        assert!(!taps.up(0, later + Duration::from_millis(60)));
    }

    #[test]
    fn queue_evicts_old_entries_at_entry_limit() {
        let mut queue = CommitQueue::default();
        for index in 0..=MAX_PENDING_COMMITS {
            assert!(queue.push(index.to_string()));
        }

        assert_eq!(queue.len(), MAX_PENDING_COMMITS);
        assert_eq!(queue.front(), Some("1"));
        assert!(queue.total_bytes() <= MAX_PENDING_BYTES);
    }

    #[test]
    fn queue_evicts_old_entries_at_total_byte_limit() {
        let mut queue = CommitQueue::default();
        let commit = "x".repeat(MAX_COMMIT_BYTES);
        for _ in 0..=MAX_PENDING_BYTES / MAX_COMMIT_BYTES {
            assert!(queue.push(commit.clone()));
        }

        assert!(queue.total_bytes() <= MAX_PENDING_BYTES);
        assert!(queue.len() <= MAX_PENDING_COMMITS);
    }

    #[test]
    fn oversized_commit_is_truncated_at_utf8_boundary() {
        let mut queue = CommitQueue::default();
        assert!(queue.push("é".repeat(MAX_COMMIT_BYTES)));

        let drained = queue.drain();
        assert_eq!(drained.len(), 1);
        assert!(drained[0].len() <= MAX_COMMIT_BYTES);
        assert!(drained[0].is_char_boundary(drained[0].len()));
        assert_eq!(queue.total_bytes(), 0);
    }

    #[test]
    fn empty_commit_is_not_retained() {
        let mut queue = CommitQueue::default();
        assert!(!queue.push(String::new()));
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn oversized_commit_with_3byte_and_4byte_utf8_truncates_safely() {
        let mut queue = CommitQueue::default();
        // 3-byte character '中' (0xE4 0xB8 0xAD)
        let count_3byte = (MAX_COMMIT_BYTES / 3) + 10;
        let text_3byte = "中".repeat(count_3byte);
        assert!(queue.push(text_3byte));
        let drained = queue.drain();
        assert_eq!(drained.len(), 1);
        assert!(drained[0].len() <= MAX_COMMIT_BYTES);
        assert!(drained[0].is_char_boundary(drained[0].len()));

        // 4-byte emoji '🦀' (0xF0 0x9F 0xA6 0x80)
        let count_4byte = (MAX_COMMIT_BYTES / 4) + 10;
        let text_4byte = "🦀".repeat(count_4byte);
        assert!(queue.push(text_4byte));
        let drained = queue.drain();
        assert_eq!(drained.len(), 1);
        assert!(drained[0].len() <= MAX_COMMIT_BYTES);
        assert!(drained[0].is_char_boundary(drained[0].len()));
    }

    #[test]
    fn clear_resets_queue_state_and_bytes() {
        let mut queue = CommitQueue::default();
        queue.push("abc".into());
        queue.push("def".into());
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.total_bytes(), 6);

        queue.clear();
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.total_bytes(), 0);
        assert!(queue.drain().is_empty());
    }
}
