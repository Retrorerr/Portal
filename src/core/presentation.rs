//! Aspect-preserving presentation geometry for Android window resizing.
//!
//! This module is intentionally platform-independent (no Android/JNI/Smithay
//! dependencies) so the resize state machine can be exhaustively tested on the
//! host.
//!
//! Three scale concepts are kept rigorously separate:
//! 1. Android density (dpi → baseline scale, e.g. 420dpi → 2.625) —
//!    describes the physical panel, never changes on resize.
//! 2. KWin/Plasma desktop UI scale (e.g. 2.25) — user-configured, stable
//!    across resizes, read from `kwinoutputconfig.json`, never derived from
//!    stale presentation state.
//! 3. Temporary presentation scale used to FIT a committed KWin frame inside
//!    the current Android window — uniform, aspect-preserving, allows <1 for
//!    downscaling, centered with temporary letterboxing. Never stretches.

/// Returns true for a usable Android host surface size.
#[inline]
pub fn is_valid_host_size(w: i32, h: i32) -> bool {
    w > 0 && h > 0
}

/// Returns true for a usable committed guest surface size (surface-local logical units).
#[inline]
pub fn is_valid_surface_size(w: f64, h: f64) -> bool {
    w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()
}

/// Uniform aspect-preserving FIT scale: `min(host_w/guest_w, host_h/guest_h)`.
///
/// Allows values below 1 (downscaling an old large frame into a smaller popup).
/// Returns `None` for invalid/degenerate geometry (including zero surfaces —
/// callers must preserve last valid state instead of fabricating 1px geometry).
pub fn uniform_fit_scale(host: (i32, i32), guest: (f64, f64)) -> Option<f64> {
    if !is_valid_host_size(host.0, host.1) || !is_valid_surface_size(guest.0, guest.1) {
        return None;
    }
    let sx = host.0 as f64 / guest.0;
    let sy = host.1 as f64 / guest.1;
    if !sx.is_finite() || !sy.is_finite() || sx <= 0.0 || sy <= 0.0 {
        return None;
    }
    let scale = sx.min(sy);
    (scale.is_finite() && scale > 0.0).then_some(scale)
}

/// Centered FIT viewport for `guest` inside `host` using the uniform scale.
///
/// `viewport_size = guest * scale`, `viewport_origin = (host - viewport)/2`.
/// When aspects match the viewport fills the host (fullscreen); otherwise one
/// axis letterboxes/pillarboxes. Never stretches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitViewport {
    pub scale: f64,
    pub origin: (f64, f64),
    pub size: (f64, f64),
}

pub fn fit_viewport(host: (i32, i32), guest: (f64, f64)) -> Option<FitViewport> {
    let scale = uniform_fit_scale(host, guest)?;
    let vw = guest.0 * scale;
    let vh = guest.1 * scale;
    if !vw.is_finite() || !vh.is_finite() || vw <= 0.0 || vh <= 0.0 {
        return None;
    }
    // FIT guarantees vw <= host_w and vh <= host_h (within float rounding).
    // Clamp tiny overshoots caused by rounding so origin never goes negative.
    let host_w = host.0 as f64;
    let host_h = host.1 as f64;
    let vw = vw.min(host_w);
    let vh = vh.min(host_h);
    let ox = ((host_w - vw) / 2.0).max(0.0);
    let oy = ((host_h - vh) / 2.0).max(0.0);
    Some(FitViewport {
        scale,
        origin: (ox, oy),
        size: (vw, vh),
    })
}

/// Single coherent immutable geometry snapshot.
///
/// Rendering, cursor, mouse and touch must all derive from the same snapshot
/// for one operation — never mix `window.inner_size()` in one subsystem with
/// cached resize state from another generation in a different subsystem.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationSnapshot {
    /// Monotonically increasing host resize generation. A newer resize must
    /// never be overwritten by state from an obsolete generation.
    pub generation: u64,
    /// Current Android host target geometry (physical pixels, always >0).
    pub host: (i32, i32),
    /// KWin logical desktop geometry for the committed frame
    /// (e.g. round(host_converged / plasma_scale)). Stable across
    /// transitional resizes — still describes the OLD desktop until the
    /// matching KWin commit arrives.
    pub guest_logical: (f64, f64),
    /// Latest valid committed KWin surface geometry (surface-local logical).
    pub committed: Option<(f64, f64)>,
    /// User-configured Plasma UI scale. Stable across resizes.
    pub plasma_scale: f64,
    /// Uniform aspect-preserving presentation scale (allows <1).
    pub uniform_scale: f64,
    /// Viewport origin in host physical pixels (centered, 0,0 when fullscreen).
    pub viewport_origin: (f64, f64),
    /// Viewport size in host physical pixels (== host when fullscreen).
    pub viewport_size: (f64, f64),
    /// True when the viewport fills the host (no letterboxing).
    pub converged: bool,
    /// Request generation the rendered frame was attributed to (owning
    /// request in the configure history; 0 = initial/unknown).
    pub rendered_generation: u64,
    /// Host size the rendered frame was produced for.
    pub rendered_host: (i32, i32),
    /// Newest geometry requested from KWin (authoritative future target;
    /// never moved backward by stale commits).
    pub requested: (i32, i32),
    /// Generation of `requested`.
    pub requested_generation: u64,
    /// Last xdg configure serial acked by KWin (0 = none yet).
    pub acked_serial: u32,
}

impl PresentationSnapshot {
    /// Android physical → KWin logical. Returns `None` for touches/clicks in
    /// transitional letterbox borders — they must NOT be clamped into edge
    /// clicks on Plasma.
    pub fn physical_to_logical(&self, px: f64, py: f64) -> Option<(f64, f64)> {
        if !px.is_finite() || !py.is_finite() {
            return None;
        }
        if self.guest_logical.0 <= 0.0
            || self.guest_logical.1 <= 0.0
            || !self.guest_logical.0.is_finite()
            || !self.guest_logical.1.is_finite()
        {
            return None;
        }
        if self.viewport_size.0 <= 0.0
            || self.viewport_size.1 <= 0.0
            || !self.viewport_size.0.is_finite()
            || !self.viewport_size.1.is_finite()
        {
            return None;
        }
        let lx = px - self.viewport_origin.0;
        let ly = py - self.viewport_origin.1;
        // Outside the presented viewport (letterbox/pillarbox) → outside guest.
        if lx < 0.0 || ly < 0.0 || lx > self.viewport_size.0 || ly > self.viewport_size.1 {
            return None;
        }
        let u = lx / self.viewport_size.0;
        let v = ly / self.viewport_size.1;
        if !u.is_finite() || !v.is_finite() {
            return None;
        }
        Some((u * self.guest_logical.0, v * self.guest_logical.1))
    }

    /// KWin logical → Android physical (for cursor placement). Clamps to keep
    /// the cursor visible; inner points map exactly (round-trip <1px with
    /// `physical_to_logical`).
    pub fn logical_to_physical(&self, lx: f64, ly: f64) -> Option<(f64, f64)> {
        if !lx.is_finite() || !ly.is_finite() {
            return None;
        }
        if self.guest_logical.0 <= 0.0
            || self.guest_logical.1 <= 0.0
            || !self.guest_logical.0.is_finite()
            || !self.guest_logical.1.is_finite()
        {
            return None;
        }
        let u = (lx / self.guest_logical.0).clamp(0.0, 1.0);
        let v = (ly / self.guest_logical.1).clamp(0.0, 1.0);
        if !u.is_finite() || !v.is_finite() {
            return None;
        }
        Some((
            self.viewport_origin.0 + u * self.viewport_size.0,
            self.viewport_origin.1 + v * self.viewport_size.1,
        ))
    }

    /// Rendered sprite origin for a cursor with the given logical position and
    /// surface-local hotspot, using the exact same geometry as the desktop.
    /// `hotspot_origin + hotspot*uniform_scale == pointer_physical` (within
    /// rounding to integer sprite location, <1px error).
    pub fn cursor_sprite_origin(
        &self,
        pointer_logical: (f64, f64),
        hotspot: (f64, f64),
    ) -> Option<(f64, f64)> {
        let (px, py) = self.logical_to_physical(pointer_logical.0, pointer_logical.1)?;
        if !hotspot.0.is_finite() || !hotspot.1.is_finite() {
            return None;
        }
        Some((
            px - hotspot.0 * self.uniform_scale,
            py - hotspot.1 * self.uniform_scale,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_allows_downscaling_and_centers() {
        // Old fullscreen frame 1130x800 (from 3392x2400 host, buffer_scale 3)
        // must downscale into a smaller 1100x900 popup.
        let vp = fit_viewport((1100, 900), (1130.0, 800.0)).unwrap();
        let expected = (1100.0f64 / 1130.0).min(900.0 / 800.0);
        assert!((vp.scale - expected).abs() < 1e-9);
        assert!(vp.scale < 1.0, "must allow downscale, got {}", vp.scale);
        assert!((vp.size.0 - 1130.0 * vp.scale).abs() < 1e-6);
        assert!((vp.size.1 - 800.0 * vp.scale).abs() < 1e-6);
        // Centered: origin = (host - viewport)/2.
        assert!((vp.origin.0 - (1100.0 - vp.size.0) / 2.0).abs() < 1e-6);
        assert!((vp.origin.1 - (900.0 - vp.size.1) / 2.0).abs() < 1e-6);
        // No stretch: viewport aspect == guest aspect.
        assert!((vp.size.0 / vp.size.1 - 1130.0 / 800.0).abs() < 1e-9);
    }

    #[test]
    fn fit_rejects_zero_without_fabrication() {
        assert_eq!(uniform_fit_scale((0, 900), (1130.0, 800.0)), None);
        assert_eq!(uniform_fit_scale((1100, 0), (1130.0, 800.0)), None);
        assert_eq!(uniform_fit_scale((1100, 900), (0.0, 800.0)), None);
        assert_eq!(fit_viewport((0, 0), (1130.0, 800.0)), None);
    }
}
