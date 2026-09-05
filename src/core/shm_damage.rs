//! Authoritative SHM damage pipeline for Performance Pass 2.
//!
//! All texture uploads are expressed in **physical SHM buffer pixels**
//! (top-left origin, extent = actual `wl_shm` buffer dimensions).
//! This module is intentionally platform-independent (no Smithay/Android
//! dependencies) so the coordinate model can be exhaustively tested on host.
//!
//! Historical root cause (why the old partial uploads ghosted):
//! the previous `import_shm_buffer` fast path uploaded `damage` rectangles
//! directly with integer `wl_surface.set_buffer_scale` scaling only. Under
//! fractional scaling (e.g. Plasma 2.25x via `wp_fractional_scale` +
//! `wp_viewporter`) the integer scale (typically 1) under-scales surface-logical
//! damage (e.g. 10 logical px should cover ~23 buffer px at 2.25x but only
//! covered 10), and integer protocol truncation of fractional damage was never
//! conservatively recovered. Partial `glTexSubImage2D` then updated only the
//! top-left subset of the true dirty region, leaving stale pixels that
//! accumulated as window-drag trails/ghosting. The temporary fix uploaded the
//! full buffer every time.
//!
//! Authoritative model implemented here (mirrored in the Smithay patch):
//! - Surface-logical damage (`wl_surface.damage`, integer Logical) is mapped
//!   `dst -> src` through the viewporter (`src.size/dst.size` + `src.loc`),
//!   transformed in `surface_size` space, scaled by the integer buffer scale,
//!   then floored/ceiled **in buffer pixels** (single final expansion).
//! - Before viewport mapping the integer logical rect is conservatively
//!   expanded by 1 logical pixel on every side to cover `wl_surface.damage`
//!   integer truncation of true fractional client damage. This keeps tiny
//!   changes tiny (e.g. 10px -> 12px logical -> ~27px buffer at 2.25x) while
//!   guaranteeing coverage.
//! - Buffer damage (`wl_surface.damage_buffer`, already buffer pixels) is used
//!   verbatim apart from clamping.
//! - Every rect is clamped to `(0,0,buffer_w,buffer_h)`; empty results are
//!   skipped (not fallback). Any ambiguous state (invalid geometry, invalid
//!   scale, invalid viewport, non-finite floats) returns `Err` and the caller
//!   must fall back to a full upload. Correctness always beats optimisation.
//! - Accumulation is generation-tracked: damage is unioned across all commits
//!   since the last successful texture upload; resize/new-buffer forces full.
//!
//! Ghosting fix (explicit content-generation gate, `ShmSyncTracker`): damage
//! rects alone cannot prove a partial upload is safe, because the cached GLES
//! texture's true contents are a function of *which committed buffer state it
//! was last synced to* and *every subsequent mutation* — not just the latest
//! damage rectangle. In particular, same-`wl_buffer` reuse with new contents,
//! buffer replacement/rotation, damage-history eviction, failed GL uploads,
//! and viewport/scale/transform reinterpretation can all leave the texture
//! behind the damage chain while every individual rect looks valid. The gate
//! therefore tracks an explicit monotonic content generation per commit and
//! the exact generation the texture provably contains; partial upload is
//! allowed only with a complete, ordered damage record for every generation
//! in between, on the same buffer object, geometry, format interpretation,
//! and view. Anything uncertain forces exactly one full upload, after which
//! the generation resynchronizes and partials may resume.
//!
//! On the `+1 logical pixel` conservative expansion (kept deliberately): it
//! is not bug-hiding padding but protocol-rounding coverage. Clients report
//! `wl_surface.damage` as integers; a client that inward-rounds its true
//! fractional dirty region can under-report by just under 1 logical pixel per
//! side, so the expansion guarantees coverage regardless of client rounding
//! direction. Outward expansion can only ever copy current (correct) pixels,
//! so it cannot cause staleness — only at most a few extra buffer pixels.

/// Integer SHM buffer dimensions in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferSize {
    pub w: i32,
    pub h: i32,
}

/// Integer buffer-pixel rectangle, top-left origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl BufferRect {
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn area(&self) -> i64 {
        if self.is_empty() {
            0
        } else {
            self.w as i64 * self.h as i64
        }
    }

    /// Intersection with `(0,0,bw,bh)`. `None` when empty/outside.
    pub fn clamp_to(&self, buffer: BufferSize) -> Option<BufferRect> {
        if self.w <= 0 || self.h <= 0 || buffer.w <= 0 || buffer.h <= 0 {
            return None;
        }
        let x0 = self.x.max(0);
        let y0 = self.y.max(0);
        let x1 = (self.x as i64 + self.w as i64).min(buffer.w as i64);
        let y1 = (self.y as i64 + self.h as i64).min(buffer.h as i64);
        if x1 <= x0 as i64 || y1 <= y0 as i64 {
            return None;
        }
        Some(BufferRect {
            x: x0,
            y: y0,
            w: (x1 - x0 as i64) as i32,
            h: (y1 - y0 as i64) as i32,
        })
    }

    pub fn overlaps_or_touches(&self, other: BufferRect) -> bool {
        if self.is_empty() || other.is_empty() {
            return false;
        }
        self.x <= other.x + other.w
            && other.x <= self.x + self.w
            && self.y <= other.y + other.h
            && other.y <= self.y + self.h
    }

    pub fn merge(&self, other: BufferRect) -> BufferRect {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x as i64 + self.w as i64).max(other.x as i64 + other.w as i64);
        let y1 = (self.y as i64 + self.h as i64).max(other.y as i64 + other.h as i64);
        BufferRect {
            x: x0,
            y: y0,
            w: (x1 - x0 as i64) as i32,
            h: (y1 - y0 as i64) as i32,
        }
    }
}

/// Buffer transform, mirroring Wayland/Smithay semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferTransform {
    Normal,
    R90,
    R180,
    R270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

fn transform_rect_f64(
    rect: (f64, f64, f64, f64),
    area: (f64, f64),
    transform: BufferTransform,
) -> Option<(f64, f64, f64, f64)> {
    let (x, y, w, h) = rect;
    let (aw, ah) = area;
    if !(w > 0.0 && h > 0.0 && aw > 0.0 && ah > 0.0) {
        return None;
    }
    if ![x, y, w, h, aw, ah].iter().all(|v| v.is_finite()) {
        return None;
    }
    let (nx, ny, nw, nh) = match transform {
        BufferTransform::Normal => (x, y, w, h),
        BufferTransform::R90 => (ah - y - h, x, h, w),
        BufferTransform::R180 => (aw - x - w, ah - y - h, w, h),
        BufferTransform::R270 => (y, aw - x - w, h, w),
        BufferTransform::Flipped => (aw - x - w, y, w, h),
        BufferTransform::Flipped90 => (ah - y - h, aw - x - w, h, w),
        BufferTransform::Flipped180 => (x, ah - y - h, w, h),
        BufferTransform::Flipped270 => (y, x, h, w),
    };
    if ![nx, ny, nw, nh].iter().all(|v| v.is_finite()) {
        return None;
    }
    Some((nx, ny, nw, nh))
}

/// Viewporter state. `dst` is the surface-local logical size damage is
/// expressed in; `src` is the corresponding floating source in
/// `surface_size` (buffer-logical) space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportState {
    pub src_x: f64,
    pub src_y: f64,
    pub src_w: f64,
    pub src_h: f64,
    pub dst_w: i32,
    pub dst_h: i32,
}

impl ViewportState {
    fn validate(&self, surface_w: f64, surface_h: f64) -> bool {
        if !(self.src_w > 0.0
            && self.src_h > 0.0
            && surface_w > 0.0
            && surface_h > 0.0
            && self.dst_w > 0
            && self.dst_h > 0)
        {
            return false;
        }
        if ![self.src_x, self.src_y, self.src_w, self.src_h]
            .iter()
            .all(|v| v.is_finite())
        {
            return false;
        }
        // src must lie inside the surface (allow tiny float slack).
        if self.src_x < -1.0
            || self.src_y < -1.0
            || self.src_x + self.src_w > surface_w + 1.0
            || self.src_y + self.src_h > surface_h + 1.0
        {
            return false;
        }
        true
    }
}

/// Why a conversion requires a full upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageFallback {
    InvalidGeometry,
    InvalidScale,
    InvalidViewport,
}

/// Convert one `wl_surface.damage` (surface-logical integer) rect to buffer
/// pixels.
///
/// - `surface`: integer damage `(x,y,w,h)` in `dst` space.
/// - `buffer`: actual SHM pixels.
/// - `surface_size`: buffer-logical extent `(sw,sh)` = `buffer/scale` (with
///   transform applied), i.e. the `src` space extent.
/// - `viewport`: `None` when KWin sets no viewporter (damage already in
///   `surface_size` space); `Some` when dst/src mapping is needed.
/// - `buffer_scale`: integer `wl_surface.set_buffer_scale` (1..=8).
/// - `transform`: `wl_surface.set_buffer_transform`.
///
/// Returns `Ok(None)` for empty/fully-clipped rects (skip, keep other rects),
/// `Err(_)` for ambiguous state (caller must full-upload the frame).
pub fn surface_to_buffer(
    surface: (i32, i32, i32, i32),
    buffer: BufferSize,
    surface_size: (f64, f64),
    viewport: Option<ViewportState>,
    buffer_scale: i32,
    transform: BufferTransform,
) -> Result<Option<BufferRect>, DamageFallback> {
    let (sx, sy, sw, sh) = surface;
    if sw <= 0 || sh <= 0 {
        return Ok(None);
    }
    if buffer.w <= 0 || buffer.h <= 0 {
        return Err(DamageFallback::InvalidGeometry);
    }
    if !(1..=8).contains(&buffer_scale) {
        return Err(DamageFallback::InvalidScale);
    }
    let (surf_w, surf_h) = surface_size;
    if !(surf_w > 0.0 && surf_h > 0.0 && surf_w.is_finite() && surf_h.is_finite()) {
        return Err(DamageFallback::InvalidGeometry);
    }

    // Conservative recovery of protocol integer truncation: the true
    // fractional client damage could extend up to ~1 logical pixel beyond the
    // reported integer rect on every side. Expand first; the result stays
    // proportionally tiny (e.g. 10px -> 12px logical).
    let mut fx = sx as f64 - 1.0;
    let mut fy = sy as f64 - 1.0;
    let mut fw = sw as f64 + 2.0;
    let mut fh = sh as f64 + 2.0;

    // Viewport dst -> src mapping (float, before transform).
    if let Some(vp) = viewport {
        if !vp.validate(surf_w, surf_h) {
            return Err(DamageFallback::InvalidViewport);
        }
        let scale_x = vp.src_w / vp.dst_w as f64;
        let scale_y = vp.src_h / vp.dst_h as f64;
        if !(scale_x > 0.0 && scale_y > 0.0 && scale_x.is_finite() && scale_y.is_finite()) {
            return Err(DamageFallback::InvalidViewport);
        }
        fx = vp.src_x + fx * scale_x;
        fy = vp.src_y + fy * scale_y;
        fw *= scale_x;
        fh *= scale_y;
    }
    if !(fw > 0.0
        && fh > 0.0
        && fx.is_finite()
        && fy.is_finite()
        && fw.is_finite()
        && fh.is_finite())
    {
        return Ok(None);
    }

    // Transform in surface_size (src) space, then integer buffer scale.
    // Integer upscale is exact (buffer == surface*scale modulo 90/270 swap),
    // so a single final floor/ceil in buffer pixels preserves coverage.
    let (tx, ty, tw, th) = match transform_rect_f64((fx, fy, fw, fh), (surf_w, surf_h), transform) {
        Some(v) => v,
        None => return Err(DamageFallback::InvalidGeometry),
    };
    let bs = buffer_scale as f64;
    let bx0 = (tx * bs).floor();
    let by0 = (ty * bs).floor();
    let bx1 = ((tx + tw) * bs).ceil();
    let by1 = ((ty + th) * bs).ceil();
    if ![bx0, by0, bx1, by1].iter().all(|v| v.is_finite()) {
        return Err(DamageFallback::InvalidGeometry);
    }
    let rect = BufferRect {
        x: bx0 as i32,
        y: by0 as i32,
        w: (bx1 - bx0) as i32,
        h: (by1 - by0) as i32,
    };
    if rect.is_empty() {
        return Ok(None);
    }
    // Clamp to actual buffer bounds; fully-outside contributes nothing.
    Ok(rect.clamp_to(buffer))
}

/// Convert one `wl_surface.damage_buffer` (already buffer pixels) rect.
///
/// Pure clamp/validate: no scaling, no transform. `Ok(None)` = empty/outside
/// (skip); `Err` = invalid buffer geometry (full fallback).
pub fn buffer_to_buffer(
    damage: (i32, i32, i32, i32),
    buffer: BufferSize,
) -> Result<Option<BufferRect>, DamageFallback> {
    let (x, y, w, h) = damage;
    if w <= 0 || h <= 0 {
        return Ok(None);
    }
    if buffer.w <= 0 || buffer.h <= 0 {
        return Err(DamageFallback::InvalidGeometry);
    }
    Ok(BufferRect { x, y, w, h }.clamp_to(buffer))
}

/// Merge overlapping/touching rects to bound `glTexSubImage2D` calls.
///
/// Only rects that overlap or touch are merged, so distant tiny damages never
/// coalesce into a near-full-frame upload. Order-independent.
pub fn merge_rects(mut rects: Vec<BufferRect>) -> Vec<BufferRect> {
    rects.retain(|r| !r.is_empty());
    rects.dedup();
    let mut out: Vec<BufferRect> = Vec::with_capacity(rects.len());
    for mut rect in rects {
        // Merge with any existing overlapping/touching entries, transitively.
        loop {
            let mut merged_idx: Option<usize> = None;
            for (i, other) in out.iter().enumerate() {
                if other.overlaps_or_touches(rect) {
                    rect = other.merge(rect);
                    merged_idx = Some(i);
                    break;
                }
            }
            if let Some(i) = merged_idx {
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

/// Per-frame upload diagnostics (profiling only, debug-level logging).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UploadStats {
    pub damaged_pixels: i64,
    pub total_pixels: i64,
    pub percent: f64,
    pub num_rects: usize,
    pub bytes_uploaded: i64,
    pub is_full_fallback: bool,
}

impl UploadStats {
    pub fn for_rects(
        rects: &[BufferRect],
        buffer: BufferSize,
        bytes_per_pixel: i64,
        full: bool,
    ) -> Self {
        let total = buffer.w as i64 * buffer.h as i64;
        let damaged: i64 = rects.iter().map(|r| r.area()).sum();
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

    /// One-line profiling summary, e.g.
    /// `shm.upload partial rects=2 damaged=1296/8138880 0.02% bytes=5184`.
    pub fn log_line(&self, kind: &str, reason: &str) -> String {
        format!(
            "shm.upload {kind} rects={} damaged={}/{} {:.2}% bytes={} reason={reason}",
            self.num_rects,
            self.damaged_pixels,
            self.total_pixels,
            self.percent,
            self.bytes_uploaded,
        )
    }
}

/// Upload plan after validation/merge.
#[derive(Debug, Clone, PartialEq)]
pub enum UploadPlan {
    Full {
        reason: &'static str,
        stats: UploadStats,
    },
    Partial {
        rects: Vec<BufferRect>,
        stats: UploadStats,
    },
}

impl UploadPlan {
    pub fn stats(&self) -> &UploadStats {
        match self {
            UploadPlan::Full { stats, .. } => stats,
            UploadPlan::Partial { stats, .. } => stats,
        }
    }

    pub fn is_full(&self) -> bool {
        matches!(self, UploadPlan::Full { .. })
    }
}

/// Decide between partial uploads and the safe full fallback.
///
/// - `validated`: already-converted, clamped buffer rects (non-empty).
/// - `buffer`: actual SHM pixels.
/// - `is_new_texture`: fresh allocation / resize / size mismatch.
/// - `had_fallback_signal`: any damage failed validation (ambiguous).
/// - `bytes_per_pixel`: e.g. 4 for ARGB8888.
///
/// Full fallback when: new texture, ambiguous state, no usable damage, or the
/// validated damage already covers most of the frame (a single full upload is
/// then cheaper than many partials).
pub fn decide_upload(
    validated: Vec<BufferRect>,
    buffer: BufferSize,
    is_new_texture: bool,
    had_fallback_signal: bool,
    bytes_per_pixel: i64,
) -> UploadPlan {
    let total = buffer.w as i64 * buffer.h as i64;
    if is_new_texture {
        let stats = UploadStats::for_rects(
            &[BufferRect {
                x: 0,
                y: 0,
                w: buffer.w.max(0),
                h: buffer.h.max(0),
            }],
            buffer,
            bytes_per_pixel,
            true,
        );
        return UploadPlan::Full {
            reason: "new-texture",
            stats,
        };
    }
    if had_fallback_signal {
        let stats = UploadStats::for_rects(
            &[BufferRect {
                x: 0,
                y: 0,
                w: buffer.w.max(0),
                h: buffer.h.max(0),
            }],
            buffer,
            bytes_per_pixel,
            true,
        );
        return UploadPlan::Full {
            reason: "ambiguous-damage",
            stats,
        };
    }
    let merged = merge_rects(validated);
    if merged.is_empty() {
        let stats = UploadStats::for_rects(
            &[BufferRect {
                x: 0,
                y: 0,
                w: buffer.w.max(0),
                h: buffer.h.max(0),
            }],
            buffer,
            bytes_per_pixel,
            true,
        );
        return UploadPlan::Full {
            reason: "missing-damage",
            stats,
        };
    }
    let damaged: i64 = merged.iter().map(|r| r.area()).sum();
    // Near-full damage is cheaper as one full upload (also covers any
    // 1px conservative-expansion overshoot on fullscreen frames).
    if total > 0 && damaged * 100 >= total * 70 {
        let stats = UploadStats::for_rects(
            &[BufferRect {
                x: 0,
                y: 0,
                w: buffer.w,
                h: buffer.h,
            }],
            buffer,
            bytes_per_pixel,
            true,
        );
        return UploadPlan::Full {
            reason: "near-full-damage",
            stats,
        };
    }
    let stats = UploadStats::for_rects(&merged, buffer, bytes_per_pixel, false);
    UploadPlan::Partial {
        rects: merged,
        stats,
    }
}

/// Generation-tracked accumulator: unions damage across all commits since the
/// last successful texture upload. Mirrors the Smithay `DamageBag` +
/// `renderer_seen` contract in host-testable form.
#[derive(Debug, Clone)]
pub struct DamageAccumulator {
    pending: Vec<BufferRect>,
    pending_full: Option<&'static str>,
    last_buffer: Option<BufferSize>,
}

impl DamageAccumulator {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            pending_full: None,
            last_buffer: None,
        }
    }

    /// Record one commit's already-converted buffer rects.
    ///
    /// - `rects`: validated clamped rects for this commit (may be empty).
    /// - `buffer`: this commit's SHM size.
    /// - `fallback`: `Some(reason)` when this commit's damage was ambiguous.
    ///
    /// Resize (buffer size change) forces a full upload: new pixels outside
    /// the old extent have no valid damage history.
    pub fn note_commit(
        &mut self,
        rects: Vec<BufferRect>,
        buffer: BufferSize,
        fallback: Option<&'static str>,
    ) {
        if let Some(prev) = self.last_buffer {
            if prev != buffer {
                self.pending.clear();
                self.pending_full = Some("resize");
                self.last_buffer = Some(buffer);
                return;
            }
        } else {
            // First commit for a new texture generation: the initial upload
            // must be complete before partials are allowed.
            self.pending.clear();
            self.pending_full = Some("new-texture");
            self.last_buffer = Some(buffer);
            return;
        }
        if let Some(reason) = fallback {
            self.pending_full = Some(reason);
            return;
        }
        // Union: never drop an intermediate region because a newer commit
        // replaced it.
        self.pending
            .extend(rects.into_iter().filter(|r| !r.is_empty()));
    }

    /// Take the pending upload plan relative to the last successful upload.
    /// Returns `None` when nothing is pending (caller may skip the upload).
    pub fn take_plan(&mut self, buffer: BufferSize, bytes_per_pixel: i64) -> Option<UploadPlan> {
        if let Some(reason) = self.pending_full {
            let stats = UploadStats::for_rects(
                &[BufferRect {
                    x: 0,
                    y: 0,
                    w: buffer.w.max(0),
                    h: buffer.h.max(0),
                }],
                buffer,
                bytes_per_pixel,
                true,
            );
            return Some(UploadPlan::Full { reason, stats });
        }
        if self.pending.is_empty() {
            return None;
        }
        Some(decide_upload(
            std::mem::take(&mut self.pending),
            buffer,
            false,
            false,
            bytes_per_pixel,
        ))
    }

    /// Mark the last taken plan as successfully uploaded.
    pub fn mark_uploaded(&mut self) {
        self.pending.clear();
        self.pending_full = None;
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn needs_full(&self) -> bool {
        self.pending_full.is_some()
    }
}

impl Default for DamageAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

/// Fingerprint of everything that determines how committed damage must be
/// interpreted and what a cached GLES texture was last proven to contain.
///
/// Plain data (no Smithay/Wayland types) so the sync contract stays
/// host-testable; the Smithay patch mirrors this struct field-for-field and
/// the wiring builds it from live surface state on every commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncFingerprint {
    /// Physical SHM buffer dimensions.
    pub buffer_w: i32,
    pub buffer_h: i32,
    /// Integer `wl_surface.set_buffer_scale`.
    pub buffer_scale: i32,
    /// `wl_surface.set_buffer_transform` discriminant (caller-defined mapping).
    pub transform: u8,
    /// Viewporter src `(x, y, w, h)` as `f64` bits.
    pub view_src: [u64; 4],
    /// Viewporter dst `(w, h)`.
    pub view_dst: [i32; 2],
}

impl SyncFingerprint {
    /// Classify drift between the last proven state and the current one.
    /// Size drift forces realloc-class resync; anything else is a
    /// reinterpretation-class resync. Both are full uploads.
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
    /// was synced to. Same object + complete damage may partial-upload;
    /// a replaced object must fully resync (object identity is not content
    /// identity, and a fresh/rotated buffer has no proven pixel history).
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
/// Portal knows exactly which committed SHM-buffer generation that texture
/// represents (`uploaded`) and holds complete, ordered damage for every
/// subsequent generation up to `committed`, with no identity/geometry/
/// interpretation drift in between. Anything else forces exactly one full
/// upload, after which the generation resynchronizes and partials may resume.
///
/// This is the host-testable spec. The Smithay patch
/// (`backend::renderer::utils::shm_damage::ShmSyncTracker`) mirrors it, and
/// `RendererSurfaceState` wires it to real commits/uploads. Damage-rect
/// transport stays in `DamageBag`/`renderer_seen`; this tracker is purely the
/// proof gate and can only ever widen full-uploads, never narrow them.
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
                // A replaced buffer object has no proven pixel history, even
                // at identical dimensions.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(w: i32, h: i32) -> BufferSize {
        BufferSize { w, h }
    }

    #[test]
    fn tiny_surface_damage_stays_tiny_at_fractional_scale() {
        // 3392x2400 buffer, integer scale 1, viewport 1507x1066 (~2.25x).
        // A 10x10 cursor-size change must stay proportionally tiny, not full.
        let buffer = buf(3392, 2400);
        let surface_size = (3392.0, 2400.0);
        let vp = ViewportState {
            src_x: 0.0,
            src_y: 0.0,
            src_w: 3392.0,
            src_h: 2400.0,
            dst_w: 1507,
            dst_h: 1066,
        };
        let rect = surface_to_buffer(
            (100, 100, 10, 10),
            buffer,
            surface_size,
            Some(vp),
            1,
            BufferTransform::Normal,
        )
        .expect("valid")
        .expect("non-empty");
        // 10 logical + 2px conservative expansion = 12 logical * ~2.25.
        assert!(rect.w >= 26 && rect.w <= 30, "w={}", rect.w);
        assert!(rect.h >= 26 && rect.h <= 30, "h={}", rect.h);
        let plan = decide_upload(vec![rect], buffer, false, false, 4);
        assert!(!plan.is_full(), "tiny damage must stay partial");
        assert!(
            plan.stats().percent < 0.1,
            "percent={}",
            plan.stats().percent
        );
    }

    #[test]
    fn fractional_damage_coverage_uses_floor_ceil() {
        // Damage at fractional viewport positions must encapsulate: floor min,
        // ceil max.
        let buffer = buf(3392, 2400);
        let surface_size = (3392.0, 2400.0);
        let vp = ViewportState {
            src_x: 0.0,
            src_y: 0.0,
            src_w: 3392.0,
            src_h: 2400.0,
            dst_w: 1507,
            dst_h: 1066,
        };
        let a = surface_to_buffer(
            (0, 0, 1, 1),
            buffer,
            surface_size,
            Some(vp),
            1,
            BufferTransform::Normal,
        )
        .unwrap()
        .unwrap();
        // 1px logical + expansion covers origin; must include (0,0).
        // (-1,-1,3,3) logical * ~2.25 = (-2.25,-2.25,6.75,6.75) buffer,
        // floored/ceiled then clamped to (0,0,5,5).
        assert_eq!((a.x, a.y), (0, 0));
        assert!(a.w >= 5 && a.h >= 5, "{a:?}");
    }

    #[test]
    fn negative_and_out_of_bounds_damage_is_clamped_not_lost() {
        let buffer = buf(800, 600);
        // Partly outside top-left: clamped, still partial.
        let r = buffer_to_buffer((-50, -20, 100, 100), buffer)
            .unwrap()
            .unwrap();
        assert_eq!(
            r,
            BufferRect {
                x: 0,
                y: 0,
                w: 50,
                h: 80
            }
        );
        // Fully outside: skipped (None), not fallback.
        assert_eq!(buffer_to_buffer((900, 700, 50, 50), buffer).unwrap(), None);
        assert_eq!(buffer_to_buffer((0, 0, 0, 10), buffer).unwrap(), None);
        // Negative surface damage clamps into the buffer.
        let s = surface_to_buffer(
            (-5, -5, 20, 20),
            buffer,
            (800.0, 600.0),
            None,
            1,
            BufferTransform::Normal,
        )
        .unwrap()
        .unwrap();
        assert_eq!((s.x, s.y), (0, 0));
        assert!(s.w > 0 && s.h > 0);
    }

    #[test]
    fn overlapping_rects_merge_but_distant_tiny_stay_split() {
        let a = BufferRect {
            x: 0,
            y: 0,
            w: 100,
            h: 100,
        };
        let b = BufferRect {
            x: 50,
            y: 50,
            w: 100,
            h: 100,
        };
        let merged = merge_rects(vec![a, b]);
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0],
            BufferRect {
                x: 0,
                y: 0,
                w: 150,
                h: 150
            }
        );
        // Distant tiny rects must NOT coalesce into near-full.
        let tiny1 = BufferRect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        };
        let tiny2 = BufferRect {
            x: 3000,
            y: 2000,
            w: 10,
            h: 10,
        };
        let merged = merge_rects(vec![tiny1, tiny2]);
        assert_eq!(merged.len(), 2);
        let plan = decide_upload(merged, buf(3392, 2400), false, false, 4);
        assert!(!plan.is_full());
    }

    #[test]
    fn accumulator_unions_multiple_commits_before_upload() {
        let buffer = buf(800, 600);
        let mut acc = DamageAccumulator::new();
        // First commit establishes the generation -> forces initial full.
        acc.note_commit(vec![], buffer, None);
        assert!(acc.needs_full());
        let plan = acc.take_plan(buffer, 4).expect("full");
        assert!(plan.is_full());
        acc.mark_uploaded();
        // Two commits before the next upload must union, not replace.
        acc.note_commit(
            vec![BufferRect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            }],
            buffer,
            None,
        );
        acc.note_commit(
            vec![BufferRect {
                x: 100,
                y: 100,
                w: 20,
                h: 20,
            }],
            buffer,
            None,
        );
        assert_eq!(acc.pending_count(), 2);
        let plan = acc.take_plan(buffer, 4).expect("partial");
        assert!(!plan.is_full());
        match plan {
            UploadPlan::Partial { rects, .. } => assert_eq!(rects.len(), 2),
            _ => panic!("expected partial"),
        }
        acc.mark_uploaded();
        assert_eq!(acc.pending_count(), 0);
    }

    #[test]
    fn resize_and_new_texture_force_full() {
        let mut acc = DamageAccumulator::new();
        acc.note_commit(vec![], buf(800, 600), None);
        acc.mark_uploaded();
        // Same-size partial is fine.
        acc.note_commit(
            vec![BufferRect {
                x: 0,
                y: 0,
                w: 5,
                h: 5,
            }],
            buf(800, 600),
            None,
        );
        assert!(!acc.needs_full());
        // Resize discards history and forces full.
        acc.note_commit(
            vec![BufferRect {
                x: 0,
                y: 0,
                w: 5,
                h: 5,
            }],
            buf(1024, 768),
            None,
        );
        assert!(acc.needs_full());
        let plan = acc.take_plan(buf(1024, 768), 4).unwrap();
        assert!(plan.is_full());
    }

    #[test]
    fn ambiguous_damage_falls_back_to_full() {
        // Invalid viewport -> Err -> full.
        let bad_vp = ViewportState {
            src_x: 0.0,
            src_y: 0.0,
            src_w: -10.0,
            src_h: 100.0,
            dst_w: 100,
            dst_h: 100,
        };
        let r = surface_to_buffer(
            (0, 0, 10, 10),
            buf(800, 600),
            (800.0, 600.0),
            Some(bad_vp),
            1,
            BufferTransform::Normal,
        );
        assert!(r.is_err());
        // Invalid scale -> Err.
        assert!(surface_to_buffer(
            (0, 0, 10, 10),
            buf(800, 600),
            (800.0, 600.0),
            None,
            0,
            BufferTransform::Normal
        )
        .is_err());
        // decide_upload with fallback signal -> full.
        let plan = decide_upload(
            vec![BufferRect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            }],
            buf(800, 600),
            false,
            true,
            4,
        );
        assert!(plan.is_full());
        // New texture -> full even with tiny damage.
        let plan = decide_upload(
            vec![BufferRect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            }],
            buf(800, 600),
            true,
            false,
            4,
        );
        assert!(plan.is_full());
    }

    #[test]
    fn full_surface_damage_uses_single_full_upload() {
        let buffer = buf(3392, 2400);
        let full = BufferRect {
            x: 0,
            y: 0,
            w: 3392,
            h: 2400,
        };
        let plan = decide_upload(vec![full], buffer, false, false, 4);
        assert!(plan.is_full());
        assert_eq!(plan.stats().bytes_uploaded, 3392_i64 * 2400 * 4);
    }

    #[test]
    fn tiny_damage_reports_proportional_bandwidth() {
        let buffer = buf(3392, 2400);
        let tiny = BufferRect {
            x: 100,
            y: 100,
            w: 12,
            h: 12,
        };
        let plan = decide_upload(vec![tiny], buffer, false, false, 4);
        let stats = plan.stats().clone();
        assert!(!plan.is_full());
        assert_eq!(stats.damaged_pixels, 144);
        assert_eq!(stats.total_pixels, 3392 * 2400);
        assert_eq!(stats.bytes_uploaded, 576);
        assert!(stats.percent < 0.01);
        assert_eq!(stats.num_rects, 1);
    }

    #[test]
    fn transforms_stay_in_bounds() {
        let buffer = buf(800, 600);
        // 180-degree transform of a corner rect lands in the opposite corner.
        let r = surface_to_buffer(
            (0, 0, 10, 10),
            buffer,
            (800.0, 600.0),
            None,
            1,
            BufferTransform::R180,
        )
        .unwrap()
        .unwrap();
        assert!(r.x > 700 && r.y > 500, "{r:?}");
        assert!(r.clamp_to(buffer).is_some());
    }

    // ---- ShmSyncTracker: explicit content-generation gate ----

    fn fp(w: i32, h: i32) -> SyncFingerprint {
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

    fn content(fp_: SyncFingerprint, new_object: bool) -> CommitSyncEvent {
        CommitSyncEvent {
            new_content: true,
            new_object,
            fingerprint: fp_,
            damage_complete: true,
            damage_reason: None,
        }
    }

    fn bad_content(fp_: SyncFingerprint, reason: &'static str) -> CommitSyncEvent {
        CommitSyncEvent {
            new_content: true,
            new_object: false,
            fingerprint: fp_,
            damage_complete: false,
            damage_reason: Some(reason),
        }
    }

    #[test]
    fn sync_first_commit_needs_full_then_partial_then_up_to_date() {
        let mut t = ShmSyncTracker::new();
        assert_eq!(
            t.decide(),
            SyncDecision::Full {
                reason: "unknown-generation"
            }
        );
        t.note_commit(content(fp(800, 600), true));
        assert_eq!(
            t.decide(),
            SyncDecision::Full {
                reason: "unknown-generation"
            }
        );
        t.mark_uploaded(true);
        assert_eq!(t.decide(), SyncDecision::UpToDate);
        // A new complete generation on the same buffer object may partial.
        t.note_commit(content(fp(800, 600), false));
        assert_eq!(t.decide(), SyncDecision::Partial);
        t.mark_uploaded(true);
        assert_eq!(t.decide(), SyncDecision::UpToDate);
        assert_eq!(t.uploaded(), Some(2));
    }

    #[test]
    fn sync_multi_commit_accumulation_stays_partial() {
        let mut t = ShmSyncTracker::new();
        t.note_commit(content(fp(800, 600), true));
        t.mark_uploaded(true);
        // Several commits before one upload: every generation is recorded, so
        // the union upload stays provably complete.
        t.note_commit(content(fp(800, 600), false));
        t.note_commit(content(fp(800, 600), false));
        t.note_commit(content(fp(800, 600), false));
        assert_eq!(t.committed(), 4);
        assert_eq!(t.decide(), SyncDecision::Partial);
        t.mark_uploaded(true);
        assert_eq!(t.decide(), SyncDecision::UpToDate);
    }

    #[test]
    fn sync_buffer_reuse_with_complete_damage_stays_partial() {
        let mut t = ShmSyncTracker::new();
        t.note_commit(content(fp(3392, 2400), true));
        t.mark_uploaded(true);
        // Same wl_buffer object re-committed with new contents + complete
        // damage: the fast path (typing lines, scroll strips, cursor).
        for _ in 0..5 {
            t.note_commit(content(fp(3392, 2400), false));
            assert_eq!(t.decide(), SyncDecision::Partial);
            t.mark_uploaded(true);
        }
        assert_eq!(t.decide(), SyncDecision::UpToDate);
    }

    #[test]
    fn sync_buffer_object_replacement_forces_full_then_resumes() {
        let mut t = ShmSyncTracker::new();
        t.note_commit(content(fp(800, 600), true));
        t.mark_uploaded(true);
        t.note_commit(content(fp(800, 600), false));
        t.mark_uploaded(true);
        // Rotated/fresh wl_buffer object: no proven pixel history.
        t.note_commit(content(fp(800, 600), true));
        assert_eq!(
            t.decide(),
            SyncDecision::Full {
                reason: "buffer-replaced"
            }
        );
        // Failure to resync must not clear the requirement.
        t.mark_uploaded(false);
        assert_eq!(
            t.decide(),
            SyncDecision::Full {
                reason: "buffer-replaced"
            }
        );
        t.mark_uploaded(true);
        // After one successful full upload, partials resume safely.
        t.note_commit(content(fp(800, 600), false));
        assert_eq!(t.decide(), SyncDecision::Partial);
    }

    #[test]
    fn sync_history_gap_forces_full_until_resync() {
        let mut t = ShmSyncTracker::new();
        t.note_commit(content(fp(800, 600), true));
        t.mark_uploaded(true);
        t.note_commit(content(fp(800, 600), false));
        // Damage history evicted before upload: completeness unprovable.
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
    fn sync_failed_upload_never_advances_generation() {
        let mut t = ShmSyncTracker::new();
        t.note_commit(content(fp(800, 600), true));
        assert_eq!(t.committed(), 1);
        t.mark_uploaded(false);
        assert_eq!(t.uploaded(), None);
        assert_eq!(
            t.decide(),
            SyncDecision::Full {
                reason: "unknown-generation"
            }
        );
        // Partial authorized, then the GL upload fails: the generation must
        // stay behind so the next import retries a superset.
        t.mark_uploaded(true);
        t.note_commit(content(fp(800, 600), false));
        assert_eq!(t.decide(), SyncDecision::Partial);
        t.mark_uploaded(false);
        assert_eq!(t.uploaded(), Some(1));
        assert_eq!(t.decide(), SyncDecision::Partial);
        t.mark_uploaded(true);
        assert_eq!(t.decide(), SyncDecision::UpToDate);
    }

    #[test]
    fn sync_ambiguous_or_missing_damage_forces_full() {
        let mut t = ShmSyncTracker::new();
        t.note_commit(content(fp(800, 600), true));
        t.mark_uploaded(true);
        t.note_commit(bad_content(fp(800, 600), "ambiguous-damage"));
        assert_eq!(
            t.decide(),
            SyncDecision::Full {
                reason: "ambiguous-damage"
            }
        );
        t.mark_uploaded(true);
        t.note_commit(bad_content(fp(800, 600), "missing-damage"));
        assert_eq!(
            t.decide(),
            SyncDecision::Full {
                reason: "missing-damage"
            }
        );
        t.mark_uploaded(true);
        assert_eq!(t.decide(), SyncDecision::UpToDate);
    }

    #[test]
    fn sync_resize_view_and_scale_drift_force_full() {
        let mut t = ShmSyncTracker::new();
        t.note_commit(content(fp(800, 600), true));
        t.mark_uploaded(true);
        // Size drift.
        t.note_commit(content(fp(1024, 768), false));
        assert_eq!(t.decide(), SyncDecision::Full { reason: "resize" });
        t.mark_uploaded(true);
        // Viewport reinterpretation at identical size.
        let mut view_changed = fp(1024, 768);
        view_changed.view_dst = [512, 384];
        t.note_commit(content(view_changed, false));
        assert_eq!(
            t.decide(),
            SyncDecision::Full {
                reason: "view-change"
            }
        );
        t.mark_uploaded(true);
        // Scale/transform drift without size change.
        let mut scale_changed = view_changed;
        scale_changed.buffer_scale = 2;
        t.note_commit(content(scale_changed, false));
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
    fn sync_state_only_commit_without_drift_stays_quiet() {
        let mut t = ShmSyncTracker::new();
        t.note_commit(content(fp(800, 600), true));
        t.mark_uploaded(true);
        // No new pixels and no reinterpretation: nothing to do.
        t.note_commit(CommitSyncEvent {
            new_content: false,
            new_object: false,
            fingerprint: fp(800, 600),
            damage_complete: true,
            damage_reason: None,
        });
        assert_eq!(t.decide(), SyncDecision::UpToDate);
        // A mid-chain excursion that returns to the proven fingerprint before
        // any upload stays uploadable; drift still present at decide() forces
        // a resync.
        let mut drifted = fp(800, 600);
        drifted.view_dst = [400, 300];
        t.note_commit(CommitSyncEvent {
            new_content: false,
            new_object: false,
            fingerprint: drifted,
            damage_complete: true,
            damage_reason: None,
        });
        assert_eq!(
            t.decide(),
            SyncDecision::Full {
                reason: "view-change"
            }
        );
    }

    #[test]
    fn sync_reset_forgets_everything() {
        let mut t = ShmSyncTracker::new();
        t.note_commit(content(fp(800, 600), true));
        t.mark_uploaded(true);
        t.note_commit(content(fp(800, 600), false));
        t.reset();
        assert_eq!(
            t.decide(),
            SyncDecision::Full {
                reason: "unknown-generation"
            }
        );
        assert_eq!(t.uploaded(), None);
    }
}
