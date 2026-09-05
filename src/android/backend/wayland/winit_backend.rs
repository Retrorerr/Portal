//! Implementation of backend traits for types provided by `winit`
//!
//! This module provides the appropriate implementations of the backend
//! interfaces for running a compositor as a Wayland or X11 client using [`winit`].
//!
//! ## Usage
//!
//! The backend is initialized using one of the [`init`], [`init_from_attributes`] or
//! [`init_from_attributes_with_gl_attr`] functions, depending on the amount of control
//! you want on the initialization of the backend. These functions will provide you
//! with two objects:
//!
//! - a [`WinitGraphicsBackend`], which can give you an implementation of a [`Renderer`]
//!   (or even [`GlesRenderer`]) through its `renderer` method in addition to further
//!   functionality to access and manage the created winit-window.
//! - a [`WinitEventLoop`], which dispatches some [`WinitEvent`] from the host graphics server.
//!
//! The other types in this module are the instances of the associated types of these
//! two traits for the winit backend.

use khronos_egl::DynamicInstance;
use smithay::{
    backend::{
        egl::{
            context::{GlAttributes, PixelFormatRequirements},
            display::EGLDisplay,
            native::EGLNativeSurface,
            EGLContext, EGLSurface, Error as EGLError,
        },
        renderer::{
            gles::{GlesError, GlesRenderer},
            Bind,
        },
        SwapBuffersError,
    },
    utils::{Physical, Rectangle, Size},
};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::Arc;
use winit::event_loop::ActiveEventLoop;
use winit::raw_window_handle::{AndroidNdkWindowHandle, HasWindowHandle, RawWindowHandle};
use winit::window::{Window as WinitWindow, WindowAttributes};

type RawEglDisplay = smithay::backend::egl::ffi::egl::types::EGLDisplay;
type RawEglSurface = smithay::backend::egl::ffi::egl::types::EGLSurface;
type RawEglBoolean = smithay::backend::egl::ffi::egl::types::EGLBoolean;

const EGL_TRUE: RawEglBoolean = 1;
const EGL_TIMESTAMPS_ANDROID: i32 = 0x3430;
const EGL_DISPLAY_PRESENT_TIME_ANDROID: i32 = 0x343A;
const EGL_TIMESTAMP_PENDING_ANDROID: i64 = -2;
const EGL_TIMESTAMP_INVALID_ANDROID: i64 = -1;
const FRAME_TIMESTAMP_EXTENSION: &str = "EGL_ANDROID_get_frame_timestamps";
const MAX_PENDING_FRAME_TIMESTAMPS: usize = 8;

type EglGetNextFrameId =
    unsafe extern "system" fn(RawEglDisplay, RawEglSurface, *mut u64) -> RawEglBoolean;
type EglGetFrameTimestamps = unsafe extern "system" fn(
    RawEglDisplay,
    RawEglSurface,
    u64,
    i32,
    *const i32,
    *mut i64,
) -> RawEglBoolean;
type EglGetFrameTimestampSupported =
    unsafe extern "system" fn(RawEglDisplay, RawEglSurface, i32) -> RawEglBoolean;

/// Whether Android's physical display-present timestamp can be queried for
/// the active EGL window surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AndroidFrameTimestampSupport {
    /// The EGL probe has not run yet. It runs while the render context is
    /// current, immediately before the first swap.
    Unknown,
    /// The extension is absent or the surface does not support the requested
    /// timestamp. Readiness may use only the explicitly labelled fallback.
    Unsupported,
    /// Timestamp collection was enabled and the display-present query is
    /// available for this surface.
    Available,
}

/// A physical display-present sample correlated with one EGL swap frame id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AndroidFrameTimestampSample {
    pub frame_id: u64,
    pub timestamp_ns: i64,
}

#[derive(Debug)]
struct AndroidFrameTimestampProbe {
    support: AndroidFrameTimestampSupport,
    get_next_frame_id: Option<EglGetNextFrameId>,
    get_frame_timestamps: Option<EglGetFrameTimestamps>,
    get_frame_timestamp_supported: Option<EglGetFrameTimestampSupported>,
    enabled_surface: Option<RawEglSurface>,
    pending_surface: Option<RawEglSurface>,
    pending_frame_ids: VecDeque<u64>,
}

impl Default for AndroidFrameTimestampProbe {
    fn default() -> Self {
        Self {
            support: AndroidFrameTimestampSupport::Unknown,
            get_next_frame_id: None,
            get_frame_timestamps: None,
            get_frame_timestamp_supported: None,
            enabled_surface: None,
            pending_surface: None,
            pending_frame_ids: VecDeque::new(),
        }
    }
}

impl AndroidFrameTimestampProbe {
    fn load_next_frame_id() -> Option<EglGetNextFrameId> {
        let address =
            unsafe { smithay::backend::egl::get_proc_address("eglGetNextFrameIdANDROID") };
        (!address.is_null()).then(|| unsafe { std::mem::transmute(address) })
    }

    fn load_frame_timestamps() -> Option<EglGetFrameTimestamps> {
        let address =
            unsafe { smithay::backend::egl::get_proc_address("eglGetFrameTimestampsANDROID") };
        (!address.is_null()).then(|| unsafe { std::mem::transmute(address) })
    }

    fn load_frame_timestamp_supported() -> Option<EglGetFrameTimestampSupported> {
        let address = unsafe {
            smithay::backend::egl::get_proc_address("eglGetFrameTimestampSupportedANDROID")
        };
        (!address.is_null()).then(|| unsafe { std::mem::transmute(address) })
    }

    fn raw_display(display: &EGLDisplay) -> RawEglDisplay {
        display.get_display_handle().handle
    }

    fn enable_for_surface(
        &mut self,
        display: &EGLDisplay,
        raw_display: RawEglDisplay,
        raw_surface: RawEglSurface,
    ) -> bool {
        let Some(supported) = self.get_frame_timestamp_supported else {
            return false;
        };

        let supported =
            unsafe { supported(raw_display, raw_surface, EGL_DISPLAY_PRESENT_TIME_ANDROID) }
                == EGL_TRUE;
        if !supported {
            log::warn!(
                "EGL_ANDROID_get_frame_timestamps is present but EGL_DISPLAY_PRESENT_TIME_ANDROID is unsupported"
            );
            self.support = AndroidFrameTimestampSupport::Unsupported;
            self.enabled_surface = None;
            return false;
        }

        // The extension specification defaults timestamp collection to
        // disabled. Enable it before asking for the first frame id.
        let enabled = unsafe {
            smithay::backend::egl::ffi::egl::SurfaceAttrib(
                raw_display,
                raw_surface,
                EGL_TIMESTAMPS_ANDROID,
                EGL_TRUE as i32,
            )
        } == EGL_TRUE;
        if !enabled {
            log::warn!("Failed to enable EGL timestamp collection on the Android window surface");
            self.support = AndroidFrameTimestampSupport::Unsupported;
            self.enabled_surface = None;
            return false;
        }

        self.enabled_surface = Some(raw_surface);
        self.pending_surface = Some(raw_surface);
        self.support = AndroidFrameTimestampSupport::Available;
        log::info!(
            "EGL Android frame timestamps enabled for display={raw_display:?} surface={raw_surface:?}"
        );
        let _ = display;
        true
    }

    fn initialise(&mut self, display: &EGLDisplay, raw_surface: RawEglSurface) {
        if !matches!(self.support, AndroidFrameTimestampSupport::Unknown) {
            return;
        }

        if !display
            .extensions()
            .iter()
            .any(|extension| extension == FRAME_TIMESTAMP_EXTENSION)
        {
            log::info!(
                "Android EGL frame timestamps unavailable: {FRAME_TIMESTAMP_EXTENSION} not advertised"
            );
            self.support = AndroidFrameTimestampSupport::Unsupported;
            return;
        }

        self.get_next_frame_id = Self::load_next_frame_id();
        self.get_frame_timestamps = Self::load_frame_timestamps();
        self.get_frame_timestamp_supported = Self::load_frame_timestamp_supported();
        if self.get_next_frame_id.is_none()
            || self.get_frame_timestamps.is_none()
            || self.get_frame_timestamp_supported.is_none()
        {
            log::warn!(
                "Android EGL frame timestamps advertised but one or more entry points are missing"
            );
            self.support = AndroidFrameTimestampSupport::Unsupported;
            return;
        }

        let raw_display = Self::raw_display(display);
        self.enable_for_surface(display, raw_display, raw_surface);
    }

    fn ensure_surface_enabled(&mut self, display: &EGLDisplay, raw_surface: RawEglSurface) -> bool {
        if !matches!(self.support, AndroidFrameTimestampSupport::Available) {
            return false;
        }
        if self.enabled_surface == Some(raw_surface) {
            return true;
        }
        let raw_display = Self::raw_display(display);
        self.enable_for_surface(display, raw_display, raw_surface)
    }

    fn before_swap(&mut self, display: &EGLDisplay, surface: &EGLSurface) -> Option<u64> {
        let raw_surface = surface.get_surface_handle();
        if raw_surface.is_null() {
            return None;
        }
        self.initialise(display, raw_surface);
        if !self.ensure_surface_enabled(display, raw_surface) {
            return None;
        }

        let get_next_frame_id = self.get_next_frame_id?;
        let mut frame_id = 0;
        let raw_display = Self::raw_display(display);
        let success =
            unsafe { get_next_frame_id(raw_display, raw_surface, &mut frame_id) } == EGL_TRUE;
        if success {
            Some(frame_id)
        } else {
            log::warn!("eglGetNextFrameIdANDROID failed for the Android window surface");
            None
        }
    }

    fn after_swap(
        &mut self,
        frame_id: Option<u64>,
        surface_before: RawEglSurface,
        surface_after: RawEglSurface,
    ) -> Option<u64> {
        let Some(frame_id) = frame_id else {
            return None;
        };
        if surface_before.is_null() || surface_before != surface_after {
            // Smithay can recreate the EGLSurface after a bad-surface swap.
            // The frame id belongs to the old surface and must not be queried
            // against the replacement.
            self.pending_frame_ids.clear();
            self.pending_surface = None;
            self.enabled_surface = None;
            log::warn!("EGL surface changed during swap; dropping frame timestamp id={frame_id}");
            return None;
        }
        self.pending_surface = Some(surface_after);
        self.pending_frame_ids.push_back(frame_id);
        while self.pending_frame_ids.len() > MAX_PENDING_FRAME_TIMESTAMPS {
            self.pending_frame_ids.pop_front();
        }
        Some(frame_id)
    }

    fn poll(
        &mut self,
        display: &EGLDisplay,
        surface: &EGLSurface,
    ) -> Vec<AndroidFrameTimestampSample> {
        if !matches!(self.support, AndroidFrameTimestampSupport::Available) {
            return Vec::new();
        }
        let raw_surface = surface.get_surface_handle();
        if raw_surface.is_null() || self.pending_surface != Some(raw_surface) {
            self.pending_frame_ids.clear();
            self.pending_surface = None;
            return Vec::new();
        }
        let Some(get_frame_timestamps) = self.get_frame_timestamps else {
            return Vec::new();
        };
        let raw_display = Self::raw_display(display);
        let timestamp_name = [EGL_DISPLAY_PRESENT_TIME_ANDROID];
        let mut remaining = VecDeque::new();
        let mut presented = Vec::new();
        while let Some(frame_id) = self.pending_frame_ids.pop_front() {
            let mut value = 0_i64;
            let success = unsafe {
                get_frame_timestamps(
                    raw_display,
                    raw_surface,
                    frame_id,
                    1,
                    timestamp_name.as_ptr(),
                    &mut value,
                )
            } == EGL_TRUE;
            if !success {
                // The implementation is allowed to retire old history. Do
                // not keep retrying an id that can no longer be queried.
                log::debug!("eglGetFrameTimestampsANDROID failed for frame id={frame_id}");
                continue;
            }
            match value {
                EGL_TIMESTAMP_PENDING_ANDROID => remaining.push_back(frame_id),
                EGL_TIMESTAMP_INVALID_ANDROID => {
                    log::debug!("Android EGL frame id={frame_id} has no display-present timestamp")
                }
                timestamp_ns if timestamp_ns >= 0 => {
                    presented.push(AndroidFrameTimestampSample {
                        frame_id,
                        timestamp_ns,
                    });
                }
                _ => log::debug!(
                    "Android EGL frame id={frame_id} returned unexpected display timestamp={value}"
                ),
            }
        }
        self.pending_frame_ids = remaining;
        presented
    }
}

#[derive(Clone, Copy, Debug)]
struct ContextCandidate {
    label: &'static str,
    attributes: GlAttributes,
    pixel_format: PixelFormatRequirements,
}

fn create_egl_context(display: &EGLDisplay) -> Result<EGLContext, String> {
    let candidates = [
        ContextCandidate {
            label: "OpenGL ES 3.0 with 10-bit hardware-accelerated surface",
            attributes: GlAttributes {
                version: (3, 0),
                profile: None,
                debug: cfg!(debug_assertions),
                vsync: true,
            },
            pixel_format: PixelFormatRequirements::_10_bit(),
        },
        ContextCandidate {
            label: "OpenGL ES 3.0 with 8-bit hardware-accelerated surface",
            attributes: GlAttributes {
                version: (3, 0),
                profile: None,
                debug: cfg!(debug_assertions),
                vsync: true,
            },
            pixel_format: PixelFormatRequirements::_8_bit(),
        },
        ContextCandidate {
            label: "OpenGL ES 3.0 with 8-bit emulator-friendly surface",
            attributes: GlAttributes {
                version: (3, 0),
                profile: None,
                debug: cfg!(debug_assertions),
                vsync: true,
            },
            pixel_format: PixelFormatRequirements {
                hardware_accelerated: None,
                color_bits: Some(24),
                float_color_buffer: false,
                alpha_bits: Some(8),
                depth_bits: Some(24),
                stencil_bits: Some(8),
                multisampling: None,
            },
        },
        ContextCandidate {
            label: "OpenGL ES 2.0 with 8-bit emulator-friendly surface",
            attributes: GlAttributes {
                version: (2, 0),
                profile: None,
                debug: cfg!(debug_assertions),
                vsync: true,
            },
            pixel_format: PixelFormatRequirements {
                hardware_accelerated: None,
                color_bits: Some(24),
                float_color_buffer: false,
                alpha_bits: Some(8),
                depth_bits: Some(24),
                stencil_bits: Some(8),
                multisampling: None,
            },
        },
    ];
    let mut errors = Vec::with_capacity(candidates.len());

    for candidate in candidates {
        match EGLContext::new_with_config(display, candidate.attributes, candidate.pixel_format) {
            Ok(context) => {
                if !errors.is_empty() {
                    log::warn!(
                        "Using EGL fallback after {} failed attempt(s): {}",
                        errors.len(),
                        candidate.label
                    );
                }
                return Ok(context);
            }
            Err(error) => {
                log::warn!("Failed EGL candidate '{}': {}", candidate.label, error);
                errors.push(format!("{}: {}", candidate.label, error));
            }
        }
    }

    Err(format!(
        "Failed to create EGLContext. Tried: {}",
        errors.join(" | ")
    ))
}

pub struct AndroidNativeSurface {
    handle: AndroidNdkWindowHandle,
}

unsafe impl Send for AndroidNativeSurface {}

unsafe impl EGLNativeSurface for AndroidNativeSurface {
    unsafe fn create(
        &self,
        display: &Arc<smithay::backend::egl::display::EGLDisplayHandle>,
        config_id: smithay::backend::egl::ffi::egl::types::EGLConfig,
    ) -> Result<*const std::os::raw::c_void, smithay::backend::egl::EGLError> {
        let surface = smithay::backend::egl::ffi::egl::CreateWindowSurface(
            display.handle,
            config_id,
            self.handle.a_native_window.as_ptr(),
            std::ptr::null(),
        );
        if surface.is_null() {
            return Err(smithay::backend::egl::EGLError::BadSurface);
        }
        Ok(surface)
    }
}

fn create_egl_display(
    _handle: AndroidNdkWindowHandle,
) -> Result<EGLDisplay, Box<dyn std::error::Error>> {
    // Load the EGL library
    let lib = unsafe { libloading::Library::new("libEGL.so") }?;
    let egl = unsafe { DynamicInstance::<khronos_egl::EGL1_4>::load_required_from(lib) }?;

    // Get the display
    let display = unsafe { egl.get_display(khronos_egl::DEFAULT_DISPLAY) }
        .expect("Failed to get EGL display");

    // Initialize the display
    let (_major, _minor) = egl.initialize(display)?;

    // Choose an EGL configuration
    let config_attribs = [khronos_egl::NONE];
    let config = egl
        .choose_first_config(display, &config_attribs)
        .expect("Failed to choose EGL config")
        .expect("No suitable EGL config found");

    // Create the EGLDisplay from raw pointers
    let egl_display = unsafe {
        EGLDisplay::from_raw(
            display.as_ptr() as *mut c_void,
            config.as_ptr() as *mut c_void,
        )
    }
    .expect("Failed to create EGL display");

    Ok(egl_display)
}

/// Create a new [`WinitGraphicsBackend`], which implements the [`Renderer`]
/// trait, from a given [`WindowAttributes`] struct, as well as given
/// [`GlAttributes`] for further customization of the rendering pipeline and a
/// corresponding [`WinitEventLoop`].
pub fn bind(event_loop: &ActiveEventLoop) -> Result<WinitGraphicsBackend<GlesRenderer>, String> {
    #[allow(deprecated)]
    let window = Arc::new(
        event_loop
            .create_window(WindowAttributes::default())
            .map_err(|error| format!("Failed to create window: {error}"))?,
    );

    let handle = window
        .window_handle()
        .map(|handle| handle.as_raw())
        .map_err(|error| format!("Failed to get window handle: {error}"))?;
    let (display, context, surface) = match handle {
        RawWindowHandle::AndroidNdk(handle) => {
            let display = create_egl_display(handle)
                .map_err(|error| format!("Failed to create EGLDisplay: {error:?}"))?;

            let context = create_egl_context(&display)?;
            let pixel_format = context
                .pixel_format()
                .ok_or_else(|| "EGL context did not expose a pixel format".to_string())?;

            let surface = unsafe {
                EGLSurface::new(
                    &display,
                    pixel_format,
                    context.config_id(),
                    AndroidNativeSurface { handle },
                )
                .map_err(|error| format!("Failed to create EGLSurface: {error}"))?
            };

            let _ = context.unbind();
            (display, context, surface)
        }
        platform => return Err(format!("Unsupported platform: {:?}", platform)),
    };

    let renderer = unsafe { GlesRenderer::new(context) }
        .map_err(|error| format!("Failed to create GLES Renderer: {error}"))?;
    let damage_tracking = display.supports_damage();

    event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);

    Ok(WinitGraphicsBackend {
        window: window.clone(),
        _display: display,
        egl_surface: surface,
        damage_tracking,
        bind_size: None,
        renderer,
        frame_timestamps: AndroidFrameTimestampProbe::default(),
    })
}

/// Errors thrown by the `winit` backends
#[derive(Debug)]
pub enum Error {
    /// Failed to initialize an event loop.
    EventLoopCreation(winit::error::EventLoopError),
    /// Failed to initialize a window.
    WindowCreation(winit::error::OsError),
    /// Surface creation error.
    Surface(Box<dyn std::error::Error>),
    /// Context creation is not supported on the current window system
    NotSupported,
    /// EGL error.
    Egl(EGLError),
    /// Renderer initialization failed.
    RendererCreationError(GlesError),
}

/// Window with an active EGL Context created by `winit`.
#[derive(Debug)]
pub struct WinitGraphicsBackend<R> {
    renderer: R,
    // The display isn't used past this point but must be kept alive.
    _display: EGLDisplay,
    egl_surface: EGLSurface,
    frame_timestamps: AndroidFrameTimestampProbe,
    window: Arc<WinitWindow>,
    damage_tracking: bool,
    bind_size: Option<Size<i32, Physical>>,
}

impl<R> WinitGraphicsBackend<R>
where
    R: Bind<EGLSurface>,
    SwapBuffersError: From<R::Error>,
{
    /// Window size of the underlying window
    pub fn window_size(&self) -> Size<i32, Physical> {
        let (w, h): (i32, i32) = self.window.inner_size().into();
        (w, h).into()
    }

    /// Scale factor of the underlying window.
    pub fn scale_factor(&self) -> f64 {
        self.window.scale_factor()
    }

    /// Reference to the underlying window
    pub fn window(&self) -> &WinitWindow {
        &self.window
    }

    /// Access the underlying renderer
    pub fn renderer(&mut self) -> &mut R {
        &mut self.renderer
    }

    /// Bind the underlying window to the underlying renderer.
    pub fn bind(&mut self) -> Result<(&mut R, R::Framebuffer<'_>), SwapBuffersError> {
        // NOTE: we must resize before making the current context current, otherwise the back
        // buffer will be latched. Some nvidia drivers may not like it, but a lot of wayland
        // software does the order that way due to mesa latching back buffer on each
        // `make_current`.
        let window_size = self.window_size();
        // Zero/invalid surfaces occur during Android multi-window/lifecycle
        // transitions: preserve last valid EGL size instead of resizing to 0
        // (which would poison EGL/Smithay/KWin geometry agreement).
        if window_size.w <= 0 || window_size.h <= 0 {
            if let Some(valid) = self.bind_size {
                log::debug!(
                    "WinitGraphicsBackend::bind: ignoring invalid window {}x{}, preserving EGL {:?}",
                    window_size.w,
                    window_size.h,
                    valid,
                );
            }
        } else {
            if Some(window_size) != self.bind_size {
                self.egl_surface.resize(window_size.w, window_size.h, 0, 0);
            }
            self.bind_size = Some(window_size);
        }

        let fb = self.renderer.bind(&mut self.egl_surface)?;

        Ok((&mut self.renderer, fb))
    }

    /// Retrieve the underlying `EGLSurface` for advanced operations
    ///
    /// **Note:** Don't carelessly use this to manually bind the renderer to the surface,
    /// `WinitGraphicsBackend::bind` transparently handles window resizes for you.
    pub fn egl_surface(&self) -> &EGLSurface {
        &self.egl_surface
    }

    /// Return whether the Android physical display-present timestamp probe is
    /// available for this window surface.
    pub fn android_frame_timestamp_support(&self) -> AndroidFrameTimestampSupport {
        self.frame_timestamps.support
    }

    /// Poll the Android timestamp associated with a previously submitted EGL
    /// frame. The result is independent of Wayland presentation feedback and
    /// is used to gate the host readiness marker.
    pub fn poll_android_frame_timestamps(&mut self) -> Vec<AndroidFrameTimestampSample> {
        self.frame_timestamps
            .poll(&self._display, &self.egl_surface)
    }

    /// Retrieve the buffer age of the current backbuffer of the window.
    ///
    /// This will only return a meaningful value, if this `WinitGraphicsBackend`
    /// is currently bound (by previously calling [`WinitGraphicsBackend::bind`]).
    ///
    /// Otherwise and on error this function returns `None`.
    /// If you are using this value actively e.g. for damage-tracking you should
    /// likely interpret an error just as if "0" was returned.
    pub fn buffer_age(&self) -> Option<usize> {
        if self.damage_tracking {
            self.egl_surface.buffer_age().map(|x| x as usize)
        } else {
            Some(0)
        }
    }

    /// Submits the back buffer to the window by swapping, requires the window to be previously
    /// bound (see [`WinitGraphicsBackend::bind`]).
    pub fn submit(
        &mut self,
        damage: Option<&[Rectangle<i32, Physical>]>,
    ) -> Result<Option<u64>, SwapBuffersError> {
        let mut damage = match damage {
            Some(damage) if self.damage_tracking && !damage.is_empty() => {
                let bind_size = self
                    .bind_size
                    .expect("submitting without ever binding the renderer.");
                let damage = damage
                    .iter()
                    .map(|rect| {
                        Rectangle::new(
                            (rect.loc.x, bind_size.h - rect.loc.y - rect.size.h).into(),
                            rect.size,
                        )
                    })
                    .collect::<Vec<_>>();
                Some(damage)
            }
            _ => None,
        };

        // `eglGetNextFrameIdANDROID` must run immediately before the swap it
        // identifies, while the renderer's EGL context is current.
        let surface_before = self.egl_surface.get_surface_handle();
        let frame_id = self
            .frame_timestamps
            .before_swap(&self._display, &self.egl_surface);

        // Request frame callback.
        self.window.pre_present_notify();
        self.egl_surface.swap_buffers(damage.as_deref_mut())?;
        let surface_after = self.egl_surface.get_surface_handle();
        Ok(self
            .frame_timestamps
            .after_swap(frame_id, surface_before, surface_after))
    }
}
