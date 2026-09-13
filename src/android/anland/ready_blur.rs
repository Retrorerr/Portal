//! Hybrid CPU-map + GPU blur for the live Anland desktop beneath READY.
//!
//! Normal presentation never enters this module. While READY owns the veil,
//! the render thread CPU-waits on KWin's fence (via a dup, so the original
//! stays valid on failure), mmaps the dequeued BufferQueue slot, uploads it
//! to a private GPU texture, runs a quarter-resolution separable 5-tap
//! Gaussian through two reused scratch targets, resolves back to full
//! resolution on the GPU, downloads the filtered image into the same slot,
//! flushes CPU caches, and returns bare (-1) for SurfaceFlinger. Upload,
//! blur math and download are driver-accelerated (fast in any Rust profile);
//! Rust itself runs no per-pixel loops. The scratch pass never samples from
//! the slot while writing it.
//!
//! Why hybrid (physical-device findings, OnePlus Pad 3):
//! - Host EGL reports neither EGL_EXT_image_dma_buf_import nor its
//!   modifiers variant, so linux-dma-buf EGLImage import fails BadMatch.
//! - EGL_NATIVE_BUFFER_ANDROID import of these CPU-connected BufferQueue
//!   slots fails BadParameter (usage 0x933, format RGBA_8888).
//! - The slots ARE CPU-mappable and KWin's fence is pollable, so mmap +
//!   glTexSubImage upload works without any EGLImage import.
//! - A pure-CPU blur is correct but takes 4s+ per 8MP frame in unoptimized
//!   debug builds (render thread stall, Plasma freezes at 0.2Hz). The hybrid
//!   keeps all bulk pixel work in the driver/GPU.
//! Architecture preserved from review: wait at the legal producer-fence
//! boundary, quarter-res separable Gaussian with reused targets, filtered
//! writeback finishing before SurfaceFlinger, safe fallback to the original
//! fence on any failure, and a zero-work fast path when READY is disabled.

use std::ffi::c_void;
use std::os::unix::io::{FromRawFd, OwnedFd};

use glow::HasContext;
use khronos_egl as egl;

use super::anw::ANativeWindowBuffer;

/// CPU wait budget for KWin's fence inside the READY path.
const BLUR_FENCE_WAIT_MS: i32 = 2000;

/// Quarter resolution denominator.
const DOWNSAMPLE: i32 = 4;

/// dma-buf cache sync (linux/dma-buf.h).
const DMA_BUF_IOCTL_SYNC: u32 = 0x4008_6200;
const DMA_BUF_SYNC_READ: u64 = 1 << 0;
const DMA_BUF_SYNC_WRITE: u64 = 1 << 1;
const DMA_BUF_SYNC_RW: u64 = DMA_BUF_SYNC_READ | DMA_BUF_SYNC_WRITE;
const DMA_BUF_SYNC_END: u64 = 1 << 2;

#[repr(C)]
struct DmaBufSync {
    flags: u64,
}

fn dma_buf_sync(fd: i32, flags: u64) -> Result<(), String> {
    let sync = DmaBufSync { flags };
    let r = unsafe { libc::ioctl(fd, DMA_BUF_IOCTL_SYNC as i32, &sync) };
    if r != 0 {
        return Err(format!(
            "dma_buf_sync flags=0x{flags:x} failed errno={}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

pub(super) struct ReadyBlur {
    egl: egl::DynamicInstance<egl::EGL1_4>,
    display: egl::Display,
    context: egl::Context,
    surface: egl::Surface,
    gl: glow::Context,
    program: glow::NativeProgram,
    vao: glow::NativeVertexArray,
    input_tex: glow::NativeTexture,
    output_tex: glow::NativeTexture,
    output_fbo: glow::NativeFramebuffer,
    scratch_a: glow::NativeTexture,
    scratch_b: glow::NativeTexture,
    scratch_fbo_a: glow::NativeFramebuffer,
    scratch_fbo_b: glow::NativeFramebuffer,
    width: i32,
    height: i32,
    stride_px: i32,
    scratch_width: i32,
    scratch_height: i32,
    first_frame_logged: bool,
}

impl ReadyBlur {
    pub(super) unsafe fn new(width: u32, height: u32) -> Result<Self, String> {
        let egl = egl::DynamicInstance::<egl::EGL1_4>::load_required()
            .map_err(|e| format!("load EGL: {e}"))?;
        let display = egl
            .get_display(egl::DEFAULT_DISPLAY)
            .ok_or("eglGetDisplay returned null")?;
        egl.initialize(display)
            .map_err(|e| format!("eglInitialize: {e}"))?;
        egl.bind_api(egl::OPENGL_ES_API)
            .map_err(|e| format!("eglBindAPI: {e}"))?;
        let config = egl
            .choose_first_config(
                display,
                &[
                    egl::SURFACE_TYPE,
                    egl::PBUFFER_BIT,
                    egl::RENDERABLE_TYPE,
                    egl::OPENGL_ES3_BIT,
                    egl::RED_SIZE,
                    8,
                    egl::GREEN_SIZE,
                    8,
                    egl::BLUE_SIZE,
                    8,
                    egl::ALPHA_SIZE,
                    8,
                    egl::NONE,
                ],
            )
            .map_err(|e| format!("eglChooseConfig: {e}"))?
            .ok_or("no ES3 pbuffer EGLConfig")?;
        let context = egl
            .create_context(
                display,
                config,
                None,
                &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE],
            )
            .map_err(|e| format!("eglCreateContext: {e}"))?;
        let surface = egl
            .create_pbuffer_surface(display, config, &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE])
            .map_err(|e| {
                let _ = egl.destroy_context(display, context);
                format!("eglCreatePbufferSurface: {e}")
            })?;
        egl.make_current(display, Some(surface), Some(surface), Some(context))
            .map_err(|e| {
                let _ = egl.destroy_surface(display, surface);
                let _ = egl.destroy_context(display, context);
                format!("eglMakeCurrent: {e}")
            })?;

        let gl = glow::Context::from_loader_function(|name| {
            egl.get_proc_address(name)
                .map(|f| f as *const c_void)
                .unwrap_or(std::ptr::null())
        });
        let program = compile_program(&gl).map_err(|e| {
            let _ = egl.make_current(display, None, None, None);
            let _ = egl.destroy_surface(display, surface);
            let _ = egl.destroy_context(display, context);
            e
        })?;
        let vao = gl.create_vertex_array().map_err(|e| {
            gl.delete_program(program);
            let _ = egl.make_current(display, None, None, None);
            let _ = egl.destroy_surface(display, surface);
            let _ = egl.destroy_context(display, context);
            e.to_string()
        })?;
        let width_i = width as i32;
        let height_i = height as i32;
        let scratch_width = (width_i + DOWNSAMPLE - 1) / DOWNSAMPLE;
        let scratch_height = (height_i + DOWNSAMPLE - 1) / DOWNSAMPLE;

        let input_tex = create_full_target(&gl).map_err(|e| {
            gl.delete_vertex_array(vao);
            gl.delete_program(program);
            let _ = egl.make_current(display, None, None, None);
            let _ = egl.destroy_surface(display, surface);
            let _ = egl.destroy_context(display, context);
            format!("input texture: {e}")
        })?;
        // Size the storage now; contents are uploaded per frame.
        gl.bind_texture(glow::TEXTURE_2D, Some(input_tex));
        gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, width_i, height_i);

        let (output_tex, output_fbo) = create_target(&gl, width_i, height_i).map_err(|e| {
            gl.delete_texture(input_tex);
            gl.delete_vertex_array(vao);
            gl.delete_program(program);
            let _ = egl.make_current(display, None, None, None);
            let _ = egl.destroy_surface(display, surface);
            let _ = egl.destroy_context(display, context);
            format!("output target: {e}")
        })?;
        let (scratch_a, scratch_fbo_a) = create_target(&gl, scratch_width, scratch_height)
            .map_err(|e| {
                gl.delete_framebuffer(output_fbo);
                gl.delete_texture(output_tex);
                gl.delete_texture(input_tex);
                gl.delete_vertex_array(vao);
                gl.delete_program(program);
                let _ = egl.make_current(display, None, None, None);
                let _ = egl.destroy_surface(display, surface);
                let _ = egl.destroy_context(display, context);
                format!("scratch A: {e}")
            })?;
        let (scratch_b, scratch_fbo_b) = create_target(&gl, scratch_width, scratch_height)
            .map_err(|e| {
                gl.delete_framebuffer(scratch_fbo_a);
                gl.delete_texture(scratch_a);
                gl.delete_framebuffer(output_fbo);
                gl.delete_texture(output_tex);
                gl.delete_texture(input_tex);
                gl.delete_vertex_array(vao);
                gl.delete_program(program);
                let _ = egl.make_current(display, None, None, None);
                let _ = egl.destroy_surface(display, surface);
                let _ = egl.destroy_context(display, context);
                format!("scratch B: {e}")
            })?;
        gl.bind_vertex_array(Some(vao));
        gl.disable(glow::BLEND);
        gl.disable(glow::DEPTH_TEST);

        log::info!(
            "anland.ready_blur=hybrid_initialized full={}x{} scratch={}x{} passes=2+resolve backend=mmap-upload-gpu",
            width,
            height,
            scratch_width,
            scratch_height
        );
        Ok(Self {
            egl,
            display,
            context,
            surface,
            gl,
            program,
            vao,
            input_tex,
            output_tex,
            output_fbo,
            scratch_a,
            scratch_b,
            scratch_fbo_a,
            scratch_fbo_b,
            width: width_i,
            height: height_i,
            stride_px: 0,
            scratch_width,
            scratch_height,
            first_frame_logged: false,
        })
    }

    /// On success consumes `input_fence` (closed) and returns -1 (bare):
    /// the download is synchronous, so SurfaceFlinger needs no GPU fence.
    /// On error the original fd remains valid for the direct queue path.
    pub(super) unsafe fn process(
        &mut self,
        anb: *mut ANativeWindowBuffer,
        input_fence: i32,
        radius_px: f32,
        strength: f32,
    ) -> Result<i32, String> {
        self.egl
            .make_current(
                self.display,
                Some(self.surface),
                Some(self.surface),
                Some(self.context),
            )
            .map_err(|e| format!("eglMakeCurrent: {e}"))?;
        if anb.is_null() {
            return Err("null ANativeWindowBuffer".into());
        }
        if input_fence >= 0 {
            let dup = unsafe { libc::dup(input_fence) };
            if dup < 0 {
                return Err(format!(
                    "dup(input fence) failed errno={}",
                    std::io::Error::last_os_error()
                ));
            }
            let owned = unsafe { OwnedFd::from_raw_fd(dup) };
            if let Err(e) = super::sys::wait_fence(owned, BLUR_FENCE_WAIT_MS) {
                return Err(format!("kwin fence wait failed: {e}"));
            }
        }

        let (buf_fd, stride_px, bw, bh) =
            unsafe { super::anw::AnwApi::buffer_dma_info(anb) }.ok_or("buffer_dma_info missing")?;
        if bw != self.width || bh != self.height {
            return Err(format!(
                "slot size {bw}x{bh} mismatches session {}x{}",
                self.width, self.height
            ));
        }
        if stride_px < bw || stride_px <= 0 {
            return Err(format!("bad stride_px={stride_px} for width {bw}"));
        }
        self.stride_px = stride_px;
        let radius = if radius_px.is_finite() {
            radius_px.clamp(0.0, 256.0)
        } else {
            0.0
        };
        let strength = if strength.is_finite() {
            strength.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let stride_bytes = stride_px as usize * 4;
        let len = stride_bytes * bh as usize;
        if len == 0 {
            return Err("zero-length slot".into());
        }

        dma_buf_sync(buf_fd, DMA_BUF_SYNC_RW)?;
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                buf_fd,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            let _ = dma_buf_sync(buf_fd, DMA_BUF_SYNC_RW | DMA_BUF_SYNC_END);
            return Err(format!(
                "mmap slot failed errno={}",
                std::io::Error::last_os_error()
            ));
        }
        // Upload (with stride), GPU blur, download (with stride) into the
        // same mapping. Any failure leaves the slot untouched except for a
        // possibly partial download; the caller falls back to the original
        // fence only when nothing was consumed, otherwise to bare.
        let r = self.gpu_blur_mapped(ptr as *mut u8, stride_bytes, len, radius, strength);
        unsafe {
            libc::munmap(ptr, len);
        }
        // Flush CPU caches so SurfaceFlinger/HWC sees the filtered pixels.
        if let Err(e) = dma_buf_sync(buf_fd, DMA_BUF_SYNC_RW | DMA_BUF_SYNC_END) {
            return Err(format!("dma_buf_sync END failed: {e}"));
        }
        // The blur wrote through the mapping; even a blur-pass failure after
        // a partial download must not present torn pixels under KWin's fence.
        // gpu_blur_mapped only returns Ok after a verified download, so here
        // pixels are final: consume the original fence and queue bare.
        r?;

        if input_fence >= 0 {
            unsafe { libc::close(input_fence) };
        }
        if !self.first_frame_logged {
            self.first_frame_logged = true;
            log::info!(
                "anland.ready_blur=frame_postprocessed actual=true backend=mmap-upload-gpu radius_px={radius:.1} strength={strength:.3} input_fenced={}",
                input_fence >= 0
            );
        }
        Ok(-1)
    }

    unsafe fn gpu_blur_mapped(
        &mut self,
        base: *mut u8,
        stride_bytes: usize,
        len: usize,
        radius_px: f32,
        strength: f32,
    ) -> Result<(), String> {
        let w = self.width;
        let h = self.height;
        // Upload with the slot's stride so padding is preserved.
        self.gl
            .pixel_store_i32(glow::UNPACK_ROW_LENGTH, self.stride_px);
        self.gl.bind_texture(glow::TEXTURE_2D, Some(self.input_tex));
        self.set_texture_parameters();
        unsafe {
            self.gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                w,
                h,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(std::slice::from_raw_parts(
                    base as *const u8,
                    len,
                ))),
            );
        }
        self.gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);

        // Two separable passes at quarter res, then resolve to full res.
        // Step math mirrors the reviewed design (radius spread over 4x).
        let step_full_x = radius_px * strength / 3.230_769 / w as f32;
        let step_full_y = radius_px * strength / 3.230_769 / h as f32;
        self.draw_pass(
            self.input_tex,
            self.scratch_fbo_a,
            self.scratch_width,
            self.scratch_height,
            step_full_x,
            0.0,
        )?;
        self.draw_pass(
            self.scratch_a,
            self.scratch_fbo_b,
            self.scratch_width,
            self.scratch_height,
            0.0,
            step_full_y,
        )?;
        self.draw_pass(self.scratch_b, self.output_fbo, w, h, 0.0, 0.0)?;

        // Synchronous download straight into the mapped slot (with stride).
        self.gl
            .pixel_store_i32(glow::PACK_ROW_LENGTH, self.stride_px);
        self.gl
            .bind_framebuffer(glow::FRAMEBUFFER, Some(self.output_fbo));
        self.gl.viewport(0, 0, w, h);
        unsafe {
            self.gl.read_pixels(
                0,
                0,
                w,
                h,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(std::slice::from_raw_parts_mut(base, len))),
            );
        }
        self.gl.pixel_store_i32(glow::PACK_ROW_LENGTH, 0);
        let error = self.gl.get_error();
        if error != glow::NO_ERROR {
            return Err(format!("GLES download error=0x{error:x}"));
        }
        // read_pixels blocks until the full pipeline completes, so the slot
        // is final when we return. Drop a finish for explicitness on
        // tiled GPUs before the cache flush in the caller.
        self.gl.finish();
        let _ = (stride_bytes, len);
        Ok(())
    }

    unsafe fn set_texture_parameters(&self) {
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
    }

    unsafe fn draw_pass(
        &self,
        source: glow::NativeTexture,
        target: glow::NativeFramebuffer,
        width: i32,
        height: i32,
        step_x: f32,
        step_y: f32,
    ) -> Result<(), String> {
        self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target));
        self.gl.viewport(0, 0, width, height);
        self.gl.use_program(Some(self.program));
        self.gl.active_texture(glow::TEXTURE0);
        self.gl.bind_texture(glow::TEXTURE_2D, Some(source));
        self.gl.uniform_1_i32(
            self.gl
                .get_uniform_location(self.program, "source")
                .as_ref(),
            0,
        );
        self.gl.uniform_2_f32(
            self.gl
                .get_uniform_location(self.program, "stepUv")
                .as_ref(),
            step_x,
            step_y,
        );
        self.gl.bind_vertex_array(Some(self.vao));
        self.gl.draw_arrays(glow::TRIANGLES, 0, 3);
        let error = self.gl.get_error();
        if error != glow::NO_ERROR {
            return Err(format!("GLES blur pass error=0x{error:x}"));
        }
        Ok(())
    }
}

impl Drop for ReadyBlur {
    fn drop(&mut self) {
        unsafe {
            let _ = self.egl.make_current(
                self.display,
                Some(self.surface),
                Some(self.surface),
                Some(self.context),
            );
            self.gl.delete_framebuffer(self.output_fbo);
            self.gl.delete_framebuffer(self.scratch_fbo_a);
            self.gl.delete_framebuffer(self.scratch_fbo_b);
            self.gl.delete_texture(self.input_tex);
            self.gl.delete_texture(self.output_tex);
            self.gl.delete_texture(self.scratch_a);
            self.gl.delete_texture(self.scratch_b);
            self.gl.delete_vertex_array(self.vao);
            self.gl.delete_program(self.program);
            let _ = self.egl.make_current(self.display, None, None, None);
            let _ = self.egl.destroy_surface(self.display, self.surface);
            let _ = self.egl.destroy_context(self.display, self.context);
            // No eglTerminate: DEFAULT_DISPLAY is shared process-wide with
            // the rest of the host. Destroying only our context/surface.
            log::info!("anland.ready_blur=gpu_resources_released");
        }
    }
}

unsafe fn create_full_target(gl: &glow::Context) -> Result<glow::NativeTexture, String> {
    let texture = gl.create_texture()?;
    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
    for (pname, param) in [
        (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
        (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
        (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
        (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
    ] {
        gl.tex_parameter_i32(glow::TEXTURE_2D, pname, param as i32);
    }
    Ok(texture)
}

unsafe fn create_target(
    gl: &glow::Context,
    width: i32,
    height: i32,
) -> Result<(glow::NativeTexture, glow::NativeFramebuffer), String> {
    let texture = gl.create_texture()?;
    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
    gl.tex_parameter_i32(
        glow::TEXTURE_2D,
        glow::TEXTURE_MIN_FILTER,
        glow::LINEAR as i32,
    );
    gl.tex_parameter_i32(
        glow::TEXTURE_2D,
        glow::TEXTURE_MAG_FILTER,
        glow::LINEAR as i32,
    );
    gl.tex_parameter_i32(
        glow::TEXTURE_2D,
        glow::TEXTURE_WRAP_S,
        glow::CLAMP_TO_EDGE as i32,
    );
    gl.tex_parameter_i32(
        glow::TEXTURE_2D,
        glow::TEXTURE_WRAP_T,
        glow::CLAMP_TO_EDGE as i32,
    );
    gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, width, height);
    let framebuffer = gl.create_framebuffer()?;
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
    gl.framebuffer_texture_2d(
        glow::FRAMEBUFFER,
        glow::COLOR_ATTACHMENT0,
        glow::TEXTURE_2D,
        Some(texture),
        0,
    );
    if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
        gl.delete_framebuffer(framebuffer);
        gl.delete_texture(texture);
        return Err("scratch framebuffer incomplete".into());
    }
    Ok((texture, framebuffer))
}

unsafe fn compile_program(gl: &glow::Context) -> Result<glow::NativeProgram, String> {
    let program = gl.create_program()?;
    let vertex = gl.create_shader(glow::VERTEX_SHADER)?;
    gl.shader_source(vertex, VERTEX_SHADER);
    gl.compile_shader(vertex);
    if !gl.get_shader_compile_status(vertex) {
        let log = gl.get_shader_info_log(vertex);
        gl.delete_shader(vertex);
        gl.delete_program(program);
        return Err(format!("blur vertex shader: {log}"));
    }
    let fragment = gl.create_shader(glow::FRAGMENT_SHADER)?;
    gl.shader_source(fragment, FRAGMENT_SHADER);
    gl.compile_shader(fragment);
    if !gl.get_shader_compile_status(fragment) {
        let log = gl.get_shader_info_log(fragment);
        gl.delete_shader(vertex);
        gl.delete_shader(fragment);
        gl.delete_program(program);
        return Err(format!("blur fragment shader: {log}"));
    }
    gl.attach_shader(program, vertex);
    gl.attach_shader(program, fragment);
    gl.link_program(program);
    gl.delete_shader(vertex);
    gl.delete_shader(fragment);
    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        gl.delete_program(program);
        return Err(format!("blur program link: {log}"));
    }
    Ok(program)
}

const VERTEX_SHADER: &str = r#"#version 300 es
precision highp float;
out vec2 uv;
void main() {
    vec2 p = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2);
    uv = p;
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const FRAGMENT_SHADER: &str = r#"#version 300 es
precision mediump float;
uniform sampler2D source;
uniform vec2 stepUv;
in vec2 uv;
out vec4 color;
void main() {
    vec4 sum = texture(source, uv) * 0.227027;
    sum += texture(source, uv + stepUv * 1.384615) * 0.316216;
    sum += texture(source, uv - stepUv * 1.384615) * 0.316216;
    sum += texture(source, uv + stepUv * 3.230769) * 0.070270;
    sum += texture(source, uv - stepUv * 3.230769) * 0.070270;
    color = sum;
}
"#;
