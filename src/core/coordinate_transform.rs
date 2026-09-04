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
/// - `observed_surface_size` is the committed texture of the CURRENTLY RENDERED
///   KWin frame (live surface, possibly stale relative to the host target).
/// - `guest_logical_size` is the KWin logical desktop matching THAT rendered
///   frame — never the logical of a different generation. A stale frame keeps
///   its own correct logical while still targeting the newest host.
/// - `rendered_host`/`rendered_generation` identify which request the rendered
///   frame was attributed to (size match, newest-first, xdg-ack tie-break).
/// - `resize_generation` increases monotonically; newer resizes never lose to
///   obsolete state. No sleeps/timing guesses.
/// - `kwin_scale` (Plasma UI scale) is stable across resizes, never derived
///   from stale presentation state.
/// One xdg configure sent (or coalesced) for a host request.
///
/// `serial` is the Wayland configure serial captured from `send_configure`
/// (0 when the send site did not capture one). Bounded ring, newest last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigureRequest {
    pub generation: u64,
    pub host: (i32, i32),
    pub serial: u32,
}

/// How many configure requests the history ring retains.
pub const REQUEST_HISTORY_LEN: usize = 16;

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
    /// KWin logical desktop matching the CURRENTLY RENDERED frame.
    /// Updated atomically with `observed_surface_size` in `note_kwin_commit`
    /// from the request the frame was attributed to — never the logical of a
    /// different generation.
    pub guest_logical_size: (f64, f64),
    /// Host size the rendered frame was produced for (owning request).
    pub rendered_host: (i32, i32),
    /// Request generation the rendered frame was attributed to.
    pub rendered_generation: u64,
    /// True when the rendered frame came from a viewporter surface size
    /// (authoritative KWin logical, kept verbatim across Plasma changes).
    pub rendered_from_surface: bool,
    /// Last xdg configure serial acked by KWin (0 = none yet). Tie-breaks
    /// same-dimension history matches toward the acknowledged request.
    pub acked_serial: u32,
    /// Ordered configure-request history, oldest first, newest last.
    /// Only the first `request_len` entries are live.
    pub requests: [ConfigureRequest; REQUEST_HISTORY_LEN],
    /// Number of live entries in `requests` (1..=REQUEST_HISTORY_LEN).
    pub request_len: u8,
    /// Last seen live surface reading (viewport dst when present).
    pub last_surface_size: Option<(f64, f64)>,
    /// Last seen live buffer reading (logical buffer size).
    pub last_buffer_size: Option<(f64, f64)>,
    /// Last seen integer buffer scale.
    pub last_buffer_scale: Option<i32>,
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
        let seed = ConfigureRequest {
            generation: 0,
            host: physical_size,
            serial: 0,
        };
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
            rendered_host: physical_size,
            rendered_generation: 0,
            rendered_from_surface: false,
            acked_serial: 0,
            requests: [seed; REQUEST_HISTORY_LEN],
            request_len: 1,
            last_surface_size: None,
            last_buffer_size: None,
            last_buffer_scale: None,
            kwin_config_mtime_ns: None,
        };
        state.guest_logical_size = state.fresh_logical_geometry();
        state
    }

    /// Live configure-request history, oldest first.
    pub fn request_history(&self) -> &[ConfigureRequest] {
        &self.requests[..self.request_len.min(REQUEST_HISTORY_LEN as u8) as usize]
    }

    fn push_request(&mut self, generation: u64, host: (i32, i32), serial: u32) {
        let entry = ConfigureRequest {
            generation,
            host,
            serial,
        };
        let len = (self.request_len as usize).min(REQUEST_HISTORY_LEN);
        if len < REQUEST_HISTORY_LEN {
            self.requests[len] = entry;
            self.request_len = (len + 1) as u8;
        } else {
            self.requests.copy_within(1.., 0);
            self.requests[REQUEST_HISTORY_LEN - 1] = entry;
        }
    }

    /// Record that an xdg configure was sent for the current request.
    /// Call at every configure send site with the captured serial so commits
    /// can later be attributed to their owning request (serial tie-break) and
    /// diagnostics can show what KWin was asked for.
    pub fn note_configure_sent(&mut self, serial: u32) {
        let gen = self.requested_generation;
        let host = self.requested_configure;
        if let Some(entry) = self.requests[..(self.request_len as usize).min(REQUEST_HISTORY_LEN)]
            .iter_mut()
            .rev()
            .find(|entry| entry.generation == gen)
        {
            entry.serial = serial;
            return;
        }
        self.push_request(gen, host, serial);
    }

    /// Record KWin's ack of a configure serial. Returns the owning request
    /// generation when the serial is found in history.
    pub fn note_configure_acked(&mut self, serial: u32) -> Option<u64> {
        self.acked_serial = serial;
        self.request_history()
            .iter()
            .rev()
            .find(|entry| entry.serial != 0 && entry.serial == serial)
            .map(|entry| entry.generation)
    }

    /// Whether the rendered frame was produced for the current host target.
    /// A stale frame may be rendered (letterboxed) while this is false, but
    /// it always carries its OWN matching logical geometry.
    pub fn rendered_is_current(&self) -> bool {
        self.observed_surface_size.is_none() || self.rendered_host == self.physical_size
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
    ///
    /// A rendered buffer-derived frame keeps its origin host but is
    /// re-paired with that host under the new scale (the live surface is
    /// overwhelmingly likely already new-scale: KWin recommits immediately on
    /// scale changes while our refresh is low-frequency). Surface-derived
    /// frames keep their authoritative KWin logical verbatim. Either way the
    /// rendered pair stays internally consistent; the next matching commit
    /// repairs any residual corner exactly.
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
            if self.observed_surface_size.is_some() && !self.rendered_from_surface {
                let w = (self.rendered_host.0 as f64 / scale).round().max(1.0);
                let h = (self.rendered_host.1 as f64 / scale).round().max(1.0);
                self.guest_logical_size = (w, h);
            }
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
        // Converged (no letterbox) when the rendered frame was produced for
        // the current host target and its viewport fills the host within
        // integer-truncation rounding.
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
            converged: self.rendered_is_current() && fills,
            rendered_generation: self.rendered_generation,
            rendered_host: self.rendered_host,
            requested: self.requested_configure,
            requested_generation: self.requested_generation,
            acked_serial: self.acked_serial,
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

    /// Attribute a commit to a frame produced for `host`, returning the
    /// matching logical geometry and whether it came from a viewporter
    /// surface (authoritative KWin logical) or was derived (buffer path).
    ///
    /// Surface sizes (viewport dst) are authoritative KWin logical when they
    /// equal `round(host/plasma)`. But `surface_size()` ALSO echoes the
    /// integer buffer logical when KWin sets no viewport — observed live on
    /// device at 2.25x (`surf == buf == 1130x800`) — so a surface reading that
    /// instead matches `host/buffer_scale` is a buffer-mirror frame whose
    /// logical must be derived, never used verbatim. Tolerances cover integer
    /// `Size<i32>` truncation.
    fn attribute_frame(
        host: (i32, i32),
        plasma: f64,
        surface: Option<(f64, f64)>,
        buffer: Option<(f64, f64)>,
        buffer_scale: Option<i32>,
    ) -> Option<((f64, f64), bool)> {
        if let Some(s) = surface {
            let exp_w = (host.0 as f64 / plasma).round();
            let exp_h = (host.1 as f64 / plasma).round();
            if (s.0 - exp_w).abs() <= 2.0 && (s.1 - exp_h).abs() <= 2.0 {
                return Some((s, true));
            }
            if let Some(scale) = buffer_scale {
                if (1..=4).contains(&scale) {
                    let bw = host.0 as f64 / scale as f64;
                    let bh = host.1 as f64 / scale as f64;
                    if (s.0 - bw).abs() <= 2.5 && (s.1 - bh).abs() <= 2.5 {
                        let w = (host.0 as f64 / plasma).round().max(1.0);
                        let h = (host.1 as f64 / plasma).round().max(1.0);
                        return Some(((w, h), false));
                    }
                }
            }
            return None;
        }
        if let (Some(b), Some(scale)) = (buffer, buffer_scale) {
            if (1..=4).contains(&scale) {
                let exp_w = host.0 as f64 / scale as f64;
                let exp_h = host.1 as f64 / scale as f64;
                if (b.0 - exp_w).abs() <= 2.5 && (b.1 - exp_h).abs() <= 2.5 {
                    let w = (host.0 as f64 / plasma).round().max(1.0);
                    let h = (host.1 as f64 / plasma).round().max(1.0);
                    return Some(((w, h), false));
                }
            }
        }
        None
    }

    /// Record a KWin commit, attributing it to its owning request.
    ///
    /// - `surface_size`: viewport dst when KWin uses viewporter (fractional
    ///   path); authoritative KWin logical, used verbatim.
    /// - `buffer_size`: logical buffer size fallback (integer `buffer_scale`
    ///   path); paired with `round(origin_host/plasma)`.
    /// - `buffer_scale`: integer `wl_surface` buffer scale when known.
    ///
    /// The commit is matched newest-first against the configure-request
    /// history; same-dimension ambiguities (A→B→A) prefer the entry whose
    /// serial KWin last acked. The rendered texture AND its logical are then
    /// updated atomically from the matched request — a stale intermediate
    /// (B committed while targeting C) renders B with B-era logical while the
    /// newest host target C is never disturbed. Re-evaluates even when the
    /// size is unchanged so same-size recommits still repair attribution.
    /// Returns true when rendered/logical/convergence state changed.
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
        // Record the raw live readings for diagnostics even when the frame
        // proves unattributable below.
        self.last_surface_size = surface;
        self.last_buffer_size = buffer;
        self.last_buffer_scale = buffer_scale;
        let Some(committed) = surface.or(buffer) else {
            return false;
        };
        let plasma = self.effective_kwin_scale();

        // Newest-first history scan; acked serial wins ties.
        let mut best: Option<(u64, (i32, i32), (f64, f64), bool)> = None;
        let mut acked_best: Option<(u64, (i32, i32), (f64, f64), bool)> = None;
        for entry in self.request_history().iter().rev() {
            if let Some((guest, from_surface)) =
                Self::attribute_frame(entry.host, plasma, surface, buffer, buffer_scale)
            {
                if best.is_none() {
                    best = Some((entry.generation, entry.host, guest, from_surface));
                }
                if entry.serial != 0 && entry.serial == self.acked_serial {
                    acked_best = Some((entry.generation, entry.host, guest, from_surface));
                    break;
                }
            }
        }
        // Fall back to the current target itself (covers evicted history and
        // pre-history initial commits); otherwise the frame is unattributable.
        let matched = acked_best.or(best).or_else(|| {
            Self::attribute_frame(self.physical_size, plasma, surface, buffer, buffer_scale).map(
                |(guest, from_surface)| {
                    (
                        self.requested_generation,
                        self.physical_size,
                        guest,
                        from_surface,
                    )
                },
            )
        });

        let (origin_gen, origin_host, guest, from_surface) = match matched {
            Some((gen, host, guest, from_surface)) => (gen, host, guest, from_surface),
            None => {
                // Unattributable frame: still track the LIVE texture (the
                // renderer draws it regardless) paired with the best available
                // logical — buffer-mirror derivation when a scale is known,
                // verbatim surface otherwise — but never claim currency and
                // never touch the newest host target.
                let (guest, from_surface) = if surface.is_some() {
                    match buffer_scale {
                        Some(scale)
                            if (1..=4).contains(&scale) && plasma.is_finite() && plasma > 0.0 =>
                        {
                            let s = surface.expect("surface branch");
                            (
                                (
                                    (s.0 * scale as f64 / plasma).round().max(1.0),
                                    (s.1 * scale as f64 / plasma).round().max(1.0),
                                ),
                                false,
                            )
                        }
                        _ => (surface.expect("surface branch"), true),
                    }
                } else if let (Some(b), Some(scale)) = (buffer, buffer_scale) {
                    if (1..=4).contains(&scale) && plasma.is_finite() && plasma > 0.0 {
                        (
                            (
                                (b.0 * scale as f64 / plasma).round().max(1.0),
                                (b.1 * scale as f64 / plasma).round().max(1.0),
                            ),
                            false,
                        )
                    } else {
                        (self.guest_logical_size, self.rendered_from_surface)
                    }
                } else {
                    (self.guest_logical_size, self.rendered_from_surface)
                };
                (
                    self.rendered_generation,
                    self.rendered_host,
                    guest,
                    from_surface,
                )
            }
        };

        let size_changed = match self.observed_surface_size {
            Some(prev) => {
                (prev.0 - committed.0).abs() > 0.01 || (prev.1 - committed.1).abs() > 0.01
            }
            None => true,
        };
        let guest_changed = (self.guest_logical_size.0 - guest.0).abs() > 0.01
            || (self.guest_logical_size.1 - guest.1).abs() > 0.01;
        let origin_changed =
            self.rendered_generation != origin_gen || self.rendered_host != origin_host;
        let from_surface_changed = self.rendered_from_surface != from_surface;

        // Deliberately no "currency changed" term: currency is derived
        // (`rendered_host == host` plus viewport fill), so folding it into
        // change detection would report phantom changes every frame.
        let changed = size_changed || guest_changed || origin_changed || from_surface_changed;
        if changed {
            self.observed_surface_size = Some(committed);
            self.guest_logical_size = guest;
            self.rendered_host = origin_host;
            self.rendered_generation = origin_gen;
            self.rendered_from_surface = from_surface;
        }
        changed
    }

    /// Update physical surface dimensions on resize or orientation change.
    ///
    /// - Zero/invalid dimensions mean the Android surface is temporarily
    ///   unavailable: preserve last valid state and return `false` (callers
    ///   must skip EGL/output/configure updates and resume on next valid).
    /// - Identical sizes coalesce (no new generation, no history entry, return
    ///   `false`) so resize storms converge on the newest valid size without
    ///   requiring the guest to process every obsolete intermediate, and
    ///   repeated equal-sized configures never corrupt request ownership.
    /// - New valid sizes bump `resize_generation`, update host + requested
    ///   configure, and append to the request history. The rendered frame
    ///   (texture + logical + origin) is untouched: the old frame keeps
    ///   rendering letterboxed with its own correct logical until the matching
    ///   KWin commit is attributed back to it.
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
        self.push_request(self.resize_generation, (w, h), 0);
        if self.observed_surface_size.is_none() {
            // No frame yet: guest tracks the fresh host directly.
            self.guest_logical_size = self.fresh_logical_geometry();
            self.rendered_host = (w, h);
            self.rendered_generation = self.resize_generation;
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
