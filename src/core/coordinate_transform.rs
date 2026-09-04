//! Authoritative mapping between the Android surface and nested Wayland desktop.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicalPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicalPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewportTransform {
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoordinateTransform {
    physical_destination: PhysicalRect,
    logical_source: LogicalRect,
    transform: ViewportTransform,
}

impl CoordinateTransform {
    pub fn new(
        physical_destination: PhysicalRect,
        logical_source: LogicalRect,
        transform: ViewportTransform,
    ) -> Option<Self> {
        let values = [
            physical_destination.x,
            physical_destination.y,
            physical_destination.width,
            physical_destination.height,
            logical_source.x,
            logical_source.y,
            logical_source.width,
            logical_source.height,
        ];
        (values.iter().all(|v| v.is_finite())
            && physical_destination.width > 0.0
            && physical_destination.height > 0.0
            && logical_source.width > 0.0
            && logical_source.height > 0.0)
            .then_some(Self {
                physical_destination,
                logical_source,
                transform,
            })
    }

    pub fn identity(width: i32, height: i32) -> Self {
        let width = width.max(1) as f64;
        let height = height.max(1) as f64;
        Self::new(
            PhysicalRect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
            LogicalRect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
            ViewportTransform::Normal,
        )
        .expect("positive identity viewport")
    }

    pub fn physical_destination(&self) -> PhysicalRect {
        self.physical_destination
    }

    pub fn logical_source(&self) -> LogicalRect {
        self.logical_source
    }

    /// Android physical surface pixels to KWin logical coordinates.
    pub fn physical_to_logical(&self, point: PhysicalPoint) -> LogicalPoint {
        let u = unit((point.x - self.physical_destination.x) / self.physical_destination.width);
        let v = unit((point.y - self.physical_destination.y) / self.physical_destination.height);
        let (u, v) = inverse_unit_transform(self.transform, u, v);
        LogicalPoint {
            x: self.logical_source.x + u * self.logical_source.width,
            y: self.logical_source.y + v * self.logical_source.height,
        }
    }

    /// KWin logical coordinates to Android physical surface pixels.
    pub fn logical_to_physical(&self, point: LogicalPoint) -> PhysicalPoint {
        let u = unit((point.x - self.logical_source.x) / self.logical_source.width);
        let v = unit((point.y - self.logical_source.y) / self.logical_source.height);
        let (u, v) = forward_unit_transform(self.transform, u, v);
        PhysicalPoint {
            x: self.physical_destination.x + u * self.physical_destination.width,
            y: self.physical_destination.y + v * self.physical_destination.height,
        }
    }
}

fn unit(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn forward_unit_transform(t: ViewportTransform, u: f64, v: f64) -> (f64, f64) {
    match t {
        ViewportTransform::Normal => (u, v),
        ViewportTransform::Rotate90 => (1.0 - v, u),
        ViewportTransform::Rotate180 => (1.0 - u, 1.0 - v),
        ViewportTransform::Rotate270 => (v, 1.0 - u),
        ViewportTransform::Flipped => (1.0 - u, v),
        ViewportTransform::Flipped90 => (1.0 - v, 1.0 - u),
        ViewportTransform::Flipped180 => (u, 1.0 - v),
        ViewportTransform::Flipped270 => (v, u),
    }
}

fn inverse_unit_transform(t: ViewportTransform, u: f64, v: f64) -> (f64, f64) {
    match t {
        ViewportTransform::Normal => (u, v),
        ViewportTransform::Rotate90 => (v, 1.0 - u),
        ViewportTransform::Rotate180 => (1.0 - u, 1.0 - v),
        ViewportTransform::Rotate270 => (1.0 - v, u),
        ViewportTransform::Flipped => (1.0 - u, v),
        ViewportTransform::Flipped90 => (1.0 - v, 1.0 - u),
        ViewportTransform::Flipped180 => (u, 1.0 - v),
        ViewportTransform::Flipped270 => (v, u),
    }
}

/// Authoritative state representing display geometry, density, configure size,
/// observed KWin surface size, and derived coordinate transforms.
///
/// Dynamic-geometry invariants (fullscreen ↔ popup resizing):
/// - `physical_size` is the current Android host target (always >0, last valid
///   preserved — zero/invalid resizes never fabricate 1px geometry).
/// - `requested_configure` is the last geometry actually requested from KWin.
/// - `observed_surface_size` is the latest valid committed KWin surface.
/// - `guest_logical_size` is the KWin logical desktop for the COMMITTED frame
///   (old desktop during transitions, NOT the future host/plasma).
/// - `resize_generation` increases monotonically; newer resizes never lose to
///   obsolete state. No sleeps/timing guesses.
/// - `kwin_scale` (Plasma UI scale) is stable across resizes, never derived
///   from stale presentation state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AuthoritativeDisplayState {
    /// Physical dimensions of the Android surface (W_phys, H_phys)
    pub physical_size: (i32, i32),
    /// Android screen density DPI (e.g. 420)
    pub density_dpi: i32,
    /// Refresh rate in millihertz (e.g. 120000)
    pub refresh_rate_millihz: i32,
    /// Transform for orientation / rotation
    pub transform: ViewportTransform,
    /// Currently observed surface size from KWin in surface-local coordinates (W_surf, H_surf).
    /// Used STRICTLY for render presentation scaling of the committed buffer texture.
    pub observed_surface_size: Option<(f64, f64)>,
    /// Configured/observed KWin logical scale factor (e.g. 2.0, 2.25).
    /// Used STRICTLY for logical output geometry and input coordinate mapping.
    pub kwin_scale: Option<f64>,
    /// Monotonically increasing host resize generation.
    pub resize_generation: u64,
    /// Last geometry actually requested from KWin (configure sent).
    pub requested_configure: (i32, i32),
    /// Generation of `requested_configure`.
    pub requested_generation: u64,
    /// KWin logical desktop for the committed frame (old during transitions).
    pub guest_logical_size: (f64, f64),
    /// True when the committed frame fills the host (no letterboxing).
    pub converged: bool,
    /// mtime of `kwinoutputconfig.json` at last successful parse, nanos since
    /// UNIX epoch. Lets the compositor refresh Plasma scale only when the
    /// authoritative file actually changes (no per-frame JSON parsing).
    pub kwin_config_mtime_ns: Option<u64>,
}

impl AuthoritativeDisplayState {
    pub fn new(
        physical_w: i32,
        physical_h: i32,
        density_dpi: i32,
        refresh_rate_millihz: i32,
    ) -> Self {
        // Initial construction has no history to preserve, so clamp to 1.
        // All later updates MUST preserve last valid instead (see
        // `try_update_physical_size`).
        let physical_size = (physical_w.max(1), physical_h.max(1));
        let mut state = Self {
            physical_size,
            density_dpi: density_dpi.max(1),
            refresh_rate_millihz: refresh_rate_millihz.max(1000),
            transform: ViewportTransform::Normal,
            observed_surface_size: None,
            kwin_scale: None,
            resize_generation: 0,
            requested_configure: physical_size,
            requested_generation: 0,
            guest_logical_size: (1.0, 1.0),
            converged: true,
            kwin_config_mtime_ns: None,
        };
        state.guest_logical_size = state.fresh_logical_geometry();
        state
    }

    fn fresh_logical_geometry(&self) -> (f64, f64) {
        let scale = self.effective_kwin_scale();
        let w = (self.physical_size.0 as f64 / scale).round().max(1.0);
        let h = (self.physical_size.1 as f64 / scale).round().max(1.0);
        (w, h)
    }

    /// The configure dimensions sent to the nested compositor toplevel.
    /// Invariant: ALWAYS derived solely from physical size and orientation,
    /// NEVER from observed surface/buffer size.
    #[inline]
    pub fn configure_size(&self) -> (i32, i32) {
        self.physical_size
    }

    /// Baseline density scale factor from Android DPI (160 DPI = 1.0)
    #[inline]
    pub fn baseline_density_scale(&self) -> f64 {
        (self.density_dpi as f64 / 160.0).clamp(1.0, 8.0)
    }

    /// Returns the effective KWin scale factor.
    /// Priority:
    /// 1. Authoritative KWin scale from kwinoutputconfig / update_kwin_scale
    /// 2. Baseline density scale from Android DPI
    ///
    /// Deliberately NEVER derived from `observed_surface_size`: during a
    /// resize the committed frame is stale (old geometry) and deriving a UI
    /// scale from `new_host / old_committed` fabricates nonsense (e.g. 0.97x)
    /// and reintroduces double-scaling/accumulated drift.
    #[inline]
    pub fn effective_kwin_scale(&self) -> f64 {
        if let Some(scale) = self.kwin_scale {
            return scale.clamp(1.0, 8.0);
        }
        self.baseline_density_scale()
    }

    /// Update the KWin scale factor.
    /// Returns true if the scale factor changed significantly.
    pub fn update_kwin_scale(&mut self, scale: f64) -> bool {
        if scale <= 0.0 || !scale.is_finite() {
            return false;
        }
        let scale = scale.clamp(1.0, 8.0);
        let changed = match self.kwin_scale {
            Some(prev) => (prev - scale).abs() > 0.001,
            None => true,
        };
        if changed {
            self.kwin_scale = Some(scale);
        }
        changed
    }

    /// Authoritative logical output geometry for the COMMITTED frame.
    ///
    /// Steady state: `round(host / plasma_scale)`.
    /// Transitional (host resized, KWin not yet recommitted): still the OLD
    /// desktop geometry — never pretend the old frame already has new size.
    /// With no committed frame yet (startup): `host / plasma`.
    #[inline]
    pub fn logical_geometry(&self) -> (f64, f64) {
        if self.observed_surface_size.is_some() {
            return self.guest_logical_size;
        }
        self.fresh_logical_geometry()
    }

    /// Physical size in millimeters for wl_output
    pub fn physical_size_mm(&self) -> (i32, i32) {
        let density = self.density_dpi as f64;
        let w_mm = (self.physical_size.0 as f64 * 25.4 / density)
            .round()
            .max(1.0) as i32;
        let h_mm = (self.physical_size.1 as f64 * 25.4 / density)
            .round()
            .max(1.0) as i32;
        (w_mm, h_mm)
    }

    /// Effective presentation scale (W_phys / W_surf, H_phys / H_surf).
    /// Used ONLY for render presentation scaling of the committed buffer texture.
    /// Kept for backwards compatibility; new code must use
    /// [`Self::uniform_presentation_scale`] (single aspect-preserving scale,
    /// no anisotropic stretch, allows <1 for downscaling).
    pub fn presentation_scale(&self) -> (f64, f64) {
        if let Some((surf_w, surf_h)) = self.observed_surface_size {
            if surf_w > 0.0 && surf_h > 0.0 && surf_w.is_finite() && surf_h.is_finite() {
                let sx = self.physical_size.0 as f64 / surf_w;
                let sy = self.physical_size.1 as f64 / surf_h;
                if sx > 0.0 && sy > 0.0 && sx.is_finite() && sy.is_finite() {
                    return (sx, sy);
                }
            }
        }
        (1.0, 1.0)
    }

    /// Single uniform aspect-preserving presentation scale:
    /// `min(host_w/guest_w, host_h/guest_h)`.
    ///
    /// Allows values below 1 so an old large KWin frame can downscale into a
    /// smaller popup. Never anisotropically stretches merely to fill an
    /// intermediate window. With no committed frame yet, returns the effective
    /// Plasma scale (cursor stays consistently sized); rendering with no
    /// texture ignores the value.
    pub fn uniform_presentation_scale(&self) -> f64 {
        if let Some(committed) = self.observed_surface_size {
            if let Some(scale) =
                crate::core::presentation::uniform_fit_scale(self.physical_size, committed)
            {
                return scale;
            }
        }
        self.effective_kwin_scale()
    }

    /// Centered FIT viewport for the committed frame inside the host.
    /// Returns `(origin, size)`. Fullscreen host when no committed frame yet.
    pub fn presentation_viewport(&self) -> ((f64, f64), (f64, f64)) {
        if let Some(committed) = self.observed_surface_size {
            if let Some(vp) = crate::core::presentation::fit_viewport(self.physical_size, committed)
            {
                return (vp.origin, vp.size);
            }
        }
        (
            (0.0, 0.0),
            (self.physical_size.0 as f64, self.physical_size.1 as f64),
        )
    }

    /// Single coherent immutable geometry snapshot for one operation.
    /// Rendering, cursor, mouse and touch for that operation must all derive
    /// from the returned value — never mix `window.inner_size()` in one
    /// subsystem with cached state from another generation elsewhere.
    pub fn presentation_snapshot(&self) -> crate::core::presentation::PresentationSnapshot {
        let (origin, size) = self.presentation_viewport();
        let uniform = self.uniform_presentation_scale();
        let (guest_w, guest_h) = self.logical_geometry();
        // Converged (no letterbox) when a committed frame exists and the
        // viewport fills the host within rounding. `converged` state is
        // authoritative (updated in `note_kwin_commit`/`try_update_physical_size`
        // with size-aware matching, not just aspect) — the geometric fill check
        // here is a consistent derived view for the snapshot.
        let host_w = self.physical_size.0 as f64;
        let host_h = self.physical_size.1 as f64;
        let fills = self.observed_surface_size.is_none()
            || ((size.0 - host_w).abs() <= 2.5 && (size.1 - host_h).abs() <= 2.5);
        crate::core::presentation::PresentationSnapshot {
            generation: self.resize_generation,
            host: self.physical_size,
            guest_logical: (guest_w, guest_h),
            committed: self.observed_surface_size,
            plasma_scale: self.effective_kwin_scale(),
            uniform_scale: uniform,
            viewport_origin: origin,
            viewport_size: size,
            converged: self.converged && fills,
        }
    }

    /// Derive the authoritative CoordinateTransform between physical Android pixels
    /// and KWin logical output coordinates.
    ///
    /// Transitional frames map the CENTERED VIEWPORT (not the full host) to the
    /// committed logical desktop: aspect-preserving FIT, never stretch. Border
    /// rejection itself lives in [`crate::core::presentation::PresentationSnapshot::physical_to_logical`]
    /// (transform clamps; snapshot returns `None` for letterbox input).
    pub fn coordinate_transform(&self) -> CoordinateTransform {
        let snap = self.presentation_snapshot();
        CoordinateTransform::new(
            PhysicalRect {
                x: snap.viewport_origin.0,
                y: snap.viewport_origin.1,
                width: snap.viewport_size.0,
                height: snap.viewport_size.1,
            },
            LogicalRect {
                x: 0.0,
                y: 0.0,
                width: snap.guest_logical.0,
                height: snap.guest_logical.1,
            },
            self.transform,
        )
        .expect("valid display state coordinate transform")
    }

    /// Update observed surface size from committed KWin surface.
    /// Returns true if the size changed significantly. Preserves invalid input.
    /// Prefer [`Self::note_kwin_commit`] (size-aware convergence + guest
    /// preservation) for resize-robust paths; this only records the texture.
    pub fn update_observed_surface_size(&mut self, size: (f64, f64)) -> bool {
        if size.0 <= 0.0 || size.1 <= 0.0 || !size.0.is_finite() || !size.1.is_finite() {
            return false;
        }
        let changed = match self.observed_surface_size {
            Some(prev) => (prev.0 - size.0).abs() > 0.01 || (prev.1 - size.1).abs() > 0.01,
            None => true,
        };
        if changed {
            self.observed_surface_size = Some(size);
        }
        changed
    }

    /// Record a KWin commit with size-aware convergence.
    ///
    /// - `surface_size`: viewport dst when KWin uses viewporter (fractional
    ///   path, expected `host/plasma`), else `None`.
    /// - `buffer_size`: logical buffer size fallback (integer `buffer_scale`
    ///   path, expected `host/buffer_scale`), else `None`.
    /// - `buffer_scale`: integer `wl_surface` buffer scale when known.
    ///
    /// Updates `observed_surface_size` (preferring surface over buffer),
    /// then atomically transitions `guest_logical_size`/`converged` ONLY when
    /// the commit matches the CURRENT host. Stale commits (old geometry
    /// arriving after a newer host resize) update the texture record but must
    /// NOT overwrite the guest logical or fake convergence. Re-evaluates even
    /// when the size is unchanged so a Plasma-scale change with identical
    /// buffer dimensions still converges on the next new commit.
    /// Returns true when committed/guest/converged state changed.
    pub fn note_kwin_commit(
        &mut self,
        surface_size: Option<(f64, f64)>,
        buffer_size: Option<(f64, f64)>,
        buffer_scale: Option<i32>,
    ) -> bool {
        fn valid(size: Option<(f64, f64)>) -> Option<(f64, f64)> {
            match size {
                Some((w, h)) if w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite() => {
                    Some((w, h))
                }
                _ => None,
            }
        }
        let surface = valid(surface_size);
        let buffer = valid(buffer_size);
        let Some(committed) = surface.or(buffer) else {
            return false;
        };
        let size_changed = match self.observed_surface_size {
            Some(prev) => {
                (prev.0 - committed.0).abs() > 0.01 || (prev.1 - committed.1).abs() > 0.01
            }
            None => true,
        };
        if size_changed {
            self.observed_surface_size = Some(committed);
        }

        let host = self.physical_size;
        let plasma = self.effective_kwin_scale();
        let mut now_converged = false;
        if let Some(s) = surface {
            let exp_w = (host.0 as f64 / plasma).round();
            let exp_h = (host.1 as f64 / plasma).round();
            if (s.0 - exp_w).abs() <= 2.0 && (s.1 - exp_h).abs() <= 2.0 {
                now_converged = true;
            }
        } else if let (Some(b), Some(scale)) = (buffer, buffer_scale) {
            if (1..=4).contains(&scale) {
                let exp_w = host.0 as f64 / scale as f64;
                let exp_h = host.1 as f64 / scale as f64;
                if (b.0 - exp_w).abs() <= 2.5 && (b.1 - exp_h).abs() <= 2.5 {
                    now_converged = true;
                }
            }
        } else {
            // No size hint to prove convergence (should not happen in
            // production — redraw always provides buffer_scale). Preserve guest.
            return size_changed;
        }

        if now_converged {
            let new_guest = self.fresh_logical_geometry();
            let guest_changed = (self.guest_logical_size.0 - new_guest.0).abs() > 0.01
                || (self.guest_logical_size.1 - new_guest.1).abs() > 0.01
                || !self.converged;
            if guest_changed || size_changed {
                self.guest_logical_size = new_guest;
                self.converged = true;
                return true;
            }
            return false;
        }

        // Transitional stale frame: keep OLD guest, mark unconverged.
        if size_changed || self.converged {
            self.converged = false;
            return true;
        }
        false
    }

    /// Update physical surface dimensions on resize or orientation change.
    ///
    /// - Zero/invalid dimensions mean the Android surface is temporarily
    ///   unavailable: preserve last valid state and return `false` (callers
    ///   must skip EGL/output/configure updates and resume on next valid).
    /// - Identical sizes coalesce (no new generation, return `false`) so
    ///   resize storms converge on the newest valid size without requiring the
    ///   guest to process every obsolete intermediate.
    /// - New valid sizes bump `resize_generation`, update host + requested
    ///   configure, preserve the OLD guest logical until the matching KWin
    ///   commit arrives (transitional FIT letterboxes, never stretches).
    /// Returns true when host/generation actually changed.
    pub fn update_physical_size(&mut self, w: i32, h: i32) -> bool {
        self.try_update_physical_size(w, h).is_some()
    }

    /// Validating resize entry point with generation reporting.
    /// Returns `Some(new_generation)` on a real change, `None` when
    /// invalid/unchanged (preserved/coalesced).
    pub fn try_update_physical_size(&mut self, w: i32, h: i32) -> Option<u64> {
        if w <= 0 || h <= 0 {
            return None;
        }
        if self.physical_size == (w, h) {
            return None;
        }
        self.resize_generation = self.resize_generation.wrapping_add(1);
        self.physical_size = (w, h);
        self.requested_configure = (w, h);
        self.requested_generation = self.resize_generation;
        if self.observed_surface_size.is_none() {
            // No frame yet: guest tracks the fresh host directly.
            self.guest_logical_size = self.fresh_logical_geometry();
            self.converged = true;
        } else {
            // Transitional: keep OLD guest until matching commit converges.
            self.converged = false;
        }
        Some(self.resize_generation)
    }

    /// Update display density DPI (preserves on invalid, returns changed).
    pub fn update_density_dpi(&mut self, density_dpi: i32) -> bool {
        if density_dpi <= 0 {
            return false;
        }
        if self.density_dpi == density_dpi {
            return false;
        }
        self.density_dpi = density_dpi;
        // Density never rewrites the user's Plasma scale or the committed
        // guest: with no frame yet the fresh guest follows on next snapshot.
        if self.observed_surface_size.is_none() {
            self.guest_logical_size = self.fresh_logical_geometry();
        }
        true
    }
}

/// Parse the active output scale factor from KWin's output configuration JSON
/// (`kwinoutputconfig.json`).
pub fn parse_kwin_scale_from_json(content: &str) -> Option<f64> {
    let json: serde_json::Value = serde_json::from_str(content).ok()?;
    let outputs = json.as_array()?;
    for entry in outputs {
        if entry.get("name").and_then(|n| n.as_str()) == Some("outputs") {
            if let Some(data) = entry.get("data").and_then(|d| d.as_array()) {
                for output in data {
                    if let Some(scale) = output.get("scale").and_then(|s| s.as_f64()) {
                        if scale > 0.0 && scale.is_finite() {
                            return Some(scale);
                        }
                    }
                }
            }
        }
    }
    None
}
