//! Portal PipeWire capture helper: the tablet's microphone and cameras as
//! guest PipeWire devices.
//!
//! Like `localdesktop_pipewire_aaudio_sink`, this is a normal PipeWire client
//! process on the Android side, supervised by
//! `src/android/backend/pipewire_standalone_aaudio.rs`. One process per mode:
//!
//! - `--mic` registers `Audio/Source` "Portal Microphone" and feeds it from an
//!   AAudio input stream;
//! - `--camera` registers one `Video/Source` (media.role Camera) per Android
//!   camera and feeds it from NDK Camera2 through an AImageReader. V4L2 apps
//!   reach these nodes through PipeWire's libpw-v4l2 (see
//!   `assets/guest-arm64/portal-v4l2.c`).
//!
//! The nodes exist from the start so apps can list them, but the Android
//! device opens only while a guest app is actually streaming. The first time
//! that happens without a decision the helper prints `demand` on stderr; the
//! supervisor asks Android for RECORD_AUDIO/CAMERA and answers on stdin:
//!
//! ```text
//! grant | deny       permission for this helper's device
//! pause | resume     Portal left / returned to the foreground
//! rotation DEGREES   display rotation (camera), 0/90/180/270
//! ```
//!
//! Without a grant (or while paused) the mic records silence and cameras
//! show black frames, so guest apps keep working instead of failing.
//!
//! Android-only behind the `pipewire-sink` feature, like the sink, and built
//! by `scripts/build-pipewire-aaudio-sink.sh` (`BIN=localdesktop_pipewire_capture`).
//! The ring, rotation and pixel conversion below build on any host, so
//! `cargo test --features pipewire-sink --bin localdesktop_pipewire_capture`
//! covers them.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

macro_rules! note {
    ($($arg:tt)*) => {
        eprintln!("[portal-capture] {}", format_args!($($arg)*))
    };
}

// ----------------------------------------------------------------------------
// Control state from the supervisor
// ----------------------------------------------------------------------------

const PERMISSION_UNKNOWN: u8 = 0;
const PERMISSION_GRANTED: u8 = 1;
const PERMISSION_DENIED: u8 = 2;

static PERMISSION: AtomicU8 = AtomicU8::new(PERMISSION_UNKNOWN);
static PAUSED: AtomicBool = AtomicBool::new(false);
/// Display rotation in degrees (Surface.ROTATION_* × 90).
static ROTATION: AtomicU32 = AtomicU32::new(0);

fn apply_command(line: &str) {
    let mut words = line.split_whitespace();
    match (words.next(), words.next()) {
        (Some("grant"), _) => PERMISSION.store(PERMISSION_GRANTED, Ordering::Release),
        (Some("deny"), _) => PERMISSION.store(PERMISSION_DENIED, Ordering::Release),
        (Some("pause"), _) => PAUSED.store(true, Ordering::Release),
        (Some("resume"), _) => PAUSED.store(false, Ordering::Release),
        (Some("rotation"), Some(value)) => {
            if let Ok(degrees) = value.parse::<u32>() {
                ROTATION.store(degrees % 360 / 90 * 90, Ordering::Release);
            }
        }
        (None, _) => {}
        _ => note!("ignoring command {line:?}"),
    }
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn spawn_command_reader() {
    std::thread::spawn(|| {
        use std::io::BufRead;
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            apply_command(&line);
        }
        // The supervisor is gone: nothing will ever grant or resume again.
        note!("control channel closed; exiting");
        std::process::exit(0);
    });
}

// ----------------------------------------------------------------------------
// Microphone ring: AAudio callback (producer) -> PipeWire process (consumer)
// ----------------------------------------------------------------------------

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct Ring {
    frames: u64,
    samples: Vec<UnsafeCell<f32>>,
    read_frame: AtomicU64,
    write_frame: AtomicU64,
}

unsafe impl Send for Ring {}
unsafe impl Sync for Ring {}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
impl Ring {
    fn new(frames: u64) -> Ring {
        Ring {
            frames,
            samples: (0..frames).map(|_| UnsafeCell::new(0.0)).collect(),
            read_frame: AtomicU64::new(0),
            write_frame: AtomicU64::new(0),
        }
    }

    fn buffered(&self) -> u64 {
        let read = self.read_frame.load(Ordering::Acquire);
        self.write_frame.load(Ordering::Acquire).saturating_sub(read)
    }

    fn clear(&self) {
        let write = self.write_frame.load(Ordering::Acquire);
        self.read_frame.store(write, Ordering::Release);
    }

    /// Producer side. Overwrites nothing: when full, the newest frames drop.
    fn write(&self, src: impl Iterator<Item = f32>) {
        let read = self.read_frame.load(Ordering::Acquire);
        let mut write = self.write_frame.load(Ordering::Relaxed);
        for sample in src {
            if write - read >= self.frames {
                break;
            }
            unsafe { *self.samples[(write % self.frames) as usize].get() = sample };
            write += 1;
        }
        self.write_frame.store(write, Ordering::Release);
    }

    /// Consumer side: fill `dst`, padding with silence. Keeps the latency
    /// bounded by skipping the oldest audio once more than `keep` frames
    /// beyond this read are waiting (clock drift against the graph driver).
    fn read(&self, dst: &mut [f32], keep: u64) {
        let write = self.write_frame.load(Ordering::Acquire);
        let mut read = self.read_frame.load(Ordering::Relaxed);
        let excess = (write - read).saturating_sub(dst.len() as u64 + keep);
        read += excess;
        for sample in dst.iter_mut() {
            if read < write {
                *sample = unsafe { *self.samples[(read % self.frames) as usize].get() };
                read += 1;
            } else {
                *sample = 0.0;
            }
        }
        self.read_frame.store(read, Ordering::Release);
    }
}

// ----------------------------------------------------------------------------
// Camera frames: orientation and pixel conversion
// ----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PixelFormat {
    Yuy2,
    Nv12,
    I420,
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
impl PixelFormat {
    const ALL: [PixelFormat; 3] = [PixelFormat::Yuy2, PixelFormat::Nv12, PixelFormat::I420];

    fn frame_size(self, width: usize, height: usize) -> usize {
        match self {
            PixelFormat::Yuy2 => width * height * 2,
            PixelFormat::Nv12 | PixelFormat::I420 => width * height * 3 / 2,
        }
    }

    fn stride(self, width: usize) -> usize {
        match self {
            PixelFormat::Yuy2 => width * 2,
            PixelFormat::Nv12 | PixelFormat::I420 => width,
        }
    }
}

/// Clockwise rotation that turns the sensor image upright for the current
/// display rotation (Camera2's documented formula; front cameras are not
/// mirrored, like a desktop webcam).
fn upright_rotation(sensor_orientation: u32, display_rotation: u32, front: bool) -> u32 {
    if front {
        (sensor_orientation + display_rotation) % 360
    } else {
        (sensor_orientation + 360 - display_rotation % 360) % 360
    }
}

/// A YUV_420_888 image as AImage exposes it.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct Yuv420<'a> {
    width: usize,
    height: usize,
    y: &'a [u8],
    y_row: usize,
    y_pixel: usize,
    u: &'a [u8],
    v: &'a [u8],
    uv_row: usize,
    uv_pixel: usize,
}

impl Yuv420<'_> {
    #[inline]
    fn luma(&self, x: usize, y: usize) -> u8 {
        self.y.get(y * self.y_row + x * self.y_pixel).copied().unwrap_or(16)
    }

    #[inline]
    fn chroma(&self, x: usize, y: usize) -> (u8, u8) {
        let at = (y / 2) * self.uv_row + (x / 2) * self.uv_pixel;
        (
            self.u.get(at).copied().unwrap_or(128),
            self.v.get(at).copied().unwrap_or(128),
        )
    }
}

/// Where each output pixel samples the source: the rotated image scaled to
/// fit the output and centered (black bars when the aspect differs, e.g. a
/// portrait-held tablet in a landscape stream). Nearest neighbour.
struct Mapping {
    key: (usize, usize, usize, usize, u32),
    /// Per output column / row: index into the rotated image, or None (bar).
    cols: Vec<Option<usize>>,
    rows: Vec<Option<usize>>,
    rotation: u32,
    src_w: usize,
    src_h: usize,
}

impl Mapping {
    fn new(src_w: usize, src_h: usize, out_w: usize, out_h: usize, rotation: u32) -> Mapping {
        let (rot_w, rot_h) = if rotation % 180 == 90 { (src_h, src_w) } else { (src_w, src_h) };
        // Fit (rot_w × rot_h) inside (out_w × out_h), even sizes and offsets
        // so chroma pairs stay aligned.
        let (fit_w, fit_h) = if rot_w * out_h > rot_h * out_w {
            (out_w, (rot_h * out_w / rot_w.max(1)) & !1)
        } else {
            ((rot_w * out_h / rot_h.max(1)) & !1, out_h)
        };
        let (fit_w, fit_h) = (fit_w.max(2), fit_h.max(2));
        let off_x = ((out_w - fit_w.min(out_w)) / 2) & !1;
        let off_y = ((out_h - fit_h.min(out_h)) / 2) & !1;
        let axis = |out: usize, off: usize, fit: usize, rot: usize| -> Vec<Option<usize>> {
            (0..out)
                .map(|i| {
                    (i >= off && i < off + fit).then(|| ((i - off) * rot / fit).min(rot - 1))
                })
                .collect()
        };
        Mapping {
            key: (src_w, src_h, out_w, out_h, rotation),
            cols: axis(out_w, off_x, fit_w, rot_w),
            rows: axis(out_h, off_y, fit_h, rot_h),
            rotation,
            src_w,
            src_h,
        }
    }

    /// Source pixel for rotated-image coordinates.
    #[inline]
    fn source(&self, rx: usize, ry: usize) -> (usize, usize) {
        match self.rotation {
            90 => (ry, self.src_h - 1 - rx),
            180 => (self.src_w - 1 - rx, self.src_h - 1 - ry),
            270 => (self.src_w - 1 - ry, rx),
            _ => (rx, ry),
        }
    }

    #[inline]
    fn sample(&self, x: usize, y: usize) -> Option<(usize, usize)> {
        Some(self.source(self.cols[x]?, self.rows[y]?))
    }

    fn is_identity(&self) -> bool {
        let (src_w, src_h, out_w, out_h, rotation) = self.key;
        rotation == 0 && src_w == out_w && src_h == out_h
    }
}

const BLACK_Y: u8 = 16;
const BLACK_C: u8 = 128;

fn fill_black(format: PixelFormat, width: usize, height: usize, dst: &mut [u8]) {
    let luma = width * height;
    match format {
        PixelFormat::Yuy2 => {
            for pair in dst.chunks_exact_mut(4) {
                pair.copy_from_slice(&[BLACK_Y, BLACK_C, BLACK_Y, BLACK_C]);
            }
        }
        PixelFormat::Nv12 | PixelFormat::I420 => {
            let end = luma.min(dst.len());
            dst[..end].fill(BLACK_Y);
            dst[end..].fill(BLACK_C);
        }
    }
}

/// Convert one camera image into `dst` (`format`, `out_w` × `out_h`).
fn convert(src: &Yuv420, map: &Mapping, format: PixelFormat, out_w: usize, out_h: usize, dst: &mut [u8]) {
    let luma = out_w * out_h;
    if dst.len() < format.frame_size(out_w, out_h) {
        return;
    }
    match format {
        PixelFormat::Yuy2 => {
            for y in 0..out_h {
                let row = &mut dst[y * out_w * 2..(y + 1) * out_w * 2];
                for x in (0..out_w).step_by(2) {
                    let (y0, y1, u, v) = match (map.sample(x, y), map.sample(x + 1, y)) {
                        (Some(a), b) => {
                            let (u, v) = src.chroma(a.0, a.1);
                            let y1 = b.map_or(BLACK_Y, |b| src.luma(b.0, b.1));
                            (src.luma(a.0, a.1), y1, u, v)
                        }
                        (None, Some(b)) => {
                            let (u, v) = src.chroma(b.0, b.1);
                            (BLACK_Y, src.luma(b.0, b.1), u, v)
                        }
                        (None, None) => (BLACK_Y, BLACK_Y, BLACK_C, BLACK_C),
                    };
                    row[x * 2..x * 2 + 4].copy_from_slice(&[y0, u, y1, v]);
                }
            }
        }
        PixelFormat::Nv12 | PixelFormat::I420 => {
            let (y_plane, chroma) = dst.split_at_mut(luma);
            if map.is_identity() && src.y_pixel == 1 {
                for y in 0..out_h {
                    let from = y * src.y_row;
                    let line = &mut y_plane[y * out_w..(y + 1) * out_w];
                    match src.y.get(from..from + out_w) {
                        Some(source) => line.copy_from_slice(source),
                        None => line.fill(BLACK_Y),
                    }
                }
            } else {
                for y in 0..out_h {
                    let line = &mut y_plane[y * out_w..(y + 1) * out_w];
                    for (x, out) in line.iter_mut().enumerate() {
                        *out = map.sample(x, y).map_or(BLACK_Y, |(sx, sy)| src.luma(sx, sy));
                    }
                }
            }
            let (cw, ch) = (out_w / 2, out_h / 2);
            let (u_plane, v_plane) = chroma.split_at_mut(cw * ch);
            for cy in 0..ch {
                for cx in 0..cw {
                    let (u, v) = map
                        .sample(cx * 2, cy * 2)
                        .map_or((BLACK_C, BLACK_C), |(sx, sy)| src.chroma(sx, sy));
                    if format == PixelFormat::I420 {
                        u_plane[cy * cw + cx] = u;
                        v_plane[cy * cw + cx] = v;
                    } else {
                        let at = (cy * cw + cx) * 2;
                        chroma_pair(u_plane, v_plane, at, u, v);
                    }
                }
            }
        }
    }
}

/// NV12's interleaved UV plane spans both halves `convert` split it into.
#[inline]
fn chroma_pair(first: &mut [u8], second: &mut [u8], at: usize, u: u8, v: u8) {
    let put = |first: &mut [u8], second: &mut [u8], i: usize, value: u8| {
        if i < first.len() {
            first[i] = value;
        } else if let Some(slot) = second.get_mut(i - first.len()) {
            *slot = value;
        }
    };
    put(first, second, at, u);
    put(first, second, at + 1, v);
}

/// The stream sizes Portal offers, in preference order, from what the camera
/// can deliver as YUV_420_888.
fn offered_sizes(available: &[(u32, u32)]) -> Vec<(u32, u32)> {
    const PREFERRED: [(u32, u32); 4] = [(1280, 720), (640, 480), (1920, 1080), (320, 240)];
    let mut sizes: Vec<_> = PREFERRED
        .iter()
        .copied()
        .filter(|size| available.contains(size))
        .collect();
    if sizes.is_empty() {
        if let Some(best) = available
            .iter()
            .copied()
            .filter(|&(w, h)| w <= 1920 && h <= 1080 && w % 2 == 0 && h % 2 == 0)
            .max_by_key(|&(w, h)| w * h)
        {
            sizes.push(best);
        }
    }
    sizes
}

// ----------------------------------------------------------------------------
// Arguments
// ----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Mic,
    Camera,
}

struct Args {
    mode: Mode,
    package: Option<String>,
    rate: u32,
    /// Input preset name for the mic (`generic`, `voice-recognition`,
    /// `communication`, `unprocessed`).
    preset: String,
}

fn usage() {
    eprintln!(
        "Usage: localdesktop-pipewire-capture --mic|--camera [--package NAME] [--rate HZ] [--preset generic|voice-recognition|communication|unprocessed]"
    );
}

fn parse_args(argv: &[String]) -> Result<Option<Args>, String> {
    let mut mode = None;
    let mut args = Args {
        mode: Mode::Mic,
        package: None,
        rate: 48000,
        preset: "voice-recognition".into(),
    };
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--help" | "-h" => return Ok(None),
            "--mic" => mode = Some(Mode::Mic),
            "--camera" => mode = Some(Mode::Camera),
            flag @ ("--package" | "--rate" | "--preset") => {
                let value = argv.get(i + 1).ok_or_else(|| format!("{flag} needs a value"))?;
                match flag {
                    "--package" => args.package = Some(value.clone()),
                    "--rate" => {
                        args.rate = value
                            .parse()
                            .ok()
                            .filter(|&n: &u32| n != 0)
                            .ok_or_else(|| format!("invalid value for --rate: {value}"))?
                    }
                    _ => args.preset = value.clone(),
                }
                i += 1;
            }
            other => return Err(format!("unknown argument {other}")),
        }
        i += 1;
    }
    args.mode = mode.ok_or("one of --mic or --camera is required")?;
    Ok(Some(args))
}

// ----------------------------------------------------------------------------
// Android: AAudio input, NDK Camera2 and the PipeWire nodes
// ----------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod android {
    use super::*;

    use std::ffi::{c_char, c_void, CStr, CString};
    use std::io::Cursor;
    use std::sync::atomic::{AtomicI64, AtomicPtr, AtomicUsize};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use libloading::Library;
    use pipewire as pw;
    use pw::spa;

    unsafe fn sym<T: Copy>(lib: &Library, name: &[u8]) -> Result<T, String> {
        lib.get::<T>(name).map(|s| *s).map_err(|e| {
            format!("missing symbol {}: {e}", String::from_utf8_lossy(&name[..name.len() - 1]))
        })
    }

    unsafe fn opt_sym<T: Copy>(lib: &Library, name: &[u8]) -> Option<T> {
        lib.get::<T>(name).ok().map(|s| *s)
    }

    fn monotonic_ns() -> i64 {
        let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
        now.tv_sec * 1_000_000_000 + now.tv_nsec
    }

    // -- SPA pods ------------------------------------------------------------

    fn pod_bytes(value: &spa::pod::Value) -> Vec<u8> {
        spa::pod::serialize::PodSerializer::serialize(Cursor::new(Vec::new()), value)
            .expect("serialize pod")
            .0
            .into_inner()
    }

    fn prop(key: u32, value: spa::pod::Value) -> spa::pod::Property {
        spa::pod::Property {
            key,
            flags: spa::pod::PropertyFlags::empty(),
            value,
        }
    }

    fn int_range(default: i32, min: i32, max: i32) -> spa::pod::Value {
        spa::pod::Value::Choice(spa::pod::ChoiceValue::Int(spa::utils::Choice(
            spa::utils::ChoiceFlags::empty(),
            spa::utils::ChoiceEnum::Range { default, min, max },
        )))
    }

    fn meta_header_pod() -> Vec<u8> {
        pod_bytes(&spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamMeta.as_raw(),
            id: spa::param::ParamType::Meta.as_raw(),
            properties: vec![
                prop(
                    spa::sys::SPA_PARAM_META_type,
                    spa::pod::Value::Id(spa::utils::Id(spa::sys::SPA_META_Header)),
                ),
                prop(
                    spa::sys::SPA_PARAM_META_size,
                    spa::pod::Value::Int(std::mem::size_of::<spa::sys::spa_meta_header>() as i32),
                ),
            ],
        }))
    }

    fn update_params(stream: &pw::stream::Stream, pods: &[Vec<u8>]) {
        let mut params: Vec<&spa::pod::Pod> =
            pods.iter().filter_map(|bytes| spa::pod::Pod::from_bytes(bytes)).collect();
        if let Err(e) = stream.update_params(&mut params) {
            note!("failed to update stream params: {e}");
        }
    }

    /// Whatever the setup must keep alive while the loop runs.
    trait Keep {}
    impl<T> Keep for T {}

    fn run_loop_with(
        setup: impl for<'a> FnOnce(&'a pw::main_loop::MainLoopRc, &pw::core::CoreRc) -> Result<Box<dyn Keep + 'a>, String>,
    ) -> Result<(), String> {
        pw::init();
        let result = (|| {
            let mainloop = pw::main_loop::MainLoopRc::new(None)
                .map_err(|e| format!("failed to create PipeWire main loop: {e}"))?;
            let quit = {
                let mainloop = mainloop.clone();
                move || mainloop.quit()
            };
            let _sigint = mainloop.loop_().add_signal_local(pw::loop_::Signal::INT, quit.clone());
            let _sigterm = mainloop.loop_().add_signal_local(pw::loop_::Signal::TERM, quit);
            let context = pw::context::ContextRc::new(&mainloop, None)
                .map_err(|e| format!("failed to create PipeWire context: {e}"))?;
            let core = context
                .connect_rc(None)
                .map_err(|e| format!("failed to connect to PipeWire: {e}"))?;
            let _keep = setup(&mainloop, &core)?;
            mainloop.run();
            Ok(())
        })();
        unsafe { pw::deinit() };
        result
    }

    // -- Microphone ----------------------------------------------------------

    mod aaudio {
        use super::*;

        pub type Res = i32;
        pub enum Builder {}
        pub enum Stream {}

        pub const OK: Res = 0;
        pub const DIRECTION_INPUT: i32 = 1;
        pub const FORMAT_PCM_FLOAT: i32 = 2;
        pub const PERFORMANCE_MODE_LOW_LATENCY: i32 = 12;
        pub const SHARING_MODE_SHARED: i32 = 1;
        pub const CALLBACK_RESULT_CONTINUE: i32 = 0;

        pub type DataCallback = unsafe extern "C" fn(*mut Stream, *mut c_void, *mut c_void, i32) -> i32;
        pub type ErrorCallback = unsafe extern "C" fn(*mut Stream, *mut c_void, Res);

        pub struct Api {
            _lib: Library,
            pub result_text: unsafe extern "C" fn(Res) -> *const c_char,
            pub create_builder: unsafe extern "C" fn(*mut *mut Builder) -> Res,
            pub builder_delete: unsafe extern "C" fn(*mut Builder),
            pub set_direction: unsafe extern "C" fn(*mut Builder, i32),
            pub set_format: unsafe extern "C" fn(*mut Builder, i32),
            pub set_performance_mode: unsafe extern "C" fn(*mut Builder, i32),
            pub set_sharing_mode: unsafe extern "C" fn(*mut Builder, i32),
            pub set_sample_rate: unsafe extern "C" fn(*mut Builder, i32),
            pub set_channel_count: unsafe extern "C" fn(*mut Builder, i32),
            pub set_input_preset: Option<unsafe extern "C" fn(*mut Builder, i32)>,
            pub set_package_name: Option<unsafe extern "C" fn(*mut Builder, *const c_char)>,
            pub set_data_callback: unsafe extern "C" fn(*mut Builder, DataCallback, *mut c_void),
            pub set_error_callback: unsafe extern "C" fn(*mut Builder, ErrorCallback, *mut c_void),
            pub open_stream: unsafe extern "C" fn(*mut Builder, *mut *mut Stream) -> Res,
            pub sample_rate: unsafe extern "C" fn(*mut Stream) -> i32,
            pub channel_count: unsafe extern "C" fn(*mut Stream) -> i32,
            pub request_start: unsafe extern "C" fn(*mut Stream) -> Res,
            pub request_stop: unsafe extern "C" fn(*mut Stream) -> Res,
            pub close: unsafe extern "C" fn(*mut Stream) -> Res,
        }

        impl Api {
            pub fn load() -> Result<Api, String> {
                unsafe {
                    let lib = Library::new("libaaudio.so")
                        .map_err(|e| format!("failed to dlopen libaaudio.so: {e}"))?;
                    Ok(Api {
                        result_text: sym(&lib, b"AAudio_convertResultToText\0")?,
                        create_builder: sym(&lib, b"AAudio_createStreamBuilder\0")?,
                        builder_delete: sym(&lib, b"AAudioStreamBuilder_delete\0")?,
                        set_direction: sym(&lib, b"AAudioStreamBuilder_setDirection\0")?,
                        set_format: sym(&lib, b"AAudioStreamBuilder_setFormat\0")?,
                        set_performance_mode: sym(&lib, b"AAudioStreamBuilder_setPerformanceMode\0")?,
                        set_sharing_mode: sym(&lib, b"AAudioStreamBuilder_setSharingMode\0")?,
                        set_sample_rate: sym(&lib, b"AAudioStreamBuilder_setSampleRate\0")?,
                        set_channel_count: sym(&lib, b"AAudioStreamBuilder_setChannelCount\0")?,
                        set_input_preset: opt_sym(&lib, b"AAudioStreamBuilder_setInputPreset\0"),
                        set_package_name: opt_sym(&lib, b"AAudioStreamBuilder_setPackageName\0"),
                        set_data_callback: sym(&lib, b"AAudioStreamBuilder_setDataCallback\0")?,
                        set_error_callback: sym(&lib, b"AAudioStreamBuilder_setErrorCallback\0")?,
                        open_stream: sym(&lib, b"AAudioStreamBuilder_openStream\0")?,
                        sample_rate: sym(&lib, b"AAudioStream_getSampleRate\0")?,
                        channel_count: sym(&lib, b"AAudioStream_getChannelCount\0")?,
                        request_start: sym(&lib, b"AAudioStream_requestStart\0")?,
                        request_stop: sym(&lib, b"AAudioStream_requestStop\0")?,
                        close: sym(&lib, b"AAudioStream_close\0")?,
                        _lib: lib,
                    })
                }
            }
        }

        /// AAUDIO_INPUT_PRESET_* by name.
        pub fn preset(name: &str) -> i32 {
            match name {
                "generic" => 1,
                "communication" => 7,
                "unprocessed" => 9,
                _ => 6, // voice recognition: the least processed default path
            }
        }
    }

    struct Mic {
        api: aaudio::Api,
        ring: Ring,
        rate: u32,
        preset: i32,
        package: Option<CString>,
        /// Channels of the open AAudio stream, read by its callback.
        channels: AtomicUsize,
        stream: AtomicPtr<aaudio::Stream>,
        failed: AtomicBool,
        streaming: AtomicBool,
    }

    static MIC: OnceLock<Mic> = OnceLock::new();

    unsafe extern "C" fn mic_data_callback(
        _stream: *mut aaudio::Stream,
        _userdata: *mut c_void,
        audio_data: *mut c_void,
        num_frames: i32,
    ) -> i32 {
        let Some(mic) = MIC.get() else {
            return aaudio::CALLBACK_RESULT_CONTINUE;
        };
        let channels = mic.channels.load(Ordering::Acquire).max(1);
        let frames = num_frames.max(0) as usize;
        let src = std::slice::from_raw_parts(audio_data as *const f32, frames * channels);
        mic.ring.write(
            src.chunks_exact(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32),
        );
        aaudio::CALLBACK_RESULT_CONTINUE
    }

    unsafe extern "C" fn mic_error_callback(_stream: *mut aaudio::Stream, _userdata: *mut c_void, error: aaudio::Res) {
        if let Some(mic) = MIC.get() {
            let text = CStr::from_ptr((mic.api.result_text)(error)).to_string_lossy();
            note!("AAudio input error: {text}");
            mic.failed.store(true, Ordering::Release);
        }
    }

    impl Mic {
        fn is_open(&self) -> bool {
            !self.stream.load(Ordering::Acquire).is_null()
        }

        fn open(&self) -> Result<(), String> {
            let api = &self.api;
            unsafe {
                let mut builder: *mut aaudio::Builder = std::ptr::null_mut();
                if (api.create_builder)(&mut builder) != aaudio::OK {
                    return Err("AAudio_createStreamBuilder failed".into());
                }
                (api.set_direction)(builder, aaudio::DIRECTION_INPUT);
                (api.set_format)(builder, aaudio::FORMAT_PCM_FLOAT);
                (api.set_performance_mode)(builder, aaudio::PERFORMANCE_MODE_LOW_LATENCY);
                (api.set_sharing_mode)(builder, aaudio::SHARING_MODE_SHARED);
                (api.set_sample_rate)(builder, self.rate as i32);
                (api.set_channel_count)(builder, 1);
                if let Some(set_input_preset) = api.set_input_preset {
                    set_input_preset(builder, self.preset);
                }
                if let (Some(set_package_name), Some(package)) = (api.set_package_name, &self.package) {
                    set_package_name(builder, package.as_ptr());
                }
                (api.set_data_callback)(builder, mic_data_callback, std::ptr::null_mut());
                (api.set_error_callback)(builder, mic_error_callback, std::ptr::null_mut());
                let mut stream: *mut aaudio::Stream = std::ptr::null_mut();
                let res = (api.open_stream)(builder, &mut stream);
                (api.builder_delete)(builder);
                if res != aaudio::OK {
                    let text = CStr::from_ptr((api.result_text)(res)).to_string_lossy();
                    return Err(format!("AAudio input open failed: {text}"));
                }
                let rate = (api.sample_rate)(stream);
                let channels = (api.channel_count)(stream).max(1) as usize;
                if rate as u32 != self.rate {
                    // The PipeWire node runs at self.rate; a different
                    // device rate would play back at the wrong pitch.
                    (api.close)(stream);
                    return Err(format!("AAudio input opened at {rate} Hz, not {}", self.rate));
                }
                self.channels.store(channels, Ordering::Release);
                self.ring.clear();
                self.failed.store(false, Ordering::Release);
                if (api.request_start)(stream) != aaudio::OK {
                    (api.close)(stream);
                    return Err("AAudio input start failed".into());
                }
                self.stream.store(stream, Ordering::Release);
                note!("microphone open: rate={rate} channels={channels}");
            }
            Ok(())
        }

        fn close(&self) {
            let stream = self.stream.swap(std::ptr::null_mut(), Ordering::AcqRel);
            if stream.is_null() {
                return;
            }
            unsafe {
                (self.api.request_stop)(stream);
                (self.api.close)(stream);
            }
            self.ring.clear();
            note!("microphone closed");
        }
    }

    fn audio_format_pod(rate: u32) -> Vec<u8> {
        let mut info = spa::param::audio::AudioInfoRaw::new();
        info.set_format(spa::param::audio::AudioFormat::F32LE);
        info.set_rate(rate);
        info.set_channels(1);
        let mut position = [0u32; spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
        position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
        info.set_position(position);
        pod_bytes(&spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties: info.into(),
        }))
    }

    fn audio_buffers_pod(rate: u32) -> Vec<u8> {
        let frame = std::mem::size_of::<f32>() as i32;
        pod_bytes(&spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
            id: spa::param::ParamType::Buffers.as_raw(),
            properties: vec![
                prop(spa::sys::SPA_PARAM_BUFFERS_buffers, int_range(8, 2, 16)),
                prop(spa::sys::SPA_PARAM_BUFFERS_blocks, spa::pod::Value::Int(1)),
                prop(
                    spa::sys::SPA_PARAM_BUFFERS_size,
                    int_range((rate / 100).max(256) as i32 * frame, frame * 256, frame * 8192),
                ),
                prop(spa::sys::SPA_PARAM_BUFFERS_stride, spa::pod::Value::Int(frame)),
            ],
        }))
    }

    fn run_mic(args: &Args) -> Result<(), String> {
        let api = aaudio::Api::load()?;
        let _ = MIC.set(Mic {
            api,
            // 500 ms: the ring only ever holds a few quanta.
            ring: Ring::new(args.rate as u64 / 2),
            rate: args.rate,
            preset: aaudio::preset(&args.preset),
            package: args.package.as_deref().and_then(|p| CString::new(p).ok()),
            channels: AtomicUsize::new(1),
            stream: AtomicPtr::new(std::ptr::null_mut()),
            failed: AtomicBool::new(false),
            streaming: AtomicBool::new(false),
        });
        let mic: &'static Mic = MIC.get().unwrap();
        let rate = args.rate;

        run_loop_with(move |mainloop, core| {
            let props = pw::properties::properties! {
                *pw::keys::MEDIA_CLASS => "Audio/Source",
                *pw::keys::NODE_NAME => "portal-microphone",
                *pw::keys::NODE_DESCRIPTION => "Portal Microphone",
                *pw::keys::MEDIA_ROLE => "Communication",
                *pw::keys::AUDIO_RATE => rate.to_string(),
                *pw::keys::AUDIO_CHANNELS => "1",
                "audio.position" => "MONO",
                "device.icon-name" => "audio-input-microphone",
                // Above any sink monitor, so it is the default recording
                // device: without it WirePlumber picked the speaker monitor
                // and apps recording "default" (Chromium, Electron) got that.
                *pw::keys::PRIORITY_SESSION => "2000",
            };
            let stream = pw::stream::StreamRc::new(core.clone(), "portal-microphone", props)
                .map_err(|e| format!("failed to create PipeWire stream: {e}"))?;
            let listener = stream
                .add_local_listener_with_user_data(mic)
                .state_changed(|_stream, mic, old, new| {
                    mic.streaming
                        .store(new == pw::stream::StreamState::Streaming, Ordering::Release);
                    note!("microphone stream {old:?} -> {new:?}");
                })
                .param_changed(move |stream, _mic, id, param| {
                    if param.is_some() && id == spa::param::ParamType::Format.as_raw() {
                        update_params(stream, &[audio_buffers_pod(rate)]);
                    }
                })
                .process(move |stream, mic| {
                    let Some(mut buffer) = stream.dequeue_buffer() else { return };
                    let requested = buffer.requested() as usize;
                    if let Some(data) = buffer.datas_mut().first_mut() {
                        let Some(bytes) = data.data() else { return };
                        let capacity = bytes.len() / 4;
                        let frames = if requested > 0 { requested.min(capacity) } else { capacity.min(1024) };
                        let samples = unsafe {
                            std::slice::from_raw_parts_mut(bytes.as_mut_ptr() as *mut f32, frames)
                        };
                        // Keep at most ~2 quanta queued (40 ms floor).
                        mic.ring.read(samples, (frames as u64 * 2).max(rate as u64 / 25));
                        let chunk = data.chunk_mut();
                        *chunk.offset_mut() = 0;
                        *chunk.stride_mut() = 4;
                        *chunk.size_mut() = (frames * 4) as u32;
                    }
                })
                .register()
                .map_err(|e| format!("failed to register stream listener: {e}"))?;

            let format = audio_format_pod(rate);
            let mut params = [spa::pod::Pod::from_bytes(&format).ok_or("bad EnumFormat pod")?];
            stream
                .connect(
                    spa::utils::Direction::Output,
                    None,
                    pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS,
                    &mut params,
                )
                .map_err(|e| format!("failed to connect PipeWire stream: {e}"))?;

            // Reconcile the Android device with what the guest wants, 4×/s.
            let state = std::cell::RefCell::new(Episode::default());
            let timer = mainloop.loop_().add_timer(move |_| {
                let streaming = mic.streaming.load(Ordering::Acquire);
                let mut state = state.borrow_mut();
                match state.tick(streaming, mic.is_open(), mic.failed.load(Ordering::Acquire)) {
                    Action::Open => {
                        if let Err(e) = mic.open() {
                            note!("{e}");
                            state.retry_at = Some(Instant::now() + Duration::from_secs(2));
                        }
                    }
                    Action::Close => mic.close(),
                    Action::Reopen => {
                        mic.close();
                        state.retry_at = Some(Instant::now() + Duration::from_secs(1));
                    }
                    Action::None => {}
                }
            });
            timer.update_timer(Some(Duration::from_millis(250)), Some(Duration::from_millis(250)));
            note!("running node=portal-microphone rate={rate}");
            Ok(Box::new((stream, listener, timer)))
        })
    }

    enum Action {
        None,
        Open,
        Close,
        Reopen,
    }

    /// One "a guest app is using this device" episode: asks for permission
    /// once when it starts, opens the device while allowed, and closes it
    /// 3 s after the guest stops (or at once when Portal leaves).
    #[derive(Default)]
    struct Episode {
        active: bool,
        idle_ticks: u32,
        retry_at: Option<Instant>,
    }

    impl Episode {
        fn tick(&mut self, streaming: bool, open: bool, failed: bool) -> Action {
            const IDLE_CLOSE_TICKS: u32 = 12;
            if streaming && !self.active {
                self.active = true;
                if PERMISSION.load(Ordering::Acquire) != PERMISSION_GRANTED {
                    note!("demand");
                }
            }
            if streaming {
                self.idle_ticks = 0;
            } else if self.active {
                self.idle_ticks += 1;
                if self.idle_ticks >= IDLE_CLOSE_TICKS {
                    self.active = false;
                }
            }
            let allowed = PERMISSION.load(Ordering::Acquire) == PERMISSION_GRANTED
                && !PAUSED.load(Ordering::Acquire);
            let want = self.active && allowed;
            if open && failed {
                return Action::Reopen;
            }
            if open && !want {
                return Action::Close;
            }
            if !open && want && self.retry_at.is_none_or(|at| Instant::now() >= at) {
                self.retry_at = None;
                return Action::Open;
            }
            Action::None
        }
    }

    // -- Cameras -------------------------------------------------------------

    mod ndk {
        use super::*;

        pub enum Manager {}
        pub enum Metadata {}
        pub enum Device {}
        pub enum Request {}
        pub enum OutputContainer {}
        pub enum SessionOutput {}
        pub enum OutputTarget {}
        pub enum Session {}
        pub enum Window {}
        pub enum ImageReader {}
        pub enum Image {}

        pub const OK: i32 = 0;
        pub const TAG_LENS_FACING: u32 = 0x8_0005;
        pub const TAG_SENSOR_ORIENTATION: u32 = 0xE_000E;
        pub const TAG_STREAM_CONFIGURATIONS: u32 = 0xD_000A;
        pub const TAG_AE_TARGET_FPS_RANGE: u32 = 0x1_0005;
        pub const TAG_AE_AVAILABLE_FPS_RANGES: u32 = 0x1_0014;
        pub const LENS_FACING_FRONT: u8 = 0;
        pub const LENS_FACING_EXTERNAL: u8 = 2;
        pub const FORMAT_YUV_420_888: i32 = 0x23;
        pub const TEMPLATE_RECORD: i32 = 3;

        #[repr(C)]
        pub struct IdList {
            pub num: i32,
            pub ids: *const *const c_char,
        }

        #[repr(C)]
        pub struct ConstEntry {
            pub tag: u32,
            pub type_: u8,
            pub count: u32,
            pub data: *const c_void,
        }

        #[repr(C)]
        pub struct DeviceCallbacks {
            pub context: *mut c_void,
            pub on_disconnected: unsafe extern "C" fn(*mut c_void, *mut Device),
            pub on_error: unsafe extern "C" fn(*mut c_void, *mut Device, i32),
        }

        #[repr(C)]
        pub struct SessionCallbacks {
            pub context: *mut c_void,
            pub on_closed: unsafe extern "C" fn(*mut c_void, *mut Session),
            pub on_ready: unsafe extern "C" fn(*mut c_void, *mut Session),
            pub on_active: unsafe extern "C" fn(*mut c_void, *mut Session),
        }

        #[repr(C)]
        pub struct ImageListener {
            pub context: *mut c_void,
            pub on_image_available: unsafe extern "C" fn(*mut c_void, *mut ImageReader),
        }

        pub struct Api {
            _camera: Library,
            _media: Library,
            pub manager_create: unsafe extern "C" fn() -> *mut Manager,
            pub get_camera_id_list: unsafe extern "C" fn(*mut Manager, *mut *mut IdList) -> i32,
            pub delete_camera_id_list: unsafe extern "C" fn(*mut IdList),
            pub get_characteristics: unsafe extern "C" fn(*mut Manager, *const c_char, *mut *mut Metadata) -> i32,
            pub metadata_get: unsafe extern "C" fn(*const Metadata, u32, *mut ConstEntry) -> i32,
            pub metadata_free: unsafe extern "C" fn(*mut Metadata),
            pub open_camera: unsafe extern "C" fn(*mut Manager, *const c_char, *mut DeviceCallbacks, *mut *mut Device) -> i32,
            pub device_close: unsafe extern "C" fn(*mut Device) -> i32,
            pub create_request: unsafe extern "C" fn(*const Device, i32, *mut *mut Request) -> i32,
            pub request_free: unsafe extern "C" fn(*mut Request),
            pub request_add_target: unsafe extern "C" fn(*mut Request, *const OutputTarget) -> i32,
            pub request_set_i32: unsafe extern "C" fn(*mut Request, u32, u32, *const i32) -> i32,
            pub container_create: unsafe extern "C" fn(*mut *mut OutputContainer) -> i32,
            pub container_free: unsafe extern "C" fn(*mut OutputContainer),
            pub container_add: unsafe extern "C" fn(*mut OutputContainer, *const SessionOutput) -> i32,
            pub output_create: unsafe extern "C" fn(*mut Window, *mut *mut SessionOutput) -> i32,
            pub output_free: unsafe extern "C" fn(*mut SessionOutput),
            pub target_create: unsafe extern "C" fn(*mut Window, *mut *mut OutputTarget) -> i32,
            pub target_free: unsafe extern "C" fn(*mut OutputTarget),
            pub create_session: unsafe extern "C" fn(*mut Device, *const OutputContainer, *const SessionCallbacks, *mut *mut Session) -> i32,
            pub session_close: unsafe extern "C" fn(*mut Session),
            pub set_repeating: unsafe extern "C" fn(*mut Session, *mut c_void, i32, *mut *mut Request, *mut i32) -> i32,
            pub reader_new: unsafe extern "C" fn(i32, i32, i32, i32, *mut *mut ImageReader) -> i32,
            pub reader_delete: unsafe extern "C" fn(*mut ImageReader),
            pub reader_get_window: unsafe extern "C" fn(*mut ImageReader, *mut *mut Window) -> i32,
            pub reader_set_listener: unsafe extern "C" fn(*mut ImageReader, *mut ImageListener) -> i32,
            pub reader_acquire_latest: unsafe extern "C" fn(*mut ImageReader, *mut *mut Image) -> i32,
            pub image_delete: unsafe extern "C" fn(*mut Image),
            pub image_width: unsafe extern "C" fn(*const Image, *mut i32) -> i32,
            pub image_height: unsafe extern "C" fn(*const Image, *mut i32) -> i32,
            pub image_plane_data: unsafe extern "C" fn(*const Image, i32, *mut *mut u8, *mut i32) -> i32,
            pub image_row_stride: unsafe extern "C" fn(*const Image, i32, *mut i32) -> i32,
            pub image_pixel_stride: unsafe extern "C" fn(*const Image, i32, *mut i32) -> i32,
        }

        /// Camera results and ImageReader frames arrive as binder calls. An
        /// app process gets binder threads from zygote; this executable has
        /// none, so without them the camera streams but no callback runs.
        unsafe fn start_binder_threads() {
            let Ok(lib) = Library::new("libbinder_ndk.so") else {
                note!("libbinder_ndk.so unavailable; camera callbacks may stall");
                return;
            };
            if let Ok(max) = lib.get::<unsafe extern "C" fn(u32) -> bool>(
                b"ABinderProcess_setThreadPoolMaxThreadCount\0",
            ) {
                max(4);
            }
            if let Ok(start) = lib.get::<unsafe extern "C" fn()>(b"ABinderProcess_startThreadPool\0") {
                start();
            }
            std::mem::forget(lib);
        }

        impl Api {
            pub fn load() -> Result<Api, String> {
                unsafe {
                    start_binder_threads();
                    let camera = Library::new("libcamera2ndk.so")
                        .map_err(|e| format!("failed to dlopen libcamera2ndk.so: {e}"))?;
                    let media = Library::new("libmediandk.so")
                        .map_err(|e| format!("failed to dlopen libmediandk.so: {e}"))?;
                    let c = &camera;
                    let m = &media;
                    Ok(Api {
                        manager_create: sym(c, b"ACameraManager_create\0")?,
                        get_camera_id_list: sym(c, b"ACameraManager_getCameraIdList\0")?,
                        delete_camera_id_list: sym(c, b"ACameraManager_deleteCameraIdList\0")?,
                        get_characteristics: sym(c, b"ACameraManager_getCameraCharacteristics\0")?,
                        metadata_get: sym(c, b"ACameraMetadata_getConstEntry\0")?,
                        metadata_free: sym(c, b"ACameraMetadata_free\0")?,
                        open_camera: sym(c, b"ACameraManager_openCamera\0")?,
                        device_close: sym(c, b"ACameraDevice_close\0")?,
                        create_request: sym(c, b"ACameraDevice_createCaptureRequest\0")?,
                        request_free: sym(c, b"ACaptureRequest_free\0")?,
                        request_add_target: sym(c, b"ACaptureRequest_addTarget\0")?,
                        request_set_i32: sym(c, b"ACaptureRequest_setEntry_i32\0")?,
                        container_create: sym(c, b"ACaptureSessionOutputContainer_create\0")?,
                        container_free: sym(c, b"ACaptureSessionOutputContainer_free\0")?,
                        container_add: sym(c, b"ACaptureSessionOutputContainer_add\0")?,
                        output_create: sym(c, b"ACaptureSessionOutput_create\0")?,
                        output_free: sym(c, b"ACaptureSessionOutput_free\0")?,
                        target_create: sym(c, b"ACameraOutputTarget_create\0")?,
                        target_free: sym(c, b"ACameraOutputTarget_free\0")?,
                        create_session: sym(c, b"ACameraDevice_createCaptureSession\0")?,
                        session_close: sym(c, b"ACameraCaptureSession_close\0")?,
                        set_repeating: sym(c, b"ACameraCaptureSession_setRepeatingRequest\0")?,
                        reader_new: sym(m, b"AImageReader_new\0")?,
                        reader_delete: sym(m, b"AImageReader_delete\0")?,
                        reader_get_window: sym(m, b"AImageReader_getWindow\0")?,
                        reader_set_listener: sym(m, b"AImageReader_setImageListener\0")?,
                        reader_acquire_latest: sym(m, b"AImageReader_acquireLatestImage\0")?,
                        image_delete: sym(m, b"AImage_delete\0")?,
                        image_width: sym(m, b"AImage_getWidth\0")?,
                        image_height: sym(m, b"AImage_getHeight\0")?,
                        image_plane_data: sym(m, b"AImage_getPlaneData\0")?,
                        image_row_stride: sym(m, b"AImage_getPlaneRowStride\0")?,
                        image_pixel_stride: sym(m, b"AImage_getPlanePixelStride\0")?,
                        _camera: camera,
                        _media: media,
                    })
                }
            }
        }
    }

    static CAMERA_API: OnceLock<ndk::Api> = OnceLock::new();

    fn api() -> &'static ndk::Api {
        CAMERA_API.get().expect("camera API loaded")
    }

    /// Negotiated stream format.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    struct StreamFormat {
        format: PixelFormat,
        width: usize,
        height: usize,
    }

    /// The newest converted frame, handed from the image thread to `process`.
    struct Frame {
        data: Vec<u8>,
        format: Option<StreamFormat>,
        map: Option<Mapping>,
        fresh: bool,
    }

    /// An open Android camera and its capture session.
    struct Opened {
        device: *mut ndk::Device,
        reader: *mut ndk::ImageReader,
        session: *mut ndk::Session,
        request: *mut ndk::Request,
        target: *mut ndk::OutputTarget,
        output: *mut ndk::SessionOutput,
        container: *mut ndk::OutputContainer,
        format: StreamFormat,
    }

    unsafe impl Send for Opened {}

    struct Camera {
        id: CString,
        node_name: String,
        description: String,
        front: bool,
        sensor_orientation: u32,
        sizes: Vec<(u32, u32)>,
        fps_range: [i32; 2],
        stream: AtomicPtr<pw::sys::pw_stream>,
        streaming: AtomicBool,
        format: Mutex<Option<StreamFormat>>,
        frame: Mutex<Frame>,
        opened: Mutex<Option<Opened>>,
        failed: AtomicBool,
        /// Live images are flowing (set by the image thread).
        live: AtomicBool,
        last_frame_ns: AtomicI64,
        sequence: AtomicU64,
        device_callbacks: UnsafeCell<ndk::DeviceCallbacks>,
        session_callbacks: UnsafeCell<ndk::SessionCallbacks>,
        listener: UnsafeCell<ndk::ImageListener>,
    }

    unsafe impl Send for Camera {}
    unsafe impl Sync for Camera {}

    unsafe extern "C" fn on_disconnected(context: *mut c_void, _device: *mut ndk::Device) {
        let camera = &*(context as *const Camera);
        note!("{}: camera disconnected", camera.node_name);
        camera.failed.store(true, Ordering::Release);
    }

    unsafe extern "C" fn on_error(context: *mut c_void, _device: *mut ndk::Device, error: i32) {
        let camera = &*(context as *const Camera);
        note!("{}: camera error {error}", camera.node_name);
        camera.failed.store(true, Ordering::Release);
    }

    unsafe extern "C" fn on_session_state(_context: *mut c_void, _session: *mut ndk::Session) {}

    unsafe extern "C" fn on_image_available(context: *mut c_void, reader: *mut ndk::ImageReader) {
        let camera = &*(context as *const Camera);
        let api = api();
        let mut image: *mut ndk::Image = std::ptr::null_mut();
        if (api.reader_acquire_latest)(reader, &mut image) != 0 || image.is_null() {
            return;
        }
        camera.take_image(image);
        (api.image_delete)(image);
        camera.live.store(true, Ordering::Release);
        camera.last_frame_ns.store(monotonic_ns(), Ordering::Release);
        camera.trigger();
    }

    impl Camera {
        fn trigger(&self) {
            let stream = self.stream.load(Ordering::Acquire);
            if !stream.is_null() && self.streaming.load(Ordering::Acquire) {
                unsafe { pw::sys::pw_stream_trigger_process(stream) };
            }
        }

        unsafe fn take_image(&self, image: *mut ndk::Image) {
            let api = api();
            let (mut width, mut height) = (0, 0);
            (api.image_width)(image, &mut width);
            (api.image_height)(image, &mut height);
            let plane = |index: i32| -> Option<(&[u8], usize, usize)> {
                let (mut data, mut len, mut row, mut pixel) = (std::ptr::null_mut(), 0, 0, 0);
                if (api.image_plane_data)(image, index, &mut data, &mut len) != 0 || data.is_null() {
                    return None;
                }
                (api.image_row_stride)(image, index, &mut row);
                (api.image_pixel_stride)(image, index, &mut pixel);
                Some((std::slice::from_raw_parts(data, len.max(0) as usize), row.max(0) as usize, pixel.max(1) as usize))
            };
            let (Some(y), Some(u), Some(v)) = (plane(0), plane(1), plane(2)) else { return };
            let src = Yuv420 {
                width: width.max(0) as usize,
                height: height.max(0) as usize,
                y: y.0,
                y_row: y.1,
                y_pixel: y.2,
                u: u.0,
                v: v.0,
                uv_row: u.1,
                uv_pixel: u.2,
            };
            let rotation = upright_rotation(self.sensor_orientation, ROTATION.load(Ordering::Acquire), self.front);
            let mut frame = self.frame.lock().unwrap_or_else(|e| e.into_inner());
            let Some(format) = frame.format else { return };
            let key = (src.width, src.height, format.width, format.height, rotation);
            if frame.map.as_ref().is_none_or(|map| map.key != key) {
                frame.map = Some(Mapping::new(src.width, src.height, format.width, format.height, rotation));
            }
            let Frame { data, map, fresh, .. } = &mut *frame;
            convert(&src, map.as_ref().unwrap(), format.format, format.width, format.height, data);
            *fresh = true;
        }

        /// Size the frame buffer for a (re)negotiated format, starting black.
        fn set_format(&self, format: Option<StreamFormat>) {
            *self.format.lock().unwrap_or_else(|e| e.into_inner()) = format;
            let mut frame = self.frame.lock().unwrap_or_else(|e| e.into_inner());
            frame.format = format;
            frame.map = None;
            if let Some(f) = format {
                frame.data.resize(f.format.frame_size(f.width, f.height), 0);
                fill_black(f.format, f.width, f.height, &mut frame.data);
                frame.fresh = true;
            }
        }

        fn is_open(&self) -> bool {
            self.opened.lock().map(|o| o.is_some()).unwrap_or(false)
        }

        fn open_format(&self) -> Option<StreamFormat> {
            self.opened.lock().ok().and_then(|o| o.as_ref().map(|o| o.format))
        }

        fn open(&'static self, manager: *mut ndk::Manager) -> Result<(), String> {
            let Some(format) = *self.format.lock().unwrap_or_else(|e| e.into_inner()) else {
                return Err("no negotiated format yet".into());
            };
            let api = api();
            let context = self as *const Camera as *mut c_void;
            unsafe {
                let mut opened = Opened {
                    device: std::ptr::null_mut(),
                    reader: std::ptr::null_mut(),
                    session: std::ptr::null_mut(),
                    request: std::ptr::null_mut(),
                    target: std::ptr::null_mut(),
                    output: std::ptr::null_mut(),
                    container: std::ptr::null_mut(),
                    format,
                };
                let result = (|| {
                    let status = (api.reader_new)(
                        format.width as i32,
                        format.height as i32,
                        ndk::FORMAT_YUV_420_888,
                        4,
                        &mut opened.reader,
                    );
                    if status != 0 {
                        return Err(format!("AImageReader_new failed: {status}"));
                    }
                    *self.listener.get() = ndk::ImageListener { context, on_image_available };
                    (api.reader_set_listener)(opened.reader, self.listener.get());
                    let mut window: *mut ndk::Window = std::ptr::null_mut();
                    (api.reader_get_window)(opened.reader, &mut window);
                    *self.device_callbacks.get() = ndk::DeviceCallbacks { context, on_disconnected, on_error };
                    let status = (api.open_camera)(manager, self.id.as_ptr(), self.device_callbacks.get(), &mut opened.device);
                    if status != ndk::OK {
                        return Err(format!("ACameraManager_openCamera failed: {status}"));
                    }
                    check((api.create_request)(opened.device, ndk::TEMPLATE_RECORD, &mut opened.request), "createCaptureRequest")?;
                    check((api.target_create)(window, &mut opened.target), "ACameraOutputTarget_create")?;
                    check((api.request_add_target)(opened.request, opened.target), "ACaptureRequest_addTarget")?;
                    (api.request_set_i32)(opened.request, ndk::TAG_AE_TARGET_FPS_RANGE, 2, self.fps_range.as_ptr());
                    check((api.container_create)(&mut opened.container), "ACaptureSessionOutputContainer_create")?;
                    check((api.output_create)(window, &mut opened.output), "ACaptureSessionOutput_create")?;
                    check((api.container_add)(opened.container, opened.output), "ACaptureSessionOutputContainer_add")?;
                    *self.session_callbacks.get() = ndk::SessionCallbacks {
                        context,
                        on_closed: on_session_state,
                        on_ready: on_session_state,
                        on_active: on_session_state,
                    };
                    check(
                        (api.create_session)(opened.device, opened.container, self.session_callbacks.get(), &mut opened.session),
                        "createCaptureSession",
                    )?;
                    let mut request = opened.request;
                    check(
                        (api.set_repeating)(opened.session, std::ptr::null_mut(), 1, &mut request, std::ptr::null_mut()),
                        "setRepeatingRequest",
                    )?;
                    Ok(())
                })();
                match result {
                    Ok(()) => {
                        self.failed.store(false, Ordering::Release);
                        note!(
                            "{}: camera open {}x{} {:?} fps {:?}",
                            self.node_name,
                            format.width,
                            format.height,
                            format.format,
                            self.fps_range
                        );
                        *self.opened.lock().unwrap_or_else(|e| e.into_inner()) = Some(opened);
                        Ok(())
                    }
                    Err(e) => {
                        release(opened);
                        Err(e)
                    }
                }
            }
        }

        fn close(&self) {
            let opened = self.opened.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(opened) = opened {
                self.live.store(false, Ordering::Release);
                unsafe { release(opened) };
                note!("{}: camera closed", self.node_name);
            }
        }
    }

    fn check(status: i32, what: &str) -> Result<(), String> {
        if status == ndk::OK {
            Ok(())
        } else {
            Err(format!("{what} failed: {status}"))
        }
    }

    /// Tear down in dependency order: session, device, then the objects the
    /// session referenced, and the image reader last.
    unsafe fn release(opened: Opened) {
        let api = api();
        if !opened.session.is_null() {
            (api.session_close)(opened.session);
        }
        if !opened.device.is_null() {
            (api.device_close)(opened.device);
        }
        if !opened.request.is_null() {
            (api.request_free)(opened.request);
        }
        if !opened.target.is_null() {
            (api.target_free)(opened.target);
        }
        if !opened.output.is_null() {
            (api.output_free)(opened.output);
        }
        if !opened.container.is_null() {
            (api.container_free)(opened.container);
        }
        if !opened.reader.is_null() {
            (api.reader_delete)(opened.reader);
        }
    }

    unsafe fn entry(api: &ndk::Api, metadata: *const ndk::Metadata, tag: u32) -> Option<ndk::ConstEntry> {
        let mut entry = ndk::ConstEntry {
            tag: 0,
            type_: 0,
            count: 0,
            data: std::ptr::null(),
        };
        ((api.metadata_get)(metadata, tag, &mut entry) == ndk::OK && !entry.data.is_null()).then_some(entry)
    }

    /// Every camera Android lists, described from its characteristics. Needs
    /// no permission.
    fn enumerate(manager: *mut ndk::Manager) -> Vec<Camera> {
        let api = api();
        let mut cameras = Vec::new();
        unsafe {
            let mut list: *mut ndk::IdList = std::ptr::null_mut();
            if (api.get_camera_id_list)(manager, &mut list) != ndk::OK || list.is_null() {
                return cameras;
            }
            let ids = std::slice::from_raw_parts((*list).ids, (*list).num.max(0) as usize);
            let (mut fronts, mut backs, mut externals) = (0, 0, 0);
            for &id in ids {
                let mut metadata: *mut ndk::Metadata = std::ptr::null_mut();
                if (api.get_characteristics)(manager, id, &mut metadata) != ndk::OK || metadata.is_null() {
                    continue;
                }
                let facing = entry(api, metadata, ndk::TAG_LENS_FACING)
                    .map(|e| *(e.data as *const u8))
                    .unwrap_or(1);
                let orientation = entry(api, metadata, ndk::TAG_SENSOR_ORIENTATION)
                    .map(|e| *(e.data as *const i32))
                    .unwrap_or(0)
                    .rem_euclid(360) as u32;
                let mut available = Vec::new();
                if let Some(e) = entry(api, metadata, ndk::TAG_STREAM_CONFIGURATIONS) {
                    let values = std::slice::from_raw_parts(e.data as *const i32, e.count as usize);
                    for config in values.chunks_exact(4) {
                        // (format, width, height, is_input)
                        if config[0] == ndk::FORMAT_YUV_420_888 && config[3] == 0 {
                            available.push((config[1] as u32, config[2] as u32));
                        }
                    }
                }
                // Prefer a range topping out at 30 fps, as steady as allowed.
                let mut fps_range = [15, 30];
                if let Some(e) = entry(api, metadata, ndk::TAG_AE_AVAILABLE_FPS_RANGES) {
                    let values = std::slice::from_raw_parts(e.data as *const i32, e.count as usize);
                    if let Some(best) = values
                        .chunks_exact(2)
                        .filter(|r| r[1] == 30)
                        .max_by_key(|r| r[0])
                        .or_else(|| values.chunks_exact(2).filter(|r| r[1] <= 30).max_by_key(|r| (r[1], r[0])))
                    {
                        fps_range = [best[0], best[1]];
                    }
                }
                (api.metadata_free)(metadata);

                let sizes = offered_sizes(&available);
                if sizes.is_empty() {
                    continue;
                }
                let (kind, count) = match facing {
                    ndk::LENS_FACING_FRONT => ("front", &mut fronts),
                    ndk::LENS_FACING_EXTERNAL => ("external", &mut externals),
                    _ => ("back", &mut backs),
                };
                *count += 1;
                let suffix = if *count > 1 { format!("-{count}") } else { String::new() };
                let label = match kind {
                    "front" => "Front",
                    "external" => "External",
                    _ => "Back",
                };
                let id = CStr::from_ptr(id).to_owned();
                note!("camera id={id:?} {kind} sensor={orientation} sizes={sizes:?} fps={fps_range:?}");
                cameras.push(Camera {
                    id,
                    node_name: format!("portal-camera-{kind}{suffix}"),
                    description: format!(
                        "Portal {label} Camera{}",
                        if *count > 1 { format!(" {count}") } else { String::new() }
                    ),
                    front: facing == ndk::LENS_FACING_FRONT,
                    sensor_orientation: orientation,
                    sizes,
                    fps_range,
                    stream: AtomicPtr::new(std::ptr::null_mut()),
                    streaming: AtomicBool::new(false),
                    format: Mutex::new(None),
                    frame: Mutex::new(Frame {
                        data: Vec::new(),
                        format: None,
                        map: None,
                        fresh: false,
                    }),
                    opened: Mutex::new(None),
                    failed: AtomicBool::new(false),
                    live: AtomicBool::new(false),
                    last_frame_ns: AtomicI64::new(0),
                    sequence: AtomicU64::new(0),
                    device_callbacks: UnsafeCell::new(ndk::DeviceCallbacks {
                        context: std::ptr::null_mut(),
                        on_disconnected,
                        on_error,
                    }),
                    session_callbacks: UnsafeCell::new(ndk::SessionCallbacks {
                        context: std::ptr::null_mut(),
                        on_closed: on_session_state,
                        on_ready: on_session_state,
                        on_active: on_session_state,
                    }),
                    listener: UnsafeCell::new(ndk::ImageListener {
                        context: std::ptr::null_mut(),
                        on_image_available,
                    }),
                });
            }
            (api.delete_camera_id_list)(list);
        }
        // Front first: it becomes the default camera, like a laptop webcam.
        cameras.sort_by_key(|c| !c.front);
        cameras
    }

    fn video_format_pods(camera: &Camera) -> Vec<Vec<u8>> {
        let mut pods = Vec::new();
        for format in PixelFormat::ALL {
            let raw = match format {
                PixelFormat::Yuy2 => spa::sys::SPA_VIDEO_FORMAT_YUY2,
                PixelFormat::Nv12 => spa::sys::SPA_VIDEO_FORMAT_NV12,
                PixelFormat::I420 => spa::sys::SPA_VIDEO_FORMAT_I420,
            };
            for &(width, height) in &camera.sizes {
                pods.push(pod_bytes(&spa::pod::Value::Object(spa::pod::Object {
                    type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
                    id: spa::param::ParamType::EnumFormat.as_raw(),
                    properties: vec![
                        prop(
                            spa::sys::SPA_FORMAT_mediaType,
                            spa::pod::Value::Id(spa::utils::Id(spa::sys::SPA_MEDIA_TYPE_video)),
                        ),
                        prop(
                            spa::sys::SPA_FORMAT_mediaSubtype,
                            spa::pod::Value::Id(spa::utils::Id(spa::sys::SPA_MEDIA_SUBTYPE_raw)),
                        ),
                        prop(spa::sys::SPA_FORMAT_VIDEO_format, spa::pod::Value::Id(spa::utils::Id(raw))),
                        prop(
                            spa::sys::SPA_FORMAT_VIDEO_size,
                            spa::pod::Value::Rectangle(spa::utils::Rectangle { width, height }),
                        ),
                        prop(
                            spa::sys::SPA_FORMAT_VIDEO_framerate,
                            spa::pod::Value::Fraction(spa::utils::Fraction { num: 30, denom: 1 }),
                        ),
                    ],
                })));
            }
        }
        pods
    }

    fn video_buffers_pod(format: StreamFormat) -> Vec<u8> {
        pod_bytes(&spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
            id: spa::param::ParamType::Buffers.as_raw(),
            properties: vec![
                prop(spa::sys::SPA_PARAM_BUFFERS_buffers, int_range(4, 2, 8)),
                prop(spa::sys::SPA_PARAM_BUFFERS_blocks, spa::pod::Value::Int(1)),
                prop(
                    spa::sys::SPA_PARAM_BUFFERS_size,
                    spa::pod::Value::Int(format.format.frame_size(format.width, format.height) as i32),
                ),
                prop(
                    spa::sys::SPA_PARAM_BUFFERS_stride,
                    spa::pod::Value::Int(format.format.stride(format.width) as i32),
                ),
            ],
        }))
    }

    fn parse_video_format(param: &spa::pod::Pod) -> Option<StreamFormat> {
        let (media_type, media_subtype) = spa::param::format_utils::parse_format(param).ok()?;
        if media_type != spa::param::format::MediaType::Video
            || media_subtype != spa::param::format::MediaSubtype::Raw
        {
            return None;
        }
        let mut info = spa::param::video::VideoInfoRaw::new();
        info.parse(param).ok()?;
        let format = match info.format().as_raw() {
            spa::sys::SPA_VIDEO_FORMAT_YUY2 => PixelFormat::Yuy2,
            spa::sys::SPA_VIDEO_FORMAT_NV12 => PixelFormat::Nv12,
            spa::sys::SPA_VIDEO_FORMAT_I420 => PixelFormat::I420,
            _ => return None,
        };
        let size = info.size();
        Some(StreamFormat {
            format,
            width: size.width as usize & !1,
            height: size.height as usize & !1,
        })
    }

    /// Copy the newest frame into a PipeWire buffer and queue it.
    fn process_camera(stream: &pw::stream::Stream, camera: &Camera) {
        unsafe {
            let raw = stream.as_raw_ptr();
            let pw_buffer = pw::sys::pw_stream_dequeue_buffer(raw);
            if pw_buffer.is_null() {
                return;
            }
            let buffer = (*pw_buffer).buffer;
            if !buffer.is_null() && (*buffer).n_datas > 0 {
                let data = &mut *(*buffer).datas;
                let mut frame = camera.frame.lock().unwrap_or_else(|e| e.into_inner());
                if let (Some(format), false) = (frame.format, data.data.is_null()) {
                    let size = frame.data.len().min(data.maxsize as usize);
                    std::ptr::copy_nonoverlapping(frame.data.as_ptr(), data.data as *mut u8, size);
                    frame.fresh = false;
                    let chunk = &mut *data.chunk;
                    chunk.offset = 0;
                    chunk.size = size as u32;
                    chunk.stride = format.format.stride(format.width) as i32;
                    chunk.flags = 0;
                }
                drop(frame);
                let header = spa::sys::spa_buffer_find_meta_data(
                    buffer,
                    spa::sys::SPA_META_Header,
                    std::mem::size_of::<spa::sys::spa_meta_header>(),
                ) as *mut spa::sys::spa_meta_header;
                if !header.is_null() {
                    (*header).flags = 0;
                    (*header).offset = 0;
                    (*header).pts = monotonic_ns();
                    (*header).dts_offset = 0;
                    (*header).seq = camera.sequence.fetch_add(1, Ordering::Relaxed);
                }
            }
            pw::sys::pw_stream_queue_buffer(raw, pw_buffer);
        }
    }

    struct CameraManager(*mut ndk::Manager);
    unsafe impl Send for CameraManager {}

    fn run_camera() -> Result<(), String> {
        let _ = CAMERA_API.set(ndk::Api::load()?);
        let manager = unsafe { (api().manager_create)() };
        if manager.is_null() {
            return Err("ACameraManager_create failed".into());
        }
        // Right after Portal starts, the camera service can still report no
        // cameras to this process; ask again for a few seconds.
        let mut found = enumerate(manager);
        for _ in 0..10 {
            if !found.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
            found = enumerate(manager);
        }
        let cameras: Vec<&'static Camera> = found
            .into_iter()
            .map(|camera| &*Box::leak(Box::new(camera)))
            .collect();
        if cameras.is_empty() {
            // Nothing to publish; stay alive so the supervisor does not
            // restart this in a loop.
            note!("no usable cameras");
        }
        let manager = CameraManager(manager);

        run_loop_with(move |mainloop, core| {
            let mut keep: Vec<Box<dyn Keep>> = Vec::new();
            for (index, &camera) in cameras.iter().enumerate() {
                let props = pw::properties::properties! {
                    *pw::keys::MEDIA_CLASS => "Video/Source",
                    *pw::keys::MEDIA_ROLE => "Camera",
                    *pw::keys::NODE_NAME => camera.node_name.as_str(),
                    *pw::keys::NODE_DESCRIPTION => camera.description.as_str(),
                    *pw::keys::NODE_DRIVER => "true",
                    *pw::keys::PRIORITY_SESSION => (1000 - index as i32 * 10).to_string(),
                    "device.icon-name" => "camera-web",
                    "object.path" => format!("portal:camera:{}", camera.id.to_string_lossy()),
                };
                let stream = pw::stream::StreamRc::new(core.clone(), &camera.node_name, props)
                    .map_err(|e| format!("failed to create PipeWire stream: {e}"))?;
                let listener = stream
                    .add_local_listener_with_user_data(camera)
                    .state_changed(|_stream, camera, old, new| {
                        let streaming = new == pw::stream::StreamState::Streaming;
                        camera.streaming.store(streaming, Ordering::Release);
                        note!("{} stream {old:?} -> {new:?}", camera.node_name);
                        if streaming {
                            // Show something at once (black until live).
                            camera.trigger();
                        }
                    })
                    .param_changed(|stream, camera, id, param| {
                        if id != spa::param::ParamType::Format.as_raw() {
                            return;
                        }
                        let format = param.and_then(parse_video_format);
                        note!("{} format {format:?}", camera.node_name);
                        camera.set_format(format);
                        if let Some(format) = format {
                            update_params(stream, &[video_buffers_pod(format), meta_header_pod()]);
                        }
                    })
                    .process(|stream, camera| process_camera(stream, camera))
                    .register()
                    .map_err(|e| format!("failed to register stream listener: {e}"))?;
                // Connect without formats, then publish them: a stream whose
                // first EnumFormat is video gets wrapped in the video adapter,
                // which needs libspa-videoconvert (and its FFmpeg libraries)
                // on the Android side. The frames are already in the
                // negotiated format, so the plain node is all a camera needs.
                stream
                    .connect(
                        spa::utils::Direction::Output,
                        None,
                        pw::stream::StreamFlags::DRIVER | pw::stream::StreamFlags::MAP_BUFFERS,
                        &mut [],
                    )
                    .map_err(|e| format!("failed to connect PipeWire stream: {e}"))?;
                update_params(&stream, &video_format_pods(camera));
                camera.stream.store(stream.as_raw_ptr(), Ordering::Release);
                note!("running node={} ({})", camera.node_name, camera.description);
                keep.push(Box::new((stream, listener)));
            }

            // Reconcile each Android camera with its node, 10×/s. Without
            // live images (no permission yet, paused, opening) the node
            // still delivers black frames at ~5 fps so apps never stall.
            let episodes = std::cell::RefCell::new(
                cameras.iter().map(|_| Episode::default()).collect::<Vec<_>>(),
            );
            let manager = manager;
            let timer = mainloop.loop_().add_timer(move |_| {
                let mut episodes = episodes.borrow_mut();
                for (camera, episode) in cameras.iter().zip(episodes.iter_mut()) {
                    let streaming = camera.streaming.load(Ordering::Acquire);
                    let open = camera.is_open();
                    // A renegotiated size needs a new capture session.
                    let stale = open && camera.open_format() != *camera.format.lock().unwrap_or_else(|e| e.into_inner());
                    match episode.tick(streaming, open, camera.failed.load(Ordering::Acquire) || stale) {
                        Action::Open => {
                            if let Err(e) = camera.open(manager.0) {
                                note!("{}: {e}", camera.node_name);
                                episode.retry_at = Some(Instant::now() + Duration::from_secs(2));
                            }
                        }
                        Action::Close => camera.close(),
                        Action::Reopen => {
                            camera.close();
                            episode.retry_at = Some(Instant::now() + Duration::from_millis(if stale { 0 } else { 1000 }));
                        }
                        Action::None => {}
                    }
                    if streaming {
                        let quiet_ns = monotonic_ns() - camera.last_frame_ns.load(Ordering::Acquire);
                        if !camera.is_open() || quiet_ns > 500_000_000 {
                            if camera.live.swap(false, Ordering::AcqRel) {
                                let format = *camera.format.lock().unwrap_or_else(|e| e.into_inner());
                                camera.set_format(format);
                            }
                            if quiet_ns > 200_000_000 {
                                camera.last_frame_ns.store(monotonic_ns(), Ordering::Release);
                                camera.trigger();
                            }
                        }
                    }
                }
            });
            timer.update_timer(Some(Duration::from_millis(100)), Some(Duration::from_millis(100)));
            keep.push(Box::new(timer));
            Ok(Box::new(keep))
        })
    }

    pub fn run(args: Args) -> Result<(), String> {
        spawn_command_reader();
        match args.mode {
            Mode::Mic => run_mic(&args),
            Mode::Camera => run_camera(),
        }
    }
}

#[cfg(not(target_os = "android"))]
mod android {
    use super::Args;

    pub fn run(_args: Args) -> Result<(), String> {
        Err("localdesktop_pipewire_capture only runs on Android".into())
    }
}

fn main() -> std::process::ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&argv) {
        Err(e) => {
            note!("{e}");
            usage();
            std::process::ExitCode::from(2)
        }
        Ok(None) => {
            usage();
            std::process::ExitCode::SUCCESS
        }
        Ok(Some(args)) => match android::run(args) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(e) => {
                note!("{e}");
                std::process::ExitCode::FAILURE
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_need_a_mode() {
        let parse = |argv: &[&str]| parse_args(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(parse(&[]).is_err());
        let mic = parse(&["--mic", "--package", "app.polarbear"]).unwrap().unwrap();
        assert_eq!((mic.mode, mic.package.as_deref(), mic.rate), (Mode::Mic, Some("app.polarbear"), 48000));
        assert_eq!(parse(&["--camera"]).unwrap().unwrap().mode, Mode::Camera);
        assert!(parse(&["--mic", "--rate", "0"]).is_err());
        assert!(parse(&["--mic", "--bogus"]).is_err());
        assert!(parse(&["--help"]).unwrap().is_none());
    }

    #[test]
    fn commands_update_control_state() {
        apply_command("rotation 270");
        assert_eq!(ROTATION.load(Ordering::Acquire), 270);
        apply_command("rotation 450");
        assert_eq!(ROTATION.load(Ordering::Acquire), 90);
        apply_command("pause");
        assert!(PAUSED.load(Ordering::Acquire));
        apply_command("resume");
        assert!(!PAUSED.load(Ordering::Acquire));
    }

    #[test]
    fn ring_reads_in_order_and_bounds_latency() {
        let ring = Ring::new(64);
        ring.write((0..10).map(|i| i as f32));
        let mut out = [9.0f32; 4];
        ring.read(&mut out, 100);
        assert_eq!(out, [0.0, 1.0, 2.0, 3.0]);
        // 6 queued, keep 2 beyond a 2-frame read: the 2 oldest are skipped.
        let mut out = [0.0f32; 2];
        ring.read(&mut out, 2);
        assert_eq!(out, [6.0, 7.0]);
        let mut out = [5.0f32; 4];
        ring.read(&mut out, 100);
        assert_eq!(out, [8.0, 9.0, 0.0, 0.0]);
        // A full ring drops new frames instead of overwriting.
        ring.write((0..100).map(|i| i as f32));
        assert_eq!(ring.buffered(), 64);
    }

    #[test]
    fn rotation_follows_camera2_formula() {
        // OnePlus Pad 3: back 90, front 270, landscape = ROTATION_90.
        assert_eq!(upright_rotation(90, 90, false), 0);
        assert_eq!(upright_rotation(270, 90, true), 0);
        assert_eq!(upright_rotation(90, 270, false), 180);
        assert_eq!(upright_rotation(270, 270, true), 180);
        assert_eq!(upright_rotation(90, 0, false), 90);
        assert_eq!(upright_rotation(270, 0, true), 270);
    }

    fn gradient(width: usize, height: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let y = (0..width * height).map(|i| (i % 251) as u8).collect();
        let u = (0..width * height / 4).map(|i| (i % 7) as u8 + 100).collect();
        let v = (0..width * height / 4).map(|i| (i % 5) as u8 + 150).collect();
        (y, u, v)
    }

    fn image<'a>(width: usize, height: usize, planes: &'a (Vec<u8>, Vec<u8>, Vec<u8>)) -> Yuv420<'a> {
        Yuv420 {
            width,
            height,
            y: &planes.0,
            y_row: width,
            y_pixel: 1,
            u: &planes.1,
            v: &planes.2,
            uv_row: width / 2,
            uv_pixel: 1,
        }
    }

    #[test]
    fn identity_conversion_copies_planes() {
        let planes = gradient(8, 4);
        let src = image(8, 4, &planes);
        let map = Mapping::new(8, 4, 8, 4, 0);
        let mut i420 = vec![0; PixelFormat::I420.frame_size(8, 4)];
        convert(&src, &map, PixelFormat::I420, 8, 4, &mut i420);
        assert_eq!(&i420[..32], &planes.0[..]);
        assert_eq!(&i420[32..40], &planes.1[..]);
        assert_eq!(&i420[40..48], &planes.2[..]);

        let mut nv12 = vec![0; PixelFormat::Nv12.frame_size(8, 4)];
        convert(&src, &map, PixelFormat::Nv12, 8, 4, &mut nv12);
        assert_eq!(&nv12[..32], &planes.0[..]);
        let uv: Vec<u8> = planes.1.iter().zip(&planes.2).flat_map(|(&u, &v)| [u, v]).collect();
        assert_eq!(&nv12[32..], &uv[..]);

        let mut yuy2 = vec![0; PixelFormat::Yuy2.frame_size(8, 4)];
        convert(&src, &map, PixelFormat::Yuy2, 8, 4, &mut yuy2);
        assert_eq!(&yuy2[..4], &[planes.0[0], planes.1[0], planes.0[1], planes.2[0]]);
        // Row 1 reuses chroma row 0.
        assert_eq!(&yuy2[16..20], &[planes.0[8], planes.1[0], planes.0[9], planes.2[0]]);
    }

    #[test]
    fn half_turn_reverses_the_image() {
        let planes = gradient(4, 2);
        let src = image(4, 2, &planes);
        let map = Mapping::new(4, 2, 4, 2, 180);
        let mut out = vec![0; PixelFormat::I420.frame_size(4, 2)];
        convert(&src, &map, PixelFormat::I420, 4, 2, &mut out);
        let reversed: Vec<u8> = planes.0.iter().rev().copied().collect();
        assert_eq!(&out[..8], &reversed[..]);
    }

    #[test]
    fn quarter_turn_pillarboxes_into_landscape() {
        // 16x8 source rotated 90° is 8x16; fitted into 16x8 it is 4x8,
        // centered with 6-pixel black bars.
        let planes = gradient(16, 8);
        let src = image(16, 8, &planes);
        let map = Mapping::new(16, 8, 16, 8, 90);
        assert_eq!(map.cols.iter().filter(|c| c.is_some()).count(), 4);
        assert!(map.cols[..6].iter().all(Option::is_none));
        let mut out = vec![0; PixelFormat::I420.frame_size(16, 8)];
        convert(&src, &map, PixelFormat::I420, 16, 8, &mut out);
        assert_eq!(out[0], BLACK_Y);
        assert_eq!(out[15], BLACK_Y);
        // Output (6,0) is rotated (0,0): source (0, src_h-1) for 90° CW.
        assert_eq!(out[6], src.luma(0, 7));
        // Output (6,7) is rotated (0,14): source (14, 7).
        assert_eq!(out[7 * 16 + 6], src.luma(14, 7));
    }

    #[test]
    fn black_frames_are_video_black() {
        let mut yuy2 = vec![0; PixelFormat::Yuy2.frame_size(4, 2)];
        fill_black(PixelFormat::Yuy2, 4, 2, &mut yuy2);
        assert_eq!(&yuy2[..4], &[16, 128, 16, 128]);
        let mut nv12 = vec![0; PixelFormat::Nv12.frame_size(4, 2)];
        fill_black(PixelFormat::Nv12, 4, 2, &mut nv12);
        assert_eq!(&nv12[..8], &[16; 8]);
        assert_eq!(&nv12[8..], &[128; 4]);
    }

    #[test]
    fn offered_sizes_prefer_720p() {
        let available = [(1920, 1080), (1280, 720), (640, 480), (4000, 3000)];
        assert_eq!(offered_sizes(&available), vec![(1280, 720), (640, 480), (1920, 1080)]);
        assert_eq!(offered_sizes(&[(1440, 1080), (4000, 3000)]), vec![(1440, 1080)]);
        assert!(offered_sizes(&[(4000, 3000)]).is_empty());
    }
}
