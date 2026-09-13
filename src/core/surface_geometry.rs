//! Authoritative Android surface-geometry convergence (rotation).
//!
//! Pure state machine, no Android APIs: it decides when a physical size
//! change is real, belongs to the current native-window epoch, and is agreed
//! by both size sources (the winit resize event size and the live
//! `ANativeWindow` size). A genuine transition mints exactly one surface
//! generation; the caller performs the single rebind for it and confirms
//! afterwards.
//!
//! Rules (mirroring the Smithay resize contract, extended for two sources):
//! - Invalid sizes (`0` in either dimension, negative at the call edge) are
//!   ignored: the surface is transiently unavailable, never a 1px desktop.
//! - Notes from an older native-window epoch are stale (delayed events from
//!   a destroyed surface) and can never win over newer geometry.
//! - Identical repeats coalesce: no new generation, no work.
//! - The two sources must AGREE before anything happens. While they
//!   disagree (rotation delivers events in awkward order), the newest
//!   desired geometry is recorded and convergence retries on the next
//!   surface/resize/resume event. No sleeps, no debounce timers.
//! - A newer size supersedes an older pending one; each emitted rebind gets
//!   a fresh generation id so stale completions are identifiable.
//!
//! The machine is owned by the lifecycle (one instance across sessions) and
//! re-based with [`SurfaceConvergence::begin_epoch`] whenever a new native
//! window/session starts. Surface generation 1 is the session start itself;
//! the first rotation mints generation 2.

use std::sync::atomic::{AtomicU64, Ordering};

/// Process-wide native-window epoch. Minted once per Anland session start;
/// delayed resize events from a previous window carry the old epoch and are
/// rejected as stale.
static SURFACE_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Mint a fresh surface epoch for a new native window/session.
pub fn mint_surface_epoch() -> u64 {
    // Epoch 0 means "no session yet"; real epochs start at 1.
    SURFACE_EPOCH.fetch_add(1, Ordering::AcqRel) + 1
}

/// A validated physical surface size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceSize {
    pub w: u32,
    pub h: u32,
}

impl SurfaceSize {
    pub fn new(w: u32, h: u32) -> Option<Self> {
        if w > 0 && h > 0 {
            Some(Self { w, h })
        } else {
            None
        }
    }
}

/// What a size observation decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvergenceAction {
    /// Zero/invalid dimensions: transient surface, state preserved.
    IgnoredInvalid,
    /// Belongs to a destroyed native-window epoch: can never win.
    IgnoredStaleEpoch,
    /// Same as converged (or already emitted for this pending size): no work.
    CoalescedIdentical,
    /// Recorded, but the two sources disagree yet: retry on the next event.
    Recorded,
    /// Both sources agree on a new size: perform exactly one rebind for it.
    Rebind {
        /// Fresh surface generation for this transition.
        gen: u64,
        size: SurfaceSize,
    },
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    size: SurfaceSize,
    /// Whether a rebind was already emitted for this size. Cleared by
    /// [`SurfaceConvergence::abort_pending`] after a failed attempt so the
    /// next agreeing event re-emits (with a new generation); replaced
    /// outright when a newer size arrives.
    emitted: bool,
}

/// Authoritative surface-geometry convergence across sessions.
#[derive(Debug)]
pub struct SurfaceConvergence {
    epoch: u64,
    winit: Option<(SurfaceSize, u64)>,
    native: Option<(SurfaceSize, u64)>,
    converged: Option<(SurfaceSize, u64)>,
    pending: Option<Pending>,
    next_gen: u64,
}

impl SurfaceConvergence {
    pub fn new() -> Self {
        Self {
            epoch: 0,
            winit: None,
            native: None,
            converged: None,
            pending: None,
            // Generation 1 is assigned by begin_epoch to the session start.
            next_gen: 2,
        }
    }

    /// Re-base onto a new native window/session: forget every observation,
    /// adopt the epoch, and treat `size` as converged generation 1.
    pub fn begin_epoch(&mut self, epoch: u64, size: SurfaceSize) {
        self.epoch = epoch;
        self.winit = None;
        self.native = None;
        self.converged = Some((size, 1));
        self.pending = None;
        self.next_gen = 2;
    }

    /// Currently converged size and surface generation, if any session began.
    pub fn converged(&self) -> Option<(SurfaceSize, u64)> {
        self.converged
    }

    /// Newest agreed-but-unconverged size, if convergence is outstanding.
    pub fn desired(&self) -> Option<SurfaceSize> {
        self.pending.map(|p| p.size)
    }

    /// True while geometry still needs a rebind attempt: either a fresh
    /// desire the sources have not agreed on yet (retry on the next
    /// event, the native surface may have caught up silently) or an
    /// emitted attempt that failed and was aborted for retry.
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Observe the winit resize-event size for `epoch`.
    pub fn note_winit_size(&mut self, w: u32, h: u32, epoch: u64) -> ConvergenceAction {
        let Some(size) = SurfaceSize::new(w, h) else {
            return ConvergenceAction::IgnoredInvalid;
        };
        if epoch != self.epoch {
            return ConvergenceAction::IgnoredStaleEpoch;
        }
        self.winit = Some((size, epoch));
        self.evaluate()
    }

    /// Observe the live `ANativeWindow` size for `epoch`.
    pub fn note_native_size(&mut self, w: u32, h: u32, epoch: u64) -> ConvergenceAction {
        let Some(size) = SurfaceSize::new(w, h) else {
            return ConvergenceAction::IgnoredInvalid;
        };
        if epoch != self.epoch {
            return ConvergenceAction::IgnoredStaleEpoch;
        }
        self.native = Some((size, epoch));
        self.evaluate()
    }

    /// Record that the rebind for `gen` fully succeeded (new buffers bound,
    /// ScreenInfo published, producer reconnected, first frame presented).
    /// A completion for any other generation is ignored and reported false
    /// so stale work can never mark a newer surface ready.
    pub fn confirm_converged(&mut self, gen: u64) -> bool {
        let ok = self.pending.is_some_and(|p| {
            p.emitted && self.next_gen > 2 && gen == self.next_gen - 1
        }) || self.pending.is_none() && self.converged.is_some_and(|(_, g)| g == gen);
        if !ok {
            return false;
        }
        if let Some(p) = self.pending.take() {
            self.converged = Some((p.size, gen));
        }
        true
    }

    /// Forget an emitted-but-failed attempt so the next agreeing event
    /// re-emits for the same desired size with a fresh generation. A newer
    /// size observed later still supersedes it.
    pub fn abort_pending(&mut self) {
        if let Some(p) = self.pending.as_mut() {
            p.emitted = false;
        }
    }

    fn evaluate(&mut self) -> ConvergenceAction {
        let (Some((wsize, _)), Some((nsize, _))) = (self.winit, self.native) else {
            return ConvergenceAction::Recorded;
        };
        if wsize != nsize {
            // Sources disagree mid-rotation: remember the newest desired
            // geometry (winit leads; the native surface catches up) and wait
            // for the next event instead of acting on a transient pair.
            // A newer disagreement supersedes an older pending attempt.
            if self.pending.is_none_or(|p| p.size != wsize) {
                self.pending = Some(Pending {
                    size: wsize,
                    emitted: false,
                });
            }
            return ConvergenceAction::Recorded;
        }
        if self.converged.is_some_and(|(c, _)| c == wsize) {
            self.pending = None;
            return ConvergenceAction::CoalescedIdentical;
        }
        match self.pending {
            Some(p) if p.size == wsize && p.emitted => ConvergenceAction::CoalescedIdentical,
            _ => {
                let gen = self.next_gen;
                self.next_gen += 1;
                self.pending = Some(Pending {
                    size: wsize,
                    emitted: true,
                });
                ConvergenceAction::Rebind { gen, size: wsize }
            }
        }
    }
}

impl Default for SurfaceConvergence {
    fn default() -> Self {
        Self::new()
    }
}

/// Absolute-pointer anchor tagged with the surface generation that observed
/// it. The anchor (previous physical position for relative-delta synthesis)
/// must never cross a geometry boundary: a generation change resets it so
/// the first motion after rotation reports zero delta instead of a jump.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerAnchor {
    pub x: f32,
    pub y: f32,
    pub gen: u64,
}

/// What to do with one absolute pointer observation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MotionDecision {
    /// Observed against an obsolete generation: drop without touching state.
    Drop,
    /// Forward with the synthesized relative delta.
    Send { dx: f32, dy: f32 },
}

/// Fold one absolute pointer observation into the anchor.
///
/// `event_gen` is the surface generation the event was observed against;
/// `current_gen` is the converged generation. Stale events neither send
/// nor mutate the anchor; a generation change resets the anchor with zero
/// delta (no first-motion jump); steady-state reports real dx/dy.
pub fn anchor_motion(
    prev: Option<PointerAnchor>,
    x: f32,
    y: f32,
    event_gen: u64,
    current_gen: u64,
) -> (Option<PointerAnchor>, MotionDecision) {
    if event_gen != current_gen {
        return (prev, MotionDecision::Drop);
    }
    let next = Some(PointerAnchor { x, y, gen: event_gen });
    match prev {
        Some(a) if a.gen == event_gen => (
            next,
            MotionDecision::Send {
                dx: x - a.x,
                dy: y - a.y,
            },
        ),
        _ => (next, MotionDecision::Send { dx: 0.0, dy: 0.0 }),
    }
}

/// Whether a completion (first frame, producer attach, readiness) for
/// `completed_gen` may settle the `current_gen` surface. Old-generation
/// completions must never mark a newer surface ready/converged.
pub fn generation_completion_allowed(completed_gen: u64, current_gen: u64) -> bool {
    current_gen != 0 && completed_gen == current_gen
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPOCH: u64 = 7;
    const LAND: SurfaceSize = SurfaceSize { w: 3392, h: 2400 };
    const PORT: SurfaceSize = SurfaceSize { w: 2400, h: 3392 };

    fn landed() -> SurfaceConvergence {
        let mut c = SurfaceConvergence::new();
        c.begin_epoch(EPOCH, LAND);
        c
    }

    #[test]
    fn invalid_sizes_are_ignored() {
        let mut c = landed();
        assert_eq!(
            c.note_winit_size(0, 0, EPOCH),
            ConvergenceAction::IgnoredInvalid
        );
        assert_eq!(
            c.note_native_size(2400, 0, EPOCH),
            ConvergenceAction::IgnoredInvalid
        );
        // Nothing recorded, nothing pending: a later agreeing pair for the
        // converged size coalesces instead of rebinding.
        assert_eq!(
            c.note_winit_size(LAND.w, LAND.h, EPOCH),
            ConvergenceAction::Recorded
        );
        assert_eq!(
            c.note_native_size(LAND.w, LAND.h, EPOCH),
            ConvergenceAction::CoalescedIdentical
        );
        assert!(!c.has_pending());
    }

    #[test]
    fn identical_sizes_coalesce_without_new_generation() {
        let mut c = landed();
        assert_eq!(
            c.note_winit_size(LAND.w, LAND.h, EPOCH),
            ConvergenceAction::Recorded
        );
        assert_eq!(
            c.note_native_size(LAND.w, LAND.h, EPOCH),
            ConvergenceAction::CoalescedIdentical
        );
        // Storm of repeats: still nothing.
        for _ in 0..10 {
            assert_eq!(
                c.note_winit_size(LAND.w, LAND.h, EPOCH),
                ConvergenceAction::CoalescedIdentical
            );
            assert_eq!(
                c.note_native_size(LAND.w, LAND.h, EPOCH),
                ConvergenceAction::CoalescedIdentical
            );
        }
        assert_eq!(c.converged(), Some((LAND, 1)));
        assert!(!c.has_pending());
    }

    #[test]
    fn stale_epoch_can_never_win() {
        let mut c = landed();
        // Delayed events from the previous window generation.
        assert_eq!(
            c.note_winit_size(PORT.w, PORT.h, EPOCH - 1),
            ConvergenceAction::IgnoredStaleEpoch
        );
        assert_eq!(
            c.note_native_size(PORT.w, PORT.h, EPOCH - 1),
            ConvergenceAction::IgnoredStaleEpoch
        );
        // A future epoch without a session re-base is equally untrusted.
        assert_eq!(
            c.note_winit_size(PORT.w, PORT.h, EPOCH + 1),
            ConvergenceAction::IgnoredStaleEpoch
        );
        assert!(!c.has_pending());
        assert_eq!(c.converged(), Some((LAND, 1)));
    }

    #[test]
    fn disagreement_records_newest_and_waits() {
        let mut c = landed();
        // winit leads (portrait), native surface still landscape.
        assert_eq!(
            c.note_winit_size(PORT.w, PORT.h, EPOCH),
            ConvergenceAction::Recorded
        );
        assert_eq!(c.desired(), None); // single source is not agreement
        assert_eq!(
            c.note_native_size(LAND.w, LAND.h, EPOCH),
            ConvergenceAction::Recorded
        );
        assert_eq!(c.desired(), Some(PORT)); // winit leads
        assert!(c.has_pending()); // un-emitted desire: keep retrying
        // Native catches up: exactly one rebind, generation 2.
        assert_eq!(
            c.note_native_size(PORT.w, PORT.h, EPOCH),
            ConvergenceAction::Rebind {
                gen: 2,
                size: PORT
            }
        );
        assert!(c.has_pending()); // emitted, awaiting confirmation
        assert!(c.confirm_converged(2));
        assert!(!c.has_pending());
        assert_eq!(c.converged(), Some((PORT, 2)));
    }

    #[test]
    fn landscape_to_portrait_mints_exactly_one_generation() {
        let mut c = landed();
        let a = c.note_winit_size(PORT.w, PORT.h, EPOCH);
        let b = c.note_native_size(PORT.w, PORT.h, EPOCH);
        assert_eq!(a, ConvergenceAction::Recorded);
        assert_eq!(
            b,
            ConvergenceAction::Rebind {
                gen: 2,
                size: PORT
            }
        );
        // Repeats of the agreed pair coalesce (no second generation).
        assert_eq!(
            c.note_winit_size(PORT.w, PORT.h, EPOCH),
            ConvergenceAction::CoalescedIdentical
        );
        assert_eq!(
            c.note_native_size(PORT.w, PORT.h, EPOCH),
            ConvergenceAction::CoalescedIdentical
        );
        assert!(c.confirm_converged(2));
        assert_eq!(c.converged(), Some((PORT, 2)));
        // And back: exactly one more.
        assert_eq!(
            c.note_winit_size(LAND.w, LAND.h, EPOCH),
            ConvergenceAction::Recorded
        );
        // Native still portrait: disagreement recorded, newest desired kept.
        assert_eq!(c.desired(), Some(LAND));
        assert_eq!(
            c.note_native_size(LAND.w, LAND.h, EPOCH),
            ConvergenceAction::Rebind {
                gen: 3,
                size: LAND
            }
        );
        assert!(c.confirm_converged(3));
        assert_eq!(c.converged(), Some((LAND, 3)));
    }

    #[test]
    fn resize_storm_converges_on_latest_size() {
        let mut c = landed();
        // Awkward rotation order: several winit sizes before native moves.
        // Each newer disagreement supersedes the older pending desire.
        let storm = [
            SurfaceSize { w: 3000, h: 2600 },
            SurfaceSize { w: 2600, h: 3000 },
            PORT,
        ];
        for s in storm {
            assert_eq!(
                c.note_winit_size(s.w, s.h, EPOCH),
                ConvergenceAction::Recorded
            );
            // Native lags one step behind: still disagreement, no rebind.
            assert_eq!(
                c.note_native_size(LAND.w, LAND.h, EPOCH),
                ConvergenceAction::Recorded
            );
        }
        assert_eq!(c.desired(), Some(PORT));
        // Native lands on the latest: a single rebind for PORT, never for
        // the obsolete intermediates.
        assert_eq!(
            c.note_native_size(PORT.w, PORT.h, EPOCH),
            ConvergenceAction::Rebind {
                gen: 2,
                size: PORT
            }
        );
        assert!(c.confirm_converged(2));
        assert_eq!(c.converged(), Some((PORT, 2)));
    }

    #[test]
    fn failed_attempt_re_emits_with_fresh_generation() {
        let mut c = landed();
        c.note_winit_size(PORT.w, PORT.h, EPOCH);
        assert_eq!(
            c.note_native_size(PORT.w, PORT.h, EPOCH),
            ConvergenceAction::Rebind {
                gen: 2,
                size: PORT
            }
        );
        // Rebind failed (e.g. slot collection): allow exactly one retry.
        c.abort_pending();
        assert!(c.has_pending());
        assert_eq!(
            c.note_native_size(PORT.w, PORT.h, EPOCH),
            ConvergenceAction::Rebind {
                gen: 3,
                size: PORT
            }
        );
        assert!(c.confirm_converged(3));
        assert_eq!(c.converged(), Some((PORT, 3)));
    }

    #[test]
    fn stale_completion_cannot_mark_new_surface() {
        // Old-generation first-frame/attach completions are rejected once
        // the surface moved on; only the current generation settles it.
        assert!(!generation_completion_allowed(2, 3));
        assert!(!generation_completion_allowed(0, 0));
        assert!(!generation_completion_allowed(1, 0));
        assert!(generation_completion_allowed(3, 3));
        assert!(generation_completion_allowed(1, 1));
    }

    #[test]
    fn confirm_rejects_wrong_generation() {
        let mut c = landed();
        c.note_winit_size(PORT.w, PORT.h, EPOCH);
        assert_eq!(
            c.note_native_size(PORT.w, PORT.h, EPOCH),
            ConvergenceAction::Rebind {
                gen: 2,
                size: PORT
            }
        );
        assert!(!c.confirm_converged(1)); // previous surface
        assert!(!c.confirm_converged(3)); // never emitted
        assert_eq!(c.converged(), Some((LAND, 1))); // unchanged
        assert!(c.confirm_converged(2));
        assert_eq!(c.converged(), Some((PORT, 2)));
    }

    #[test]
    fn anchor_resets_across_geometry_boundary() {
        // Steady-state motion reports real deltas.
        let (a1, d1) = anchor_motion(None, 100.0, 200.0, 2, 2);
        assert_eq!(d1, MotionDecision::Send { dx: 0.0, dy: 0.0 });
        let (a2, d2) = anchor_motion(a1, 110.0, 205.0, 2, 2);
        assert_eq!(
            d2,
            MotionDecision::Send {
                dx: 10.0,
                dy: 5.0
            }
        );
        // Rotation converged to gen 3: the old anchor must not produce a
        // cross-boundary jump; first motion re-anchors with zero delta.
        let (a3, d3) = anchor_motion(a2, 2000.0, 100.0, 3, 3);
        assert_eq!(d3, MotionDecision::Send { dx: 0.0, dy: 0.0 });
        assert_eq!(
            a3,
            Some(PointerAnchor {
                x: 2000.0,
                y: 100.0,
                gen: 3
            })
        );
        // Stale events neither send nor mutate the new anchor.
        let (a4, d4) = anchor_motion(a3, 111.0, 206.0, 2, 3);
        assert_eq!(d4, MotionDecision::Drop);
        assert_eq!(a4, a3);
    }

    #[test]
    fn begin_epoch_rebases_cleanly() {
        let mut c = landed();
        c.note_winit_size(PORT.w, PORT.h, EPOCH);
        c.note_native_size(PORT.w, PORT.h, EPOCH);
        // New session on a new window: old observations are gone, the
        // start size converges as generation 1, rotations mint from 2.
        let next = EPOCH + 1;
        c.begin_epoch(next, PORT);
        assert_eq!(c.converged(), Some((PORT, 1)));
        assert!(!c.has_pending());
        assert_eq!(
            c.note_winit_size(PORT.w, PORT.h, next),
            ConvergenceAction::Recorded
        );
        assert_eq!(
            c.note_native_size(PORT.w, PORT.h, next),
            ConvergenceAction::CoalescedIdentical
        );
        assert_eq!(
            c.note_native_size(LAND.w, LAND.h, next),
            ConvergenceAction::Recorded
        );
        // winit still portrait, native landscape: newest desired is winit's.
        assert_eq!(c.desired(), Some(PORT));
    }
}
