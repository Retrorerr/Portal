//! Authoritative SHM damage pipeline (Performance Pass 2).
//!
//! All texture uploads are expressed in **physical SHM buffer pixels**
//! (top-left origin, extent = actual `wl_shm` buffer dimensions).
//!
//! This mirrors `localdesktop::core::shm_damage` (host-testable single model)
//! using Smithay geometry types so the GLES upload path and the
//! `RendererSurfaceState` conversion share one coordinate model.
//!
//! Previous root cause: partial `glTexSubImage2D` uploaded `damage` rects with
//! integer `wl_surface.set_buffer_scale` scaling only. Under fractional
//! scaling (Plasma ~2.25x via `wp_fractional_scale` + `wp_viewporter`) the
//! integer scale under-covers surface-logical damage, and integer protocol
//! truncation of fractional damage was never recovered. Only the top-left
//! subset of the true dirty region was refreshed, leaving trails/ghosting.
//!
//! Reintroduced-ghosting fix: rect math alone was still insufficient, because
//! a cached texture's true contents depend on *which committed buffer state
//! it was last synced to* plus *every subsequent mutation* — not just the
//! latest damage rectangle (same-`wl_buffer` reuse with new contents, buffer
//! replacement/rotation, damage-history eviction, failed GL uploads, and
//! viewport/scale/transform reinterpretation can all strand the texture
//! behind an apparently valid damage chain). `ShmSyncTracker` therefore gates
//! every import on an explicit monotonic content generation: partial upload
//! only with a complete, ordered damage record for every generation since the
//! last successful upload, on the same buffer object/geometry/interpretation;
//! anything uncertain forces exactly one full resync.
//!
//! The `+1 logical pixel` expansion is kept deliberately as protocol-rounding
//! coverage (clients report integer damage and may inward-round true
//! fractional dirty regions by just under 1px per side). Outward expansion
//! only copies current (correct) pixels, so it cannot cause staleness.

use crate::utils::{Buffer as BufferCoord, Logical, Point, Rectangle, Size, Transform};

/// Why a frame must fall back to a full upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShmDamageFallback {
    InvalidGeometry,
    InvalidScale,
    InvalidViewport,
}

/// Per-frame upload diagnostics for profiling (debug-level logging only).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShmUploadStats {
    pub damaged_pixels: i64,
    pub total_pixels: i64,
    pub percent: f64,
    pub num_rects: usize,
    pub bytes_uploaded: i64,
    pub is_full_fallback: bool,
}

impl ShmUploadStats {
    pub fn for_rects(
        rects: &[Rectangle<i32, BufferCoord>],
        buffer: Size<i32, BufferCoord>,
        bytes_per_pixel: i64,
        full: bool,
    ) -> Self {
        let total = buffer.w as i64 * buffer.h as i64;
        let damaged: i64 = rects
            .iter()
            .map(|r| {
                if r.size.w <= 0 || r.size.h <= 0 {
                    0
                } else {
                    r.size.w as i64 * r.size.h as i64
                }
            })
            .sum();
        let percent = if total > 0 {
            damaged as f64 * 100.0 / total as f64
        } else {
            0.0
        };
        Self {
            damaged_pixels: damaged,
            total_pixels: total,
            percent,
            num_rects: rects.len(),
            bytes_uploaded: damaged * bytes_per_pixel.max(1),
            is_full_fallback: full,
        }
    }
}

/// Convert one `wl_surface.damage` (surface-logical integer) rect to buffer
/// pixels.
///
/// `surface_size` is the buffer-logical extent (`buffer/scale`, transform
/// applied) i.e. the `src` space extent. `view_src`/`view_dst` describe the
/// viewporter mapping when KWin sets one (`None` = damage already in
/// `surface_size` space).
///
/// Returns `Ok(None)` for empty/fully-clipped rects (skip, keep other rects),
/// `Err(_)` for ambiguous state (caller must full-upload the frame).
#[allow(clippy::too_many_arguments)]
pub fn surface_to_buffer(
    rect: Rectangle<i32, Logical>,
    buffer: Size<i32, BufferCoord>,
    surface_size: Size<i32, Logical>,
    view_src: Option<Rectangle<f64, Logical>>,
    view_dst: Option<Size<i32, Logical>>,
    buffer_scale: i32,
    transform: Transform,
) -> Result<Option<Rectangle<i32, BufferCoord>>, ShmDamageFallback> {
    if rect.size.w <= 0 || rect.size.h <= 0 {
        return Ok(None);
    }
    if buffer.w <= 0 || buffer.h <= 0 {
        return Err(ShmDamageFallback::InvalidGeometry);
    }
    if !(1..=8).contains(&buffer_scale) {
        return Err(ShmDamageFallback::InvalidScale);
    }
    if surface_size.w <= 0 || surface_size.h <= 0 {
        return Err(ShmDamageFallback::InvalidGeometry);
    }

    // Conservative recovery of protocol integer truncation: true fractional
    // client damage may extend ~1 logical pixel beyond the reported integer
    // rect on every side. Expansion stays proportionally tiny.
    let mut fx = rect.loc.x as f64 - 1.0;
    let mut fy = rect.loc.y as f64 - 1.0;
    let mut fw = rect.size.w as f64 + 2.0;
    let mut fh = rect.size.h as f64 + 2.0;

    if let (Some(src), Some(dst)) = (view_src, view_dst) {
        if dst.w <= 0 || dst.h <= 0 || src.size.w <= 0.0 || src.size.h <= 0.0 {
            return Err(ShmDamageFallback::InvalidViewport);
        }
        if ![src.loc.x, src.loc.y, src.size.w, src.size.h]
            .iter()
            .all(|v| v.is_finite())
        {
            return Err(ShmDamageFallback::InvalidViewport);
        }
        let surf_w = surface_size.w as f64;
        let surf_h = surface_size.h as f64;
        // src must lie inside the surface (tiny float slack).
        if src.loc.x < -1.0
            || src.loc.y < -1.0
            || src.loc.x + src.size.w > surf_w + 1.0
            || src.loc.y + src.size.h > surf_h + 1.0
        {
            return Err(ShmDamageFallback::InvalidViewport);
        }
        let scale_x = src.size.w / dst.w as f64;
        let scale_y = src.size.h / dst.h as f64;
        if !(scale_x > 0.0 && scale_y > 0.0 && scale_x.is_finite() && scale_y.is_finite()) {
            return Err(ShmDamageFallback::InvalidViewport);
        }
        fx = src.loc.x + fx * scale_x;
        fy = src.loc.y + fy * scale_y;
        fw *= scale_x;
        fh *= scale_y;
    }
    if !(fw > 0.0 && fh > 0.0 && fx.is_finite() && fy.is_finite() && fw.is_finite() && fh.is_finite()) {
        return Ok(None);
    }

    // Transform in surface_size (src) space, then exact integer buffer scale.
    // A single final floor/ceil in buffer pixels preserves coverage.
    let src_rect_f64: Rectangle<f64, Logical> = Rectangle::new((fx, fy).into(), (fw, fh).into());
    let area = Size::from((surface_size.w as f64, surface_size.h as f64));
    let transformed = transform.transform_rect_in(src_rect_f64, &area);
    if ![
        transformed.loc.x,
        transformed.loc.y,
        transformed.size.w,
        transformed.size.h,
    ]
    .iter()
    .all(|v| v.is_finite())
        || transformed.size.w <= 0.0
        || transformed.size.h <= 0.0
    {
        return Err(ShmDamageFallback::InvalidGeometry);
    }
    let bs = buffer_scale as f64;
    let bx0 = (transformed.loc.x * bs).floor();
    let by0 = (transformed.loc.y * bs).floor();
    let bx1 = ((transformed.loc.x + transformed.size.w) * bs).ceil();
    let by1 = ((transformed.loc.y + transformed.size.h) * bs).ceil();
    if ![bx0, by0, bx1, by1].iter().all(|v| v.is_finite()) {
        return Err(ShmDamageFallback::InvalidGeometry);
    }
    let topleft = Point::<i32, BufferCoord>::from((bx0 as i32, by0 as i32));
    let bottomright = Point::<i32, BufferCoord>::from((bx1 as i32, by1 as i32));
    let rect_i32: Rectangle<i32, BufferCoord> =
        Rectangle::from_extremities(topleft, bottomright);
    if rect_i32.size.w <= 0 || rect_i32.size.h <= 0 {
        return Ok(None);
    }
    // Clamp to actual buffer bounds; fully-outside contributes nothing.
    Ok(rect_i32.intersection(Rectangle::from_size(buffer)))
}

/// Convert one `wl_surface.damage_buffer` (already buffer pixels) rect.
/// Pure clamp/validate: no scaling, no transform.
pub fn buffer_to_buffer(
    rect: Rectangle<i32, BufferCoord>,
    buffer: Size<i32, BufferCoord>,
) -> Result<Option<Rectangle<i32, BufferCoord>>, ShmDamageFallback> {
    if rect.size.w <= 0 || rect.size.h <= 0 {
        return Ok(None);
    }
    if buffer.w <= 0 || buffer.h <= 0 {
        return Err(ShmDamageFallback::InvalidGeometry);
    }
    Ok(rect.intersection(Rectangle::from_size(buffer)))
}

/// Merge overlapping/touching rects to bound `glTexSubImage2D` calls.
/// Distant tiny damages are never coalesced into near-full uploads.
pub fn merge_rects(mut rects: Vec<Rectangle<i32, BufferCoord>>) -> Vec<Rectangle<i32, BufferCoord>> {
    rects.retain(|r| !r.is_empty());
    let mut out: Vec<Rectangle<i32, BufferCoord>> = Vec::with_capacity(rects.len());
    for mut rect in rects {
        loop {
            let mut hit: Option<usize> = None;
            for (i, other) in out.iter().enumerate() {
                if other.overlaps_or_touches(rect) {
                    rect = other.merge(rect);
                    hit = Some(i);
                    break;
                }
            }
            if let Some(i) = hit {
                out.remove(i);
            } else {
                break;
            }
        }
        out.push(rect);
    }
    out.retain(|r| !r.is_empty());
    out
}

/// Fingerprint of everything that determines how committed damage must be
/// interpreted and what a cached GLES texture was last proven to contain.
///
/// Plain data mirroring `localdesktop::core::shm_damage::SyncFingerprint`
/// field-for-field; the wiring builds it from live surface state on every
/// commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncFingerprint {
    /// Physical SHM buffer dimensions.
    pub buffer_w: i32,
    pub buffer_h: i32,
    /// Integer `wl_surface.set_buffer_scale`.
    pub buffer_scale: i32,
    /// `wl_surface.set_buffer_transform` discriminant (wiring-defined mapping).
    pub transform: u8,
    /// Viewporter src `(x, y, w, h)` as `f64` bits.
    pub view_src: [u64; 4],
    /// Viewporter dst `(w, h)`.
    pub view_dst: [i32; 2],
}

impl SyncFingerprint {
    /// Classify drift between the last proven state and the current one.
    pub fn drift_reason(&self, current: &Self) -> &'static str {
        if self.buffer_w != current.buffer_w || self.buffer_h != current.buffer_h {
            "resize"
        } else {
            "view-change"
        }
    }
}

/// One surface commit, as observed by the sync tracker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitSyncEvent {
    /// A (possibly re-attached) SHM buffer was committed, so the content
    /// generation advances and fresh damage is required to cover it.
    pub new_content: bool,
    /// The committed wl_buffer object differs from the one the cached texture
    /// was synced to. A replaced object must fully resync: object identity is
    /// not content identity, and a fresh/rotated buffer has no proven pixel
    /// history.
    pub new_object: bool,
    pub fingerprint: SyncFingerprint,
    /// This generation's damage was completely recorded (converted,
    /// non-empty, unambiguous). `false` forces a full resync.
    pub damage_complete: bool,
    /// Why `damage_complete` is false (forwarded as the resync reason).
    pub damage_reason: Option<&'static str>,
}

/// Upload gate decision for one import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDecision {
    /// Perform one full upload, then resume gated partials.
    Full { reason: &'static str },
    /// The proven chain covers every generation since the last upload:
    /// partial upload of the accumulated damage is cache-correct.
    Partial,
    /// Texture already holds the latest committed generation: skip upload.
    UpToDate,
}

/// Explicit content-generation gate for partial SHM texture uploads.
///
/// Invariant: a cached GLES texture may receive a partial update only when
/// the compositor knows exactly which committed SHM-buffer generation that
/// texture represents (`uploaded`) and holds complete, ordered damage for
/// every subsequent generation up to `committed`, with no identity/geometry/
/// interpretation drift in between. Anything else forces exactly one full
/// upload, after which the generation resynchronizes and partials may resume.
///
/// Damage-rect transport stays in `DamageBag`/`renderer_seen`; this tracker
/// is purely the proof gate and can only ever widen full-uploads, never
/// narrow them. Only a successful GL upload may advance the proven
/// generation (`mark_uploaded(true)`); failures leave all state untouched so
/// the next import retries a superset.
#[derive(Debug, Clone)]
pub struct ShmSyncTracker {
    /// Latest committed content generation (`0` = none yet).
    committed: u64,
    /// Generation the GL texture provably contains (`None` = unknown).
    uploaded: Option<u64>,
    /// Every generation in `(uploaded, committed]` has usable damage, with no
    /// identity/geometry drift since the last successful upload.
    complete: bool,
    gap_reason: Option<&'static str>,
    uploaded_fp: Option<SyncFingerprint>,
    current_fp: Option<SyncFingerprint>,
    object_changed: bool,
}

impl ShmSyncTracker {
    pub fn new() -> Self {
        Self {
            committed: 0,
            uploaded: None,
            complete: true,
            gap_reason: None,
            uploaded_fp: None,
            current_fp: None,
            object_changed: false,
        }
    }

    /// Surface torn down / buffer removed: nothing is proven anymore.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Record one commit. State-only commits (no new SHM content) never need
    /// damage, but their fingerprint still participates in drift detection.
    pub fn note_commit(&mut self, ev: CommitSyncEvent) {
        if ev.new_content {
            self.committed += 1;
            if ev.new_object && self.uploaded.is_some() {
                self.object_changed = true;
            }
            if !ev.damage_complete {
                self.complete = false;
                self.gap_reason = ev.damage_reason;
            }
        }
        // Drift is measured against the last PROVEN state, not the previous
        // commit: mid-chain excursions that return to the proven fingerprint
        // before any upload stay uploadable; anything still drifted at import
        // time forces a resync.
        if let Some(proven) = self.uploaded_fp {
            if proven != ev.fingerprint {
                self.complete = false;
                if self.gap_reason.is_none() {
                    self.gap_reason = Some(proven.drift_reason(&ev.fingerprint));
                }
            }
        }
        self.current_fp = Some(ev.fingerprint);
    }

    /// Damage history is unavailable for some covered range (eviction,
    /// reset-during-flight, unknown buffer): completeness can no longer be
    /// demonstrated, so the next import must fully resync.
    pub fn note_history_gap(&mut self, reason: &'static str) {
        self.complete = false;
        if self.gap_reason.is_none() {
            self.gap_reason = Some(reason);
        }
    }

    pub fn decide(&self) -> SyncDecision {
        let Some(uploaded) = self.uploaded else {
            return SyncDecision::Full {
                reason: "unknown-generation",
            };
        };
        if self.object_changed {
            return SyncDecision::Full {
                reason: "buffer-replaced",
            };
        }
        if !self.complete {
            return SyncDecision::Full {
                reason: self.gap_reason.unwrap_or("history-gap"),
            };
        }
        if let (Some(proven), Some(current)) = (self.uploaded_fp, self.current_fp) {
            if proven != current {
                return SyncDecision::Full {
                    reason: proven.drift_reason(&current),
                };
            }
        }
        if uploaded == self.committed {
            SyncDecision::UpToDate
        } else {
            SyncDecision::Partial
        }
    }

    /// Record the outcome of the upload the last `decide()` authorized.
    /// Only success advances the proven generation; failure leaves every bit
    /// of state untouched so the next import retries a superset.
    pub fn mark_uploaded(&mut self, success: bool) {
        if !success {
            return;
        }
        self.uploaded = Some(self.committed);
        self.uploaded_fp = self.current_fp;
        self.complete = true;
        self.gap_reason = None;
        self.object_changed = false;
    }

    pub fn committed(&self) -> u64 {
        self.committed
    }

    pub fn uploaded(&self) -> Option<u64> {
        self.uploaded
    }
}

impl Default for ShmSyncTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Decide between partial uploads and the safe full fallback.
/// See Portal `core::shm_damage::decide_upload` for the full contract.
pub fn decide_upload(
    validated: Vec<Rectangle<i32, BufferCoord>>,
    buffer: Size<i32, BufferCoord>,
    is_new_texture: bool,
    had_fallback_signal: bool,
    bytes_per_pixel: i64,
) -> (
    Vec<Rectangle<i32, BufferCoord>>,
    bool,
    &'static str,
    ShmUploadStats,
) {
    let total = buffer.w as i64 * buffer.h as i64;
    let full_rect = Rectangle::from_size(buffer);
    if is_new_texture {
        let stats = ShmUploadStats::for_rects(&[full_rect], buffer, bytes_per_pixel, true);
        return (vec![full_rect], true, "new-texture", stats);
    }
    if had_fallback_signal {
        let stats = ShmUploadStats::for_rects(&[full_rect], buffer, bytes_per_pixel, true);
        return (vec![full_rect], true, "ambiguous-damage", stats);
    }
    let merged = merge_rects(validated);
    if merged.is_empty() {
        let stats = ShmUploadStats::for_rects(&[full_rect], buffer, bytes_per_pixel, true);
        return (vec![full_rect], true, "missing-damage", stats);
    }
    let damaged: i64 = merged.iter().map(|r| r.size.w as i64 * r.size.h as i64).sum();
    if total > 0 && damaged * 100 >= total * 70 {
        let stats = ShmUploadStats::for_rects(&[full_rect], buffer, bytes_per_pixel, true);
        return (vec![full_rect], true, "near-full-damage", stats);
    }
    let stats = ShmUploadStats::for_rects(&merged, buffer, bytes_per_pixel, false);
    (merged, false, "partial", stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_surface_damage_stays_tiny() {
        let buffer: Size<i32, BufferCoord> = (3392, 2400).into();
        let surface_size: Size<i32, Logical> = (3392, 2400).into();
        let src: Rectangle<f64, Logical> = Rectangle::new((0.0, 0.0).into(), (3392.0, 2400.0).into());
        let dst: Size<i32, Logical> = (1507, 1066).into();
        let r = surface_to_buffer(
            Rectangle::new((100, 100).into(), (10, 10).into()),
            buffer,
            surface_size,
            Some(src),
            Some(dst),
            1,
            Transform::Normal,
        )
        .unwrap()
        .unwrap();
        assert!(r.size.w >= 26 && r.size.w <= 30, "{r:?}");
        let (rects, full, _, stats) = decide_upload(vec![r], buffer, false, false, 4);
        assert!(!full);
        assert_eq!(rects.len(), 1);
        assert!(stats.percent < 0.1);
    }

    #[test]
    fn buffer_damage_clamps() {
        let buffer: Size<i32, BufferCoord> = (800, 600).into();
        let r = buffer_to_buffer(Rectangle::new((-50, -20).into(), (100, 100).into()), buffer)
            .unwrap()
            .unwrap();
        assert_eq!(r.loc.x, 0);
        assert_eq!(r.loc.y, 0);
        assert_eq!((r.size.w, r.size.h), (50, 80));
        assert!(
            buffer_to_buffer(Rectangle::new((900, 700).into(), (50, 50).into()), buffer)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn distant_tiny_never_coalesce() {
        let a: Rectangle<i32, BufferCoord> = Rectangle::new((0, 0).into(), (10, 10).into());
        let b: Rectangle<i32, BufferCoord> = Rectangle::new((3000, 2000).into(), (10, 10).into());
        assert_eq!(merge_rects(vec![a, b]).len(), 2);
        let c: Rectangle<i32, BufferCoord> = Rectangle::new((0, 0).into(), (100, 100).into());
        let d: Rectangle<i32, BufferCoord> = Rectangle::new((50, 50).into(), (100, 100).into());
        assert_eq!(merge_rects(vec![c, d]).len(), 1);
    }

    #[test]
    fn invalid_viewport_falls_back() {
        let buffer: Size<i32, BufferCoord> = (800, 600).into();
        let surface_size: Size<i32, Logical> = (800, 600).into();
        let bad_src: Rectangle<f64, Logical> = Rectangle::new((0.0, 0.0).into(), (-10.0, 100.0).into());
        assert!(surface_to_buffer(
            Rectangle::new((0, 0).into(), (10, 10).into()),
            buffer,
            surface_size,
            Some(bad_src),
            Some((100, 100).into()),
            1,
            Transform::Normal,
        )
        .is_err());
    }

    // ---- ShmSyncTracker: explicit content-generation gate ----

    fn sync_fp(w: i32, h: i32) -> SyncFingerprint {
        SyncFingerprint {
            buffer_w: w,
            buffer_h: h,
            buffer_scale: 1,
            transform: 0,
            view_src: [
                0.0f64.to_bits(),
                0.0f64.to_bits(),
                (w as f64).to_bits(),
                (h as f64).to_bits(),
            ],
            view_dst: [w, h],
        }
    }

    fn sync_content(fp_: SyncFingerprint, new_object: bool) -> CommitSyncEvent {
        CommitSyncEvent {
            new_content: true,
            new_object,
            fingerprint: fp_,
            damage_complete: true,
            damage_reason: None,
        }
    }

    #[test]
    fn sync_gate_full_then_partial_then_up_to_date() {
        let mut t = ShmSyncTracker::new();
        assert_eq!(
            t.decide(),
            SyncDecision::Full {
                reason: "unknown-generation"
            }
        );
        t.note_commit(sync_content(sync_fp(800, 600), true));
        t.mark_uploaded(true);
        assert_eq!(t.decide(), SyncDecision::UpToDate);
        t.note_commit(sync_content(sync_fp(800, 600), false));
        assert_eq!(t.decide(), SyncDecision::Partial);
        t.mark_uploaded(true);
        assert_eq!(t.decide(), SyncDecision::UpToDate);
    }

    #[test]
    fn sync_gate_object_replacement_and_gap_force_full() {
        let mut t = ShmSyncTracker::new();
        t.note_commit(sync_content(sync_fp(800, 600), true));
        t.mark_uploaded(true);
        // Rotated buffer object: no proven pixel history.
        t.note_commit(sync_content(sync_fp(800, 600), true));
        assert_eq!(
            t.decide(),
            SyncDecision::Full {
                reason: "buffer-replaced"
            }
        );
        t.mark_uploaded(true);
        // Evicted history: completeness unprovable.
        t.note_commit(sync_content(sync_fp(800, 600), false));
        t.note_history_gap("history-gap");
        assert_eq!(
            t.decide(),
            SyncDecision::Full {
                reason: "history-gap"
            }
        );
        t.mark_uploaded(true);
        assert_eq!(t.decide(), SyncDecision::UpToDate);
    }

    #[test]
    fn sync_gate_failed_upload_never_advances() {
        let mut t = ShmSyncTracker::new();
        t.note_commit(sync_content(sync_fp(800, 600), true));
        t.mark_uploaded(false);
        assert_eq!(t.uploaded(), None);
        t.mark_uploaded(true);
        t.note_commit(sync_content(sync_fp(800, 600), false));
        assert_eq!(t.decide(), SyncDecision::Partial);
        // Failed partial keeps the proven generation behind: retry superset.
        t.mark_uploaded(false);
        assert_eq!(t.uploaded(), Some(1));
        assert_eq!(t.decide(), SyncDecision::Partial);
    }

    #[test]
    fn sync_gate_resize_and_view_drift_force_full() {
        let mut t = ShmSyncTracker::new();
        t.note_commit(sync_content(sync_fp(800, 600), true));
        t.mark_uploaded(true);
        t.note_commit(sync_content(sync_fp(1024, 768), false));
        assert_eq!(t.decide(), SyncDecision::Full { reason: "resize" });
        t.mark_uploaded(true);
        let mut drifted = sync_fp(1024, 768);
        drifted.view_dst = [512, 384];
        t.note_commit(sync_content(drifted, false));
        assert_eq!(
            t.decide(),
            SyncDecision::Full {
                reason: "view-change"
            }
        );
        t.mark_uploaded(true);
        assert_eq!(t.decide(), SyncDecision::UpToDate);
    }

    #[test]
    fn sync_gate_incomplete_damage_forces_full_with_reason() {
        let mut t = ShmSyncTracker::new();
        t.note_commit(sync_content(sync_fp(800, 600), true));
        t.mark_uploaded(true);
        t.note_commit(CommitSyncEvent {
            new_content: true,
            new_object: false,
            fingerprint: sync_fp(800, 600),
            damage_complete: false,
            damage_reason: Some("ambiguous-damage"),
        });
        assert_eq!(
            t.decide(),
            SyncDecision::Full {
                reason: "ambiguous-damage"
            }
        );
        t.mark_uploaded(true);
        t.note_commit(CommitSyncEvent {
            new_content: true,
            new_object: false,
            fingerprint: sync_fp(800, 600),
            damage_complete: false,
            damage_reason: Some("missing-damage"),
        });
        assert_eq!(
            t.decide(),
            SyncDecision::Full {
                reason: "missing-damage"
            }
        );
    }
}
