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
}

impl AuthoritativeDisplayState {
    pub fn new(
        physical_w: i32,
        physical_h: i32,
        density_dpi: i32,
        refresh_rate_millihz: i32,
    ) -> Self {
        Self {
            physical_size: (physical_w.max(1), physical_h.max(1)),
            density_dpi: density_dpi.max(1),
            refresh_rate_millihz: refresh_rate_millihz.max(1000),
            transform: ViewportTransform::Normal,
            observed_surface_size: None,
            kwin_scale: None,
        }
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
    /// 2. Observed surface scale (fallback before KWin config is read)
    /// 3. Baseline density scale from Android DPI
    #[inline]
    pub fn effective_kwin_scale(&self) -> f64 {
        if let Some(scale) = self.kwin_scale {
            return scale.clamp(1.0, 8.0);
        }
        if let Some((surf_w, surf_h)) = self.observed_surface_size {
            if surf_w > 0.0 && surf_h > 0.0 && surf_w.is_finite() && surf_h.is_finite() {
                let sx = self.physical_size.0 as f64 / surf_w;
                if sx > 0.0 && sx.is_finite() {
                    return sx.clamp(1.0, 8.0);
                }
            }
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

    /// Authoritative logical output geometry expected by KWin.
    /// Invariant: derived from physical size and effective KWin scale:
    /// W_logical = round(W_phys / scale)
    /// H_logical = round(H_phys / scale)
    #[inline]
    pub fn logical_geometry(&self) -> (f64, f64) {
        let scale = self.effective_kwin_scale();
        let w = (self.physical_size.0 as f64 / scale).round().max(1.0);
        let h = (self.physical_size.1 as f64 / scale).round().max(1.0);
        (w, h)
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

    /// Derive the authoritative CoordinateTransform between physical Android pixels
    /// and KWin logical output coordinates.
    ///
    /// Invariant: logical_source is derived from KWin's logical geometry (physical_size / kwin_scale),
    /// NEVER from buffer-scale surface dimensions.
    pub fn coordinate_transform(&self) -> CoordinateTransform {
        let (log_w, log_h) = self.logical_geometry();
        CoordinateTransform::new(
            PhysicalRect {
                x: 0.0,
                y: 0.0,
                width: self.physical_size.0 as f64,
                height: self.physical_size.1 as f64,
            },
            LogicalRect {
                x: 0.0,
                y: 0.0,
                width: log_w,
                height: log_h,
            },
            self.transform,
        )
        .expect("valid display state coordinate transform")
    }

    /// Update observed surface size from committed KWin surface.
    /// Returns true if the size changed significantly.
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

    /// Update physical surface dimensions on resize or orientation change.
    pub fn update_physical_size(&mut self, w: i32, h: i32) {
        self.physical_size = (w.max(1), h.max(1));
    }

    /// Update display density DPI.
    pub fn update_density_dpi(&mut self, density_dpi: i32) {
        self.density_dpi = density_dpi.max(1);
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
