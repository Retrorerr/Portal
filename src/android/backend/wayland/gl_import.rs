//! Android EGL/GLES extension loading and AHardwareBuffer texture import.

use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::utils::Size;
use std::ffi::{c_void, CString};

const EGL_NATIVE_BUFFER_ANDROID: u32 = 0x3140;
const EGL_IMAGE_PRESERVED_KHR: i32 = 0x30D2;
const EGL_NO_CONTEXT: *const c_void = std::ptr::null();
const EGL_NONE: i32 = 0x3038;
const EGL_TRUE: i32 = 1;

const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_TEXTURE_EXTERNAL_OES: u32 = 0x8D65;
const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
const GL_TEXTURE_MAG_FILTER: u32 = 0x2800;
const GL_LINEAR: i32 = 0x2601;

type FnEglGetNativeClientBufferANDROID = unsafe extern "C" fn(*const c_void) -> *const c_void;
type FnEglCreateImageKHR = unsafe extern "C" fn(
    *const c_void,
    *const c_void,
    u32,
    *const c_void,
    *const i32,
) -> *const c_void;
type FnEglDestroyImageKHR = unsafe extern "C" fn(*const c_void, *const c_void) -> u32;
type FnGlGenTextures = unsafe extern "C" fn(i32, *mut u32);
type FnGlBindTexture = unsafe extern "C" fn(u32, u32);
type FnGlTexParameteri = unsafe extern "C" fn(u32, u32, i32);
type FnGlEGLImageTargetTexture2DOES = unsafe extern "C" fn(u32, *const c_void);
type FnGlGetError = unsafe extern "C" fn() -> u32;

#[derive(Clone, Copy)]
pub struct AhbTextureImporter {
    egl_get_native_client_buffer: FnEglGetNativeClientBufferANDROID,
    egl_create_image: FnEglCreateImageKHR,
    egl_destroy_image: FnEglDestroyImageKHR,
    gl_gen_textures: FnGlGenTextures,
    gl_bind_texture: FnGlBindTexture,
    gl_tex_parameteri: FnGlTexParameteri,
    gl_egl_image_target_texture_2d_oes: FnGlEGLImageTargetTexture2DOES,
    gl_get_error: FnGlGetError,
}

impl AhbTextureImporter {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let egl_lib = libloading::Library::new("libEGL.so")
                .map_err(|e| format!("Failed to load libEGL.so: {}", e))?;
            let gles_lib = libloading::Library::new("libGLESv3.so")
                .or_else(|_| libloading::Library::new("libGLESv2.so"))
                .map_err(|e| format!("Failed to load libGLES: {}", e))?;

            let egl_get_proc_address: libloading::Symbol<
                unsafe extern "C" fn(*const i8) -> *const c_void,
            > = egl_lib
                .get(b"eglGetProcAddress\0")
                .map_err(|e| format!("Failed to get eglGetProcAddress: {}", e))?;

            let load_proc = |name: &str| -> Result<*const c_void, String> {
                let cname = CString::new(name).unwrap();
                let ptr = egl_get_proc_address(cname.as_ptr() as *const _);
                if !ptr.is_null() {
                    Ok(ptr)
                } else {
                    let bname = format!("{}\0", name);
                    if let Ok(sym) = egl_lib.get::<*const c_void>(bname.as_bytes()) {
                        Ok(*sym)
                    } else if let Ok(sym) = gles_lib.get::<*const c_void>(bname.as_bytes()) {
                        Ok(*sym)
                    } else {
                        Err(format!("Symbol {} not found", name))
                    }
                }
            };

            let load_gl = |name: &[u8]| -> Result<*const c_void, String> {
                let sym: libloading::Symbol<*const c_void> = gles_lib
                    .get(name)
                    .map_err(|e| format!("Failed to get GL symbol: {}", e))?;
                Ok(*sym)
            };

            let importer = Self {
                egl_get_native_client_buffer: std::mem::transmute(load_proc(
                    "eglGetNativeClientBufferANDROID",
                )?),
                egl_create_image: std::mem::transmute(load_proc("eglCreateImageKHR")?),
                egl_destroy_image: std::mem::transmute(load_proc("eglDestroyImageKHR")?),
                gl_gen_textures: std::mem::transmute(load_gl(b"glGenTextures\0")?),
                gl_bind_texture: std::mem::transmute(load_gl(b"glBindTexture\0")?),
                gl_tex_parameteri: std::mem::transmute(load_gl(b"glTexParameteri\0")?),
                gl_egl_image_target_texture_2d_oes: std::mem::transmute(load_proc(
                    "glEGLImageTargetTexture2DOES",
                )?),
                gl_get_error: std::mem::transmute(load_gl(b"glGetError\0")?),
            };

            std::mem::forget(egl_lib);
            std::mem::forget(gles_lib);

            Ok(importer)
        }
    }

    pub fn import_ahb(
        &self,
        renderer: &GlesRenderer,
        raw_display: *const c_void,
        ahb: *mut c_void,
        width: i32,
        height: i32,
        is_external: bool,
    ) -> Result<GlesTexture, String> {
        unsafe {
            let client_buffer = (self.egl_get_native_client_buffer)(ahb);
            if client_buffer.is_null() {
                return Err("eglGetNativeClientBufferANDROID returned null".into());
            }

            while (self.gl_get_error)() != 0 {}

            let attribs: [i32; 3] = [EGL_IMAGE_PRESERVED_KHR, EGL_TRUE, EGL_NONE];
            let image = (self.egl_create_image)(
                raw_display,
                EGL_NO_CONTEXT,
                EGL_NATIVE_BUFFER_ANDROID,
                client_buffer,
                attribs.as_ptr(),
            );
            if image.is_null() {
                return Err("eglCreateImageKHR failed".into());
            }

            let mut tex: u32 = 0;
            (self.gl_gen_textures)(1, &mut tex);
            if tex == 0 {
                (self.egl_destroy_image)(raw_display, image);
                return Err("glGenTextures failed".into());
            }

            let target = if is_external {
                GL_TEXTURE_EXTERNAL_OES
            } else {
                GL_TEXTURE_2D
            };
            (self.gl_bind_texture)(target, tex);
            (self.gl_egl_image_target_texture_2d_oes)(target, image);

            let gl_err = (self.gl_get_error)();
            if gl_err != 0 {
                (self.egl_destroy_image)(raw_display, image);
                return Err(format!(
                    "glEGLImageTargetTexture2DOES failed with GL error: 0x{:X}",
                    gl_err
                ));
            }

            (self.gl_tex_parameteri)(target, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
            (self.gl_tex_parameteri)(target, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
            (self.gl_bind_texture)(target, 0);

            // Intermediate EGLImage can be destroyed; the GL texture owns its own gralloc ref
            (self.egl_destroy_image)(raw_display, image);

            let size = Size::from((width, height));
            let texture = GlesTexture::from_raw_with_flags(
                renderer,
                None,
                false,
                is_external,
                false,
                tex,
                size,
            );

            Ok(texture)
        }
    }
}
