//! Implementation of the android_wlegl protocol and buffer management.

use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{
    ExternalBuffer, ExternalBufferData, ExternalBufferImportError, Renderer,
};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::{DataInit, Resource};
use smithay::utils::{Buffer as BufferCoord, Rectangle, Size};
use smithay::wayland::compositor::SurfaceData;

use super::gl_import::AhbTextureImporter;
use super::protocol::android_wlegl::server::{
    android_wlegl::{self, AndroidWlegl},
    android_wlegl_handle::{self, AndroidWleglHandle},
};

extern "C" {
    fn tawc_wlegl_import(
        width: u32,
        height: u32,
        stride: u32,
        format: u32,
        usage: u64,
        fds: *const i32,
        num_fds: i32,
        ints: *const i32,
        num_ints: i32,
    ) -> *mut std::ffi::c_void;

    fn tawc_wlegl_buffer_release(ahb: *mut std::ffi::c_void);
}

pub static WLEGL_BUFFERS_CREATED: AtomicU32 = AtomicU32::new(0);
pub static WLEGL_BUFFERS_DESTROYED: AtomicU32 = AtomicU32::new(0);

pub struct WleglBufferData {
    pub ahb: *mut std::ffi::c_void,
    pub width: i32,
    pub height: i32,
    pub has_alpha: bool,
    pub is_external: bool,
    pub importer: AhbTextureImporter,
    pub texture: Mutex<Option<(usize, GlesTexture)>>,
}

impl ExternalBuffer for WleglBufferData {
    fn dimensions(&self) -> Size<i32, BufferCoord> {
        Size::from((self.width, self.height))
    }

    fn has_alpha(&self) -> Option<bool> {
        Some(self.has_alpha)
    }

    fn y_inverted(&self) -> Option<bool> {
        Some(false)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn import_gles(
        &self,
        renderer: &mut GlesRenderer,
        _surface: Option<&SurfaceData>,
        _damage: &[Rectangle<i32, BufferCoord>],
    ) -> Option<Result<GlesTexture, ExternalBufferImportError>> {
        let renderer_id = renderer.id();
        if let Some((cached_renderer_id, texture)) = self.texture.lock().unwrap().clone() {
            if cached_renderer_id == renderer_id {
                return Some(Ok(texture));
            }
        }

        log::info!(
            "wlegl: importing AHB={:?} size={}x{} renderer_id={} target=GL_TEXTURE_EXTERNAL_OES",
            self.ahb,
            self.width,
            self.height,
            renderer_id,
        );

        let texture = (|| {
            unsafe {
                renderer
                    .egl_context()
                    .make_current()
                    .map_err(|err| -> ExternalBufferImportError { Box::new(err) })?;
            }
            let display = renderer.egl_context().display().get_display_handle();
            self.importer
                .import_ahb(
                    renderer,
                    **display,
                    self.ahb,
                    self.width,
                    self.height,
                    self.is_external,
                )
                .map_err(|err| -> ExternalBufferImportError {
                    std::io::Error::new(std::io::ErrorKind::Other, err).into()
                })
        })();

        let texture = match texture {
            Ok(texture) => texture,
            Err(err) => return Some(Err(err)),
        };

        *self.texture.lock().unwrap() = Some((renderer_id, texture.clone()));
        Some(Ok(texture))
    }
}

impl Drop for WleglBufferData {
    fn drop(&mut self) {
        WLEGL_BUFFERS_DESTROYED.fetch_add(1, Ordering::Relaxed);
        if !self.ahb.is_null() {
            unsafe { tawc_wlegl_buffer_release(self.ahb) };
            self.ahb = ptr::null_mut();
        }
    }
}

unsafe impl Send for WleglBufferData {}
unsafe impl Sync for WleglBufferData {}

pub struct WleglHandleData {
    pub inner: Mutex<WleglHandleInner>,
}

pub struct WleglHandleInner {
    pub expected_fds: i32,
    pub fds: Vec<OwnedFd>,
    pub ints: Vec<i32>,
}

impl WleglHandleData {
    pub fn new(expected_fds: i32, ints: Vec<i32>) -> Self {
        Self {
            inner: Mutex::new(WleglHandleInner {
                expected_fds,
                fds: Vec::with_capacity(expected_fds as usize),
                ints,
            }),
        }
    }
}

pub struct WleglGlobalData {
    pub importer: AhbTextureImporter,
}

pub fn handle_wlegl_request<D>(
    resource: &AndroidWlegl,
    request: android_wlegl::Request,
    importer: &AhbTextureImporter,
    data_init: &mut DataInit<'_, D>,
) where
    D: smithay::reexports::wayland_server::Dispatch<AndroidWleglHandle, WleglHandleData>
        + smithay::reexports::wayland_server::Dispatch<WlBuffer, ExternalBufferData>
        + 'static,
{
    match request {
        android_wlegl::Request::CreateHandle { id, num_fds, ints } => {
            if ints.len() % 4 != 0 {
                resource.post_error(
                    android_wlegl::Error::BadValue,
                    "ints array length is not a multiple of 4",
                );
                return;
            }
            let ints_i32: Vec<i32> = ints
                .chunks_exact(4)
                .map(|c| i32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                .collect();

            data_init.init(id, WleglHandleData::new(num_fds, ints_i32));
        }
        android_wlegl::Request::CreateBuffer {
            id,
            width,
            height,
            stride,
            format,
            usage,
            native_handle,
        } => {
            let handle_data = match native_handle.data::<WleglHandleData>() {
                Some(d) => d,
                None => {
                    resource.post_error(
                        android_wlegl::Error::BadHandle,
                        "native_handle has invalid user data",
                    );
                    return;
                }
            };

            let mut inner = handle_data.inner.lock().unwrap();
            if inner.fds.len() as i32 != inner.expected_fds {
                resource.post_error(
                    android_wlegl::Error::BadHandle,
                    format!(
                        "expected {} fds, got {}",
                        inner.expected_fds,
                        inner.fds.len()
                    ),
                );
                return;
            }

            let raw_fds: Vec<RawFd> = inner.fds.iter().map(|f| f.as_raw_fd()).collect();
            let ints = inner.ints.clone();

            let w_u = width as u32;
            let h_u = height as u32;
            let stride_u = stride as u32;
            let fmt_u = format as u32;
            let usage_u64 = (usage as u32) as u64;

            let ahb = unsafe {
                tawc_wlegl_import(
                    w_u,
                    h_u,
                    stride_u,
                    fmt_u,
                    usage_u64,
                    raw_fds.as_ptr(),
                    raw_fds.len() as i32,
                    ints.as_ptr(),
                    ints.len() as i32,
                )
            };

            if ahb.is_null() {
                resource.post_error(
                    android_wlegl::Error::BadHandle,
                    "AHardwareBuffer_createFromHandle failed",
                );
                return;
            }

            // AHB registered successfully and owns the file descriptors
            for fd in inner.fds.drain(..) {
                std::mem::forget(fd);
            }
            drop(inner);

            WLEGL_BUFFERS_CREATED.fetch_add(1, Ordering::Relaxed);

            let buffer_data = WleglBufferData {
                ahb,
                width,
                height,
                has_alpha: !matches!(fmt_u, 2 | 3 | 4),
                // Android native buffers must always be sampled through
                // GL_TEXTURE_EXTERNAL_OES in the compositor, regardless of
                // whether the producer rendered into the EGLImage through a
                // GL_TEXTURE_2D framebuffer attachment.
                is_external: true,
                importer: *importer,
                texture: Mutex::new(None),
            };

            data_init.init(id, ExternalBufferData::new(buffer_data));
        }
        android_wlegl::Request::GetServerBufferHandle { .. } => {
            resource.post_error(
                android_wlegl::Error::BadValue,
                "server-side buffer allocation not supported",
            );
        }
    }
}

pub fn handle_handle_request(
    resource: &AndroidWleglHandle,
    request: android_wlegl_handle::Request,
    data: &WleglHandleData,
) {
    match request {
        android_wlegl_handle::Request::AddFd { fd } => {
            let mut inner = data.inner.lock().unwrap();
            if inner.fds.len() as i32 >= inner.expected_fds {
                resource.post_error(android_wlegl_handle::Error::TooManyFds, "too many fds");
                return;
            }
            inner.fds.push(fd);
        }
        android_wlegl_handle::Request::Destroy => {}
    }
}
