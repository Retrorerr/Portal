//! Portal PipeWire standalone-client AAudio sink.
//!
//! This is the native half of the standalone-client PipeWire/AAudio experiment:
//! a normal PipeWire client process, not a PipeWire/SPA plugin. It registers an
//! `Audio/Sink` node and writes received F32 interleaved audio to Android AAudio.
//!
//! Android-only, and behind the `pipewire-sink` feature because it needs an
//! Android PipeWire sysroot the normal APK build does not. Build it with
//! `scripts/build-pipewire-aaudio-sink.sh`, which cross-compiles it and installs
//! the result as `assets/libs/arm64-v8a/liblocaldesktop_pipewire_aaudio_sink.so`.
//! `src/android/backend/pipewire_standalone_aaudio.rs` supervises it at runtime.
//!
//! The ring buffer and argument parsing below build on any host, so
//! `cargo test --features pipewire-sink --bin localdesktop_pipewire_aaudio_sink`
//! covers them without an Android sysroot.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};

const DEFAULT_NODE_NAME: &str = "portal-audio-output";
const DEFAULT_RATE: u32 = 48000;
const DEFAULT_CHANNELS: u32 = 2;
const DEFAULT_BUFFER_MS: u32 = 120;

macro_rules! note {
    ($($arg:tt)*) => {
        eprintln!("[pipewire-aaudio-sink] {}", format_args!($($arg)*))
    };
}

// ----------------------------------------------------------------------------
// Ring buffer shared between the AAudio callback thread and the PipeWire loop
// ----------------------------------------------------------------------------

/// Single-producer (PipeWire `process`) / single-consumer (AAudio callback)
/// float ring. One `UnsafeCell` per sample: the two threads only ever touch
/// disjoint slots, so no sample is ever aliased mutably.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct Sink {
    rate: u32,
    channels: usize,
    ring_frames: u64,
    ring: Vec<UnsafeCell<f32>>,
    read_frame: AtomicU64,
    write_frame: AtomicU64,
    underrun_frames: AtomicU64,
    dropped_frames: AtomicU64,
    drive_enabled: AtomicBool,
    process_pending: AtomicBool,
    pipewire_buffer_frames: AtomicU32,
    /// The `pw_stream` this sink drives, or null before it is connected.
    stream: AtomicPtr<std::ffi::c_void>,
}

unsafe impl Send for Sink {}
unsafe impl Sync for Sink {}

impl Sink {
    fn new(rate: u32, channels: u32, buffer_ms: u32) -> Sink {
        let ring_frames = ((rate as u64 * buffer_ms as u64) / 1000).max(256);
        let channels = channels as usize;
        Sink {
            rate,
            channels,
            ring_frames,
            ring: (0..ring_frames as usize * channels)
                .map(|_| UnsafeCell::new(0.0))
                .collect(),
            read_frame: AtomicU64::new(0),
            write_frame: AtomicU64::new(0),
            underrun_frames: AtomicU64::new(0),
            dropped_frames: AtomicU64::new(0),
            drive_enabled: AtomicBool::new(false),
            process_pending: AtomicBool::new(false),
            pipewire_buffer_frames: AtomicU32::new(rate / 50),
            stream: AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    fn frame_bytes(&self) -> u32 {
        (std::mem::size_of::<f32>() * self.channels) as u32
    }

    fn buffered_frames(&self) -> u64 {
        let read = self.read_frame.load(Ordering::Acquire);
        let write = self.write_frame.load(Ordering::Acquire);
        write.saturating_sub(read)
    }

    fn clear(&self) {
        let write = self.write_frame.load(Ordering::Acquire);
        self.read_frame.store(write, Ordering::Release);
    }

    /// Append interleaved frames, dropping the oldest when the ring is full.
    fn write(&self, src: &[f32]) {
        let ch = self.channels;
        let mut read = self.read_frame.load(Ordering::Acquire);
        let mut write = self.write_frame.load(Ordering::Relaxed);

        for frame in src.chunks_exact(ch) {
            if write - read >= self.ring_frames {
                read += 1;
                self.dropped_frames.fetch_add(1, Ordering::Relaxed);
                self.read_frame.store(read, Ordering::Release);
            }

            let slot = (write % self.ring_frames) as usize * ch;
            for (c, sample) in frame.iter().enumerate() {
                unsafe { *self.ring[slot + c].get() = *sample };
            }
            write += 1;
        }

        self.write_frame.store(write, Ordering::Release);
    }

    /// Fill `dst` with interleaved frames, padding with silence on underrun.
    fn read(&self, dst: &mut [f32]) {
        let ch = self.channels;
        let mut read = self.read_frame.load(Ordering::Relaxed);
        let write = self.write_frame.load(Ordering::Acquire);

        for frame in dst.chunks_exact_mut(ch) {
            if read < write {
                let slot = (read % self.ring_frames) as usize * ch;
                for (c, sample) in frame.iter_mut().enumerate() {
                    *sample = unsafe { *self.ring[slot + c].get() };
                }
                read += 1;
            } else {
                frame.fill(0.0);
                self.underrun_frames.fetch_add(1, Ordering::Relaxed);
            }
        }

        self.read_frame.store(read, Ordering::Release);
    }
}

// ----------------------------------------------------------------------------
// Tone: Portal's own speaker voicing
// ----------------------------------------------------------------------------

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
/// RBJ-cookbook biquad in transposed direct form II, one state per channel.
/// f64 keeps the low-frequency filters' poles (near z = 1) numerically clean.
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    state: Vec<[f64; 2]>,
}

impl Biquad {
    fn new(channels: usize, b: [f64; 3], a: [f64; 3]) -> Biquad {
        Biquad {
            b0: b[0] / a[0],
            b1: b[1] / a[0],
            b2: b[2] / a[0],
            a1: a[1] / a[0],
            a2: a[2] / a[0],
            state: vec![[0.0; 2]; channels],
        }
    }

    fn high_pass(channels: usize, rate: f64, hz: f64, q: f64) -> Biquad {
        let w = std::f64::consts::TAU * hz / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        Biquad::new(
            channels,
            [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        )
    }

    /// Low (`low = true`) or high shelf with slope 1.
    fn shelf(channels: usize, rate: f64, hz: f64, gain_db: f64, low: bool) -> Biquad {
        let a = 10f64.powf(gain_db / 40.0);
        let w = std::f64::consts::TAU * hz / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / 2.0 * std::f64::consts::SQRT_2;
        let beta = 2.0 * a.sqrt() * alpha;
        let s = if low { 1.0 } else { -1.0 };
        Biquad::new(
            channels,
            [
                a * ((a + 1.0) - s * (a - 1.0) * cos + beta),
                s * 2.0 * a * ((a - 1.0) - s * (a + 1.0) * cos),
                a * ((a + 1.0) - s * (a - 1.0) * cos - beta),
            ],
            [
                (a + 1.0) + s * (a - 1.0) * cos + beta,
                -s * 2.0 * ((a - 1.0) + s * (a + 1.0) * cos),
                (a + 1.0) + s * (a - 1.0) * cos - beta,
            ],
        )
    }

    fn run(&mut self, channel: usize, x: f64) -> f64 {
        let [s1, s2] = self.state[channel];
        let y = self.b0 * x + s1;
        self.state[channel] = [self.b1 * x - self.a1 * y + s2, self.b2 * x - self.a2 * y];
        y
    }
}

/// Fuller, louder output without clipping: a subsonic high-pass (tablet
/// speakers can't play below ~40 Hz, and boosting there only eats headroom),
/// a low shelf for bass, a small high shelf so the bass doesn't turn muddy,
/// make-up gain, then a stereo-linked peak limiter that holds every sample
/// under the ceiling. Runs on the AAudio callback, so it adds no latency.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct Tone {
    channels: usize,
    filters: [Biquad; 3],
    gain: f64,
    ceiling: f64,
    release: f64,
    limiter_gain: f64,
}

impl Tone {
    const HIGH_PASS_HZ: f64 = 35.0;
    const BASS_HZ: f64 = 110.0;
    const BASS_DB: f64 = 4.5;
    const AIR_HZ: f64 = 8000.0;
    const AIR_DB: f64 = 1.5;
    const GAIN_DB: f64 = 2.5;
    /// -0.6 dBFS: headroom for Android's own resampling and mixing.
    const CEILING: f64 = 0.933;
    const RELEASE_MS: f64 = 80.0;

    fn new(rate: u32, channels: usize) -> Tone {
        let rate = rate as f64;
        Tone {
            channels,
            filters: [
                Biquad::high_pass(channels, rate, Self::HIGH_PASS_HZ, std::f64::consts::FRAC_1_SQRT_2),
                Biquad::shelf(channels, rate, Self::BASS_HZ, Self::BASS_DB, true),
                Biquad::shelf(channels, rate, Self::AIR_HZ, Self::AIR_DB, false),
            ],
            gain: 10f64.powf(Self::GAIN_DB / 20.0),
            ceiling: Self::CEILING,
            release: (-1000.0 / (Self::RELEASE_MS * rate)).exp(),
            limiter_gain: 1.0,
        }
    }

    /// Process interleaved frames in place.
    fn process(&mut self, samples: &mut [f32]) {
        let ch = self.channels;
        let mut frame = [0.0f64; 8];
        for chunk in samples.chunks_exact_mut(ch) {
            let mut peak = 0.0f64;
            for (c, sample) in chunk.iter().enumerate().take(frame.len()) {
                let mut y = *sample as f64;
                for filter in &mut self.filters {
                    y = filter.run(c, y);
                }
                y *= self.gain;
                frame[c] = y;
                peak = peak.max(y.abs());
            }

            // Instant attack, smooth release: gain drops at once to keep this
            // frame under the ceiling, then recovers toward unity.
            let target = if peak > self.ceiling {
                self.ceiling / peak
            } else {
                1.0
            };
            self.limiter_gain = if target < self.limiter_gain {
                target
            } else {
                target + (self.limiter_gain - target) * self.release
            };

            for (c, sample) in chunk.iter_mut().enumerate().take(frame.len()) {
                *sample = (frame[c] * self.limiter_gain) as f32;
            }
        }
    }
}

// ----------------------------------------------------------------------------
// Arguments
// ----------------------------------------------------------------------------

struct Args {
    node_name: String,
    rate: u32,
    channels: u32,
    buffer_ms: u32,
    tone: bool,
}

enum Parsed {
    Run(Args),
    Help,
}

fn usage() {
    eprintln!(
        "Usage: localdesktop-pipewire-aaudio-sink [--node-name NAME] [--rate HZ] [--channels N] [--buffer-ms MS] [--tone on|off]"
    );
}

fn parse_args(argv: &[String]) -> Result<Parsed, String> {
    let mut args = Args {
        node_name: DEFAULT_NODE_NAME.to_string(),
        rate: DEFAULT_RATE,
        channels: DEFAULT_CHANNELS,
        buffer_ms: DEFAULT_BUFFER_MS,
        tone: true,
    };

    let mut i = 0;
    while i < argv.len() {
        if argv[i] == "--help" || argv[i] == "-h" {
            return Ok(Parsed::Help);
        }
        let raw = argv
            .get(i + 1)
            .ok_or_else(|| format!("{} needs a value", argv[i]))?;
        let number = || -> Result<u32, String> {
            raw.parse::<u32>()
                .ok()
                .filter(|&n| n != 0)
                .ok_or_else(|| format!("invalid value for {}: {raw}", argv[i]))
        };

        match argv[i].as_str() {
            "--node-name" => args.node_name = raw.clone(),
            "--rate" => args.rate = number()?,
            "--channels" => args.channels = number()?,
            "--buffer-ms" => args.buffer_ms = number()?,
            "--tone" => {
                args.tone = match raw.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => return Err(format!("invalid value for --tone: {raw}")),
                }
            }
            other => return Err(format!("unknown argument {other}")),
        }
        i += 2;
    }

    Ok(Parsed::Run(args))
}

// ----------------------------------------------------------------------------
// Android: AAudio output and the PipeWire client
// ----------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod android {
    use super::*;

    use std::ffi::{c_char, c_void, CStr};
    use std::io::Cursor;
    use std::sync::atomic::AtomicUsize;
    use std::sync::OnceLock;

    use libloading::Library;
    use pipewire as pw;
    use pw::spa;

    static AAUDIO: OnceLock<aaudio::Api> = OnceLock::new();
    static SINK: OnceLock<Sink> = OnceLock::new();
    /// Channel count of the opened AAudio stream, published before the stream
    /// starts so the data callback can emit silence until `SINK` exists.
    static AAUDIO_CHANNELS: AtomicUsize = AtomicUsize::new(0);
    /// The open AAudio stream, read by the latency timer on the PipeWire loop.
    static AAUDIO_STREAM: AtomicPtr<aaudio::Stream> = AtomicPtr::new(std::ptr::null_mut());
    /// Whether the AAudio stream is started. It is stopped while the PipeWire
    /// node is idle so the callback thread and the audio DSP can sleep.
    static AAUDIO_STARTED: AtomicBool = AtomicBool::new(false);
    /// Latency-timer ticks (500 ms) since the node stopped streaming.
    static IDLE_TICKS: AtomicU32 = AtomicU32::new(0);
    /// Ticks of idle before AAudio is stopped (3 s), so pauses between short
    /// sounds don't restart the output each time.
    const IDLE_STOP_TICKS: u32 = 6;
    /// Portal's tone chain (`--tone on`), set before `SINK`. Only the AAudio
    /// callback thread touches it after that.
    static TONE: OnceLock<ToneCell> = OnceLock::new();

    struct ToneCell(UnsafeCell<Tone>);
    unsafe impl Sync for ToneCell {}

    impl Sink {
        /// Ask the graph for another quantum once the ring runs low. Called
        /// from the AAudio callback thread, exactly like the C original.
        fn maybe_trigger_process(&self) {
            let stream = self.stream.load(Ordering::Acquire);
            if !self.drive_enabled.load(Ordering::Acquire) || stream.is_null() {
                return;
            }

            let mut pw_frames = self.pipewire_buffer_frames.load(Ordering::Acquire);
            if pw_frames == 0 {
                pw_frames = self.rate / 50;
            }
            if self.buffered_frames() > (pw_frames / 2).max(256) as u64 {
                return;
            }

            if self
                .process_pending
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                unsafe { pw::sys::pw_stream_trigger_process(stream.cast()) };
            }
        }
    }

    // -- AAudio, loaded at runtime like the C original (no -laaudio link) -----

    mod aaudio {
        use super::*;

        pub type Res = i32;
        pub enum Builder {}
        pub enum Stream {}

        pub const OK: Res = 0;
        pub const DIRECTION_OUTPUT: i32 = 0;
        pub const FORMAT_PCM_FLOAT: i32 = 2;
        pub const PERFORMANCE_MODE_LOW_LATENCY: i32 = 12;
        pub const SHARING_MODE_SHARED: i32 = 1;
        pub const USAGE_MEDIA: i32 = 1;
        pub const CONTENT_TYPE_MUSIC: i32 = 2;
        pub const CLOCK_MONOTONIC: i32 = 1;
        pub const CALLBACK_RESULT_CONTINUE: i32 = 0;

        pub type DataCallback =
            unsafe extern "C" fn(*mut Stream, *mut c_void, *mut c_void, i32) -> i32;
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
            pub set_usage: unsafe extern "C" fn(*mut Builder, i32),
            pub set_content_type: unsafe extern "C" fn(*mut Builder, i32),
            pub set_data_callback: unsafe extern "C" fn(*mut Builder, DataCallback, *mut c_void),
            pub set_error_callback: unsafe extern "C" fn(*mut Builder, ErrorCallback, *mut c_void),
            pub open_stream: unsafe extern "C" fn(*mut Builder, *mut *mut Stream) -> Res,
            pub sample_rate: unsafe extern "C" fn(*mut Stream) -> i32,
            pub channel_count: unsafe extern "C" fn(*mut Stream) -> i32,
            pub buffer_size_in_frames: unsafe extern "C" fn(*mut Stream) -> i32,
            pub frames_written: unsafe extern "C" fn(*mut Stream) -> i64,
            pub timestamp: unsafe extern "C" fn(*mut Stream, i32, *mut i64, *mut i64) -> Res,
            pub request_start: unsafe extern "C" fn(*mut Stream) -> Res,
            pub request_stop: unsafe extern "C" fn(*mut Stream) -> Res,
            pub close: unsafe extern "C" fn(*mut Stream) -> Res,
        }

        unsafe fn sym<T: Copy>(lib: &Library, name: &[u8]) -> Result<T, String> {
            lib.get::<T>(name).map(|s| *s).map_err(|e| {
                format!(
                    "missing AAudio symbol {}: {e}",
                    String::from_utf8_lossy(&name[..name.len() - 1])
                )
            })
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
                        set_performance_mode: sym(
                            &lib,
                            b"AAudioStreamBuilder_setPerformanceMode\0",
                        )?,
                        set_sharing_mode: sym(&lib, b"AAudioStreamBuilder_setSharingMode\0")?,
                        set_sample_rate: sym(&lib, b"AAudioStreamBuilder_setSampleRate\0")?,
                        set_channel_count: sym(&lib, b"AAudioStreamBuilder_setChannelCount\0")?,
                        set_usage: sym(&lib, b"AAudioStreamBuilder_setUsage\0")?,
                        set_content_type: sym(&lib, b"AAudioStreamBuilder_setContentType\0")?,
                        set_data_callback: sym(&lib, b"AAudioStreamBuilder_setDataCallback\0")?,
                        set_error_callback: sym(&lib, b"AAudioStreamBuilder_setErrorCallback\0")?,
                        open_stream: sym(&lib, b"AAudioStreamBuilder_openStream\0")?,
                        sample_rate: sym(&lib, b"AAudioStream_getSampleRate\0")?,
                        channel_count: sym(&lib, b"AAudioStream_getChannelCount\0")?,
                        buffer_size_in_frames: sym(&lib, b"AAudioStream_getBufferSizeInFrames\0")?,
                        frames_written: sym(&lib, b"AAudioStream_getFramesWritten\0")?,
                        timestamp: sym(&lib, b"AAudioStream_getTimestamp\0")?,
                        request_start: sym(&lib, b"AAudioStream_requestStart\0")?,
                        request_stop: sym(&lib, b"AAudioStream_requestStop\0")?,
                        close: sym(&lib, b"AAudioStream_close\0")?,
                        _lib: lib,
                    })
                }
            }
        }
    }

    unsafe extern "C" fn aaudio_data_callback(
        _stream: *mut aaudio::Stream,
        _userdata: *mut c_void,
        audio_data: *mut c_void,
        num_frames: i32,
    ) -> i32 {
        let frames = num_frames.max(0) as usize;
        let channels = AAUDIO_CHANNELS.load(Ordering::Acquire);
        let dst = std::slice::from_raw_parts_mut(audio_data as *mut f32, frames * channels);

        match SINK.get() {
            // The stream starts before the ring exists; play silence until then.
            None => dst.fill(0.0),
            Some(sink) => {
                sink.read(dst);
                sink.maybe_trigger_process();
                if let Some(tone) = TONE.get() {
                    (*tone.0.get()).process(dst);
                }
            }
        }

        aaudio::CALLBACK_RESULT_CONTINUE
    }

    unsafe extern "C" fn aaudio_error_callback(
        _stream: *mut aaudio::Stream,
        _userdata: *mut c_void,
        error: aaudio::Res,
    ) {
        let text = match AAUDIO.get() {
            Some(api) => CStr::from_ptr((api.result_text)(error))
                .to_string_lossy()
                .into_owned(),
            None => "unknown".to_string(),
        };
        note!("AAudio error: {text}");
    }

    /// Open and start an AAudio output stream, returning it together with the
    /// rate and channel count it actually negotiated.
    fn open_aaudio(rate: u32, channels: u32) -> Result<(*mut aaudio::Stream, u32, u32), String> {
        let api = match AAUDIO.get() {
            Some(api) => api,
            None => {
                let _ = AAUDIO.set(aaudio::Api::load()?);
                AAUDIO.get().unwrap()
            }
        };

        unsafe {
            let mut builder: *mut aaudio::Builder = std::ptr::null_mut();
            if (api.create_builder)(&mut builder) != aaudio::OK {
                return Err("AAudio_createStreamBuilder failed".into());
            }

            (api.set_direction)(builder, aaudio::DIRECTION_OUTPUT);
            (api.set_format)(builder, aaudio::FORMAT_PCM_FLOAT);
            // Low latency on purpose. The deep-buffer output (POWER_SAVING)
            // carries OnePlus's OplusAudioX tuning, but on the Pad 3 it added
            // ~250 ms of latency, stuttered with this refill scheme and gave
            // no audible gain, so the FAST path stays (2026-09-28).
            (api.set_performance_mode)(builder, aaudio::PERFORMANCE_MODE_LOW_LATENCY);
            (api.set_usage)(builder, aaudio::USAGE_MEDIA);
            (api.set_content_type)(builder, aaudio::CONTENT_TYPE_MUSIC);
            (api.set_sharing_mode)(builder, aaudio::SHARING_MODE_SHARED);
            (api.set_sample_rate)(builder, rate as i32);
            (api.set_channel_count)(builder, channels as i32);
            (api.set_data_callback)(builder, aaudio_data_callback, std::ptr::null_mut());
            (api.set_error_callback)(builder, aaudio_error_callback, std::ptr::null_mut());

            let mut stream: *mut aaudio::Stream = std::ptr::null_mut();
            let res = (api.open_stream)(builder, &mut stream);
            (api.builder_delete)(builder);
            if res != aaudio::OK {
                return Err("AAudioStreamBuilder_openStream failed".into());
            }

            let rate = (api.sample_rate)(stream) as u32;
            let channels = (api.channel_count)(stream) as u32;
            AAUDIO_CHANNELS.store(channels as usize, Ordering::Release);

            note!(
                "opened AAudio stream: rate={rate} channels={channels} buffer_frames={}",
                (api.buffer_size_in_frames)(stream)
            );

            if (api.request_start)(stream) != aaudio::OK {
                (api.close)(stream);
                return Err("AAudioStream_requestStart failed".into());
            }

            AAUDIO_STREAM.store(stream, Ordering::Release);
            AAUDIO_STARTED.store(true, Ordering::Release);
            Ok((stream, rate, channels))
        }
    }

    /// Start or stop the open AAudio stream (no-op if already in that state).
    fn set_aaudio_started(started: bool) {
        let stream = AAUDIO_STREAM.load(Ordering::Acquire);
        let Some(api) = AAUDIO.get() else { return };
        if stream.is_null() || AAUDIO_STARTED.load(Ordering::Acquire) == started {
            return;
        }
        let res = unsafe {
            if started {
                (api.request_start)(stream)
            } else {
                (api.request_stop)(stream)
            }
        };
        if res == aaudio::OK {
            AAUDIO_STARTED.store(started, Ordering::Release);
            note!("AAudio {}", if started { "started" } else { "stopped (idle)" });
        } else {
            note!("AAudio {} failed: {res}", if started { "start" } else { "stop" });
        }
    }

    fn close_aaudio(stream: *mut aaudio::Stream) {
        AAUDIO_STREAM.store(std::ptr::null_mut(), Ordering::Release);
        AAUDIO_STARTED.store(false, Ordering::Release);
        if let (Some(api), false) = (AAUDIO.get(), stream.is_null()) {
            unsafe {
                (api.request_stop)(stream);
                (api.close)(stream);
            }
        }
    }

    /// Time from a sample entering the ring to it leaving the speaker: the
    /// frames AAudio holds but has not presented yet, plus what waits in the
    /// ring. None until AAudio reports its first timestamp.
    fn output_latency_ns(sink: &Sink) -> Option<i64> {
        let api = AAUDIO.get()?;
        let stream = AAUDIO_STREAM.load(Ordering::Acquire);
        if stream.is_null() {
            return None;
        }
        let (mut position, mut presented_ns) = (0i64, 0i64);
        let written = unsafe {
            if (api.timestamp)(stream, aaudio::CLOCK_MONOTONIC, &mut position, &mut presented_ns)
                != aaudio::OK
            {
                return None;
            }
            (api.frames_written)(stream)
        };
        let mut now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
        let now_ns = now.tv_sec * 1_000_000_000 + now.tv_nsec;
        let rate = sink.rate as i64;
        let presented_now = position + (now_ns - presented_ns).max(0) * rate / 1_000_000_000;
        let pending = (written - presented_now).max(0) + sink.buffered_frames() as i64;
        Some(pending * 1_000_000_000 / rate)
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

    fn enum_format_pod(rate: u32, channels: u32) -> Vec<u8> {
        let mut info = spa::param::audio::AudioInfoRaw::new();
        info.set_format(spa::param::audio::AudioFormat::F32LE);
        info.set_rate(rate);
        info.set_channels(channels);

        pod_bytes(&spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties: info.into(),
        }))
    }

    fn buffers_pod(rate: u32, frame_bytes: u32) -> Vec<u8> {
        let buffer_bytes = ((rate / 100).max(256) * frame_bytes) as i32;
        let frame_bytes = frame_bytes as i32;

        pod_bytes(&spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
            id: spa::param::ParamType::Buffers.as_raw(),
            properties: vec![
                prop(spa::sys::SPA_PARAM_BUFFERS_buffers, int_range(8, 2, 16)),
                prop(spa::sys::SPA_PARAM_BUFFERS_blocks, spa::pod::Value::Int(1)),
                prop(
                    spa::sys::SPA_PARAM_BUFFERS_size,
                    int_range(buffer_bytes, frame_bytes * 256, frame_bytes * 8192),
                ),
                prop(
                    spa::sys::SPA_PARAM_BUFFERS_stride,
                    spa::pod::Value::Int(frame_bytes),
                ),
                prop(spa::sys::SPA_PARAM_BUFFERS_align, spa::pod::Value::Int(16)),
                prop(
                    spa::sys::SPA_PARAM_BUFFERS_dataType,
                    spa::pod::Value::Choice(spa::pod::ChoiceValue::Int(spa::utils::Choice(
                        spa::utils::ChoiceFlags::empty(),
                        spa::utils::ChoiceEnum::Flags {
                            default: 1 << spa::sys::SPA_DATA_MemPtr,
                            flags: Vec::new(),
                        },
                    ))),
                ),
            ],
        }))
    }

    fn process_latency_pod(ns: i64) -> Vec<u8> {
        pod_bytes(&spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamProcessLatency.as_raw(),
            id: spa::param::ParamType::ProcessLatency.as_raw(),
            properties: vec![prop(
                spa::sys::SPA_PARAM_PROCESS_LATENCY_ns,
                spa::pod::Value::Long(ns),
            )],
        }))
    }

    fn meta_pod() -> Vec<u8> {
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

    // -- Stream events -------------------------------------------------------

    fn on_state_changed(
        _stream: &pw::stream::Stream,
        sink: &mut &'static Sink,
        old: pw::stream::StreamState,
        new: pw::stream::StreamState,
    ) {
        if new == pw::stream::StreamState::Streaming {
            IDLE_TICKS.store(0, Ordering::Release);
            set_aaudio_started(true);
            sink.clear();
            sink.process_pending.store(false, Ordering::Release);
            sink.drive_enabled.store(true, Ordering::Release);
            sink.maybe_trigger_process();
        } else {
            sink.drive_enabled.store(false, Ordering::Release);
            sink.process_pending.store(false, Ordering::Release);
        }
        note!("stream state {old:?} -> {new:?}");
    }

    fn on_param_changed(
        stream: &pw::stream::Stream,
        sink: &mut &'static Sink,
        id: u32,
        param: Option<&spa::pod::Pod>,
    ) {
        let Some(param) = param else { return };
        if id != spa::param::ParamType::Format.as_raw() {
            return;
        }
        let Ok((media_type, media_subtype)) = spa::param::format_utils::parse_format(param) else {
            return;
        };
        if media_type != spa::param::format::MediaType::Audio
            || media_subtype != spa::param::format::MediaSubtype::Raw
        {
            return;
        }
        let mut info = spa::param::audio::AudioInfoRaw::new();
        if info.parse(param).is_err() {
            return;
        }

        note!(
            "negotiated PipeWire format: rate={} channels={} format={:?}",
            info.rate(),
            info.channels(),
            info.format()
        );
        if info.rate() != sink.rate || info.channels() != sink.channels as u32 {
            note!("warning: negotiated format differs from AAudio stream");
        }

        let buffers = buffers_pod(sink.rate, sink.frame_bytes());
        let meta = meta_pod();
        let (Some(buffers), Some(meta)) = (
            spa::pod::Pod::from_bytes(&buffers),
            spa::pod::Pod::from_bytes(&meta),
        ) else {
            return;
        };

        if let Err(e) = stream.update_params(&mut [buffers, meta]) {
            note!("failed to update stream params: {e}");
        }
    }

    fn on_process(stream: &pw::stream::Stream, sink: &mut &'static Sink) {
        let Some(mut buffer) = stream.dequeue_buffer() else {
            note!("out of buffers");
            return;
        };

        let frame_bytes = sink.frame_bytes() as usize;
        if let Some(data) = buffer.datas_mut().first_mut() {
            let offset = data.chunk().offset() as usize;
            let size = data.chunk().size() as usize;
            if let Some(bytes) = data.data() {
                let offset = offset.min(bytes.len());
                let size = size.min(bytes.len() - offset);
                let frames = size / frame_bytes;
                if frames > 0 {
                    sink.pipewire_buffer_frames
                        .store(frames as u32, Ordering::Release);
                    // MAP_BUFFERS memory holds F32 interleaved samples.
                    let samples = unsafe {
                        std::slice::from_raw_parts(
                            bytes[offset..].as_ptr() as *const f32,
                            frames * sink.channels,
                        )
                    };
                    sink.write(samples);
                }
            }
        }

        drop(buffer);
        sink.process_pending.store(false, Ordering::Release);
    }

    // -- Entry point ---------------------------------------------------------

    fn run_pipewire(sink: &'static Sink, node_name: &str) -> Result<(), String> {
        let mainloop = pw::main_loop::MainLoopRc::new(None)
            .map_err(|e| format!("failed to create PipeWire main loop: {e}"))?;

        let quit = {
            let mainloop = mainloop.clone();
            move || mainloop.quit()
        };
        let _sigint = mainloop
            .loop_()
            .add_signal_local(pw::loop_::Signal::INT, quit.clone());
        let _sigterm = mainloop
            .loop_()
            .add_signal_local(pw::loop_::Signal::TERM, quit);

        let context = pw::context::ContextRc::new(&mainloop, None)
            .map_err(|e| format!("failed to create PipeWire context: {e}"))?;
        let core = context
            .connect_rc(None)
            .map_err(|e| format!("failed to connect to PipeWire: {e}"))?;

        let props = pw::properties::properties! {
            *pw::keys::MEDIA_CLASS => "Audio/Sink",
            *pw::keys::NODE_NAME => node_name,
            *pw::keys::NODE_DESCRIPTION => "Portal Audio Output",
            *pw::keys::NODE_DRIVER => "true",
            *pw::keys::NODE_SUSPEND_ON_IDLE => "false",
            *pw::keys::AUDIO_RATE => sink.rate.to_string(),
            *pw::keys::AUDIO_CHANNELS => sink.channels.to_string(),
        };

        let stream = pw::stream::StreamRc::new(core, node_name, props)
            .map_err(|e| format!("failed to create PipeWire stream: {e}"))?;

        let _listener = stream
            .add_local_listener_with_user_data(sink)
            .state_changed(on_state_changed)
            .param_changed(on_param_changed)
            .process(on_process)
            .register()
            .map_err(|e| format!("failed to register stream listener: {e}"))?;

        let format = enum_format_pod(sink.rate, sink.channels as u32);
        let mut params = [spa::pod::Pod::from_bytes(&format).ok_or("bad EnumFormat pod")?];
        stream
            .connect(
                spa::utils::Direction::Input,
                None,
                pw::stream::StreamFlags::AUTOCONNECT
                    | pw::stream::StreamFlags::MAP_BUFFERS
                    | pw::stream::StreamFlags::DRIVER
                    | pw::stream::StreamFlags::RT_PROCESS,
                &mut params,
            )
            .map_err(|e| format!("failed to connect PipeWire stream: {e}"))?;

        sink.stream
            .store(stream.as_raw_ptr().cast(), Ordering::Release);

        // Report the Android output latency so clients keep audio and video
        // in sync (Firefox reads it through pipewire-pulse). It moves with the
        // route, so it is sampled every half second. A single sample swings
        // by about one data callback (AAudio's written count advances in
        // callback steps), so the average of the last 8 samples is reported,
        // and only when it moves by 10 ms or more.
        let latency_stream = stream.clone();
        let samples = std::cell::RefCell::new(std::collections::VecDeque::with_capacity(8));
        let reported_ns = std::cell::Cell::new(0i64);
        let latency_timer = mainloop.loop_().add_timer(move |_| {
            if !sink.drive_enabled.load(Ordering::Acquire) {
                // Node idle: stop AAudio once the hold has passed. The
                // Streaming transition restarts it before the graph runs.
                if IDLE_TICKS.fetch_add(1, Ordering::AcqRel) + 1 >= IDLE_STOP_TICKS {
                    set_aaudio_started(false);
                }
                return;
            }
            let Some(sample) = output_latency_ns(sink) else { return };
            let ns = {
                let mut samples = samples.borrow_mut();
                if samples.len() == 8 {
                    samples.pop_front();
                }
                samples.push_back(sample);
                if samples.len() < 8 {
                    return;
                }
                samples.iter().sum::<i64>() / samples.len() as i64
            };
            if (ns - reported_ns.get()).abs() < 10_000_000 {
                return;
            }
            let bytes = process_latency_pod(ns);
            let Some(pod) = spa::pod::Pod::from_bytes(&bytes) else { return };
            match latency_stream.update_params(&mut [pod]) {
                Ok(()) => {
                    note!("output latency {} ms", ns / 1_000_000);
                    reported_ns.set(ns);
                }
                Err(e) => note!("failed to report latency: {e}"),
            }
        });
        let half_second = std::time::Duration::from_millis(500);
        latency_timer.update_timer(Some(half_second), Some(half_second));

        note!(
            "running node={node_name} rate={} channels={} ring_frames={}",
            sink.rate,
            sink.channels,
            sink.ring_frames
        );
        mainloop.run();
        Ok(())
    }

    pub fn run(args: Args) -> Result<(), String> {
        pw::init();

        let result = (|| {
            let (aaudio_stream, rate, channels) = open_aaudio(args.rate, args.channels)?;
            if args.tone {
                let _ = TONE.set(ToneCell(UnsafeCell::new(Tone::new(rate, channels as usize))));
            }
            note!("tone {}", if args.tone { "on" } else { "off" });
            let sink = match SINK.get() {
                Some(sink) => sink,
                None => {
                    let _ = SINK.set(Sink::new(rate, channels, args.buffer_ms));
                    SINK.get().unwrap()
                }
            };

            let result = run_pipewire(sink, &args.node_name);

            sink.drive_enabled.store(false, Ordering::Release);
            sink.process_pending.store(false, Ordering::Release);
            sink.stream.store(std::ptr::null_mut(), Ordering::Release);
            close_aaudio(aaudio_stream);

            note!(
                "stopped underrun_frames={} dropped_frames={}",
                sink.underrun_frames.load(Ordering::Relaxed),
                sink.dropped_frames.load(Ordering::Relaxed)
            );
            result
        })();

        unsafe { pw::deinit() };
        result
    }
}

#[cfg(not(target_os = "android"))]
mod android {
    use super::Args;

    pub fn run(_args: Args) -> Result<(), String> {
        Err("pipewire_aaudio_sink only runs on Android".into())
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
        Ok(Parsed::Help) => {
            usage();
            std::process::ExitCode::SUCCESS
        }
        Ok(Parsed::Run(args)) => match android::run(args) {
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

    fn args(argv: &[&str]) -> Args {
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        match parse_args(&argv) {
            Ok(Parsed::Run(args)) => args,
            _ => panic!("expected parsed args"),
        }
    }

    #[test]
    fn defaults_and_overrides_parse() {
        let d = args(&[]);
        assert_eq!(
            (d.node_name.as_str(), d.rate, d.channels, d.buffer_ms, d.tone),
            (DEFAULT_NODE_NAME, 48000, 2, 120, true)
        );
        assert!(!args(&["--tone", "off"]).tone);
        assert!(parse_args(&["--tone".into(), "loud".into()]).is_err());

        let a = args(&["--node-name", "x", "--rate", "44100", "--channels", "1"]);
        assert_eq!((a.node_name.as_str(), a.rate, a.channels), ("x", 44100, 1));

        assert!(parse_args(&["--rate".into(), "0".into()]).is_err());
        assert!(parse_args(&["--rate".into()]).is_err());
        assert!(parse_args(&["--nope".into(), "1".into()]).is_err());
        assert!(matches!(parse_args(&["--help".into()]), Ok(Parsed::Help)));
    }

    /// 48 kHz × 120 ms × 2ch, the values the supervisor passes.
    fn sink() -> Sink {
        Sink::new(48000, 2, 120)
    }

    #[test]
    fn ring_sizing_matches_buffer_ms() {
        assert_eq!(sink().ring_frames, 5760);
        assert_eq!(sink().frame_bytes(), 8);
        // Tiny buffers still get the 256-frame floor.
        assert_eq!(Sink::new(48000, 2, 1).ring_frames, 256);
    }

    #[test]
    fn writes_come_back_in_order() {
        let sink = sink();
        let src: Vec<f32> = (0..8).map(|i| i as f32).collect();
        sink.write(&src);
        assert_eq!(sink.buffered_frames(), 4);

        let mut dst = vec![-1.0; 8];
        sink.read(&mut dst);
        assert_eq!(dst, src);
        assert_eq!(sink.buffered_frames(), 0);
        assert_eq!(sink.underrun_frames.load(Ordering::Relaxed), 0);
        assert_eq!(sink.dropped_frames.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn underrun_pads_with_silence() {
        let sink = sink();
        sink.write(&[1.0, 2.0]);

        let mut dst = vec![-1.0; 6];
        sink.read(&mut dst);
        assert_eq!(dst, vec![1.0, 2.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(sink.underrun_frames.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn overflow_drops_the_oldest_frames() {
        let sink = Sink::new(48000, 2, 1); // 256 frames
        let src: Vec<f32> = (0..(300 * 2)).map(|i| i as f32).collect();
        sink.write(&src);

        assert_eq!(sink.dropped_frames.load(Ordering::Relaxed), 44);
        assert_eq!(sink.buffered_frames(), 256);

        let mut dst = vec![-1.0; 256 * 2];
        sink.read(&mut dst);
        // The first 44 frames were dropped, so frame 44 leads.
        assert_eq!(&dst[..2], &[88.0, 89.0]);
        assert_eq!(&dst[dst.len() - 2..], &[598.0, 599.0]);
    }

    /// Steady-state peak of a stereo sine at `hz` through a fresh Tone.
    fn tone_peak(hz: f64, amplitude: f64) -> f64 {
        let mut tone = Tone::new(48000, 2);
        let mut samples: Vec<f32> = (0..48000)
            .flat_map(|n| {
                let x = (amplitude * (std::f64::consts::TAU * hz * n as f64 / 48000.0).sin()) as f32;
                [x, x]
            })
            .collect();
        tone.process(&mut samples);
        samples[samples.len() / 2..]
            .iter()
            .fold(0.0f64, |peak, s| peak.max(s.abs() as f64))
    }

    fn db(ratio: f64) -> f64 {
        20.0 * ratio.log10()
    }

    #[test]
    fn tone_keeps_silence_silent() {
        let mut tone = Tone::new(48000, 2);
        let mut samples = vec![0.0f32; 4096];
        tone.process(&mut samples);
        assert!(samples.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn tone_lifts_bass_and_level_below_the_limiter() {
        // Quiet enough that the limiter never engages.
        let mid = db(tone_peak(1000.0, 0.1) / 0.1);
        let bass = db(tone_peak(70.0, 0.1) / 0.1);
        let rumble = db(tone_peak(15.0, 0.1) / 0.1);
        assert!((mid - Tone::GAIN_DB).abs() < 0.5, "mid {mid} dB");
        assert!(bass > mid + 3.0, "bass {bass} dB vs mid {mid} dB");
        assert!(rumble < mid, "rumble {rumble} dB vs mid {mid} dB");
    }

    #[test]
    fn tone_never_exceeds_the_ceiling() {
        for hz in [50.0, 100.0, 1000.0, 10000.0] {
            let peak = tone_peak(hz, 1.0);
            assert!(peak <= Tone::CEILING + 1e-6, "{hz} Hz peaked at {peak}");
            // Loud input still comes out loud, not squashed.
            assert!(peak > 0.8, "{hz} Hz only reached {peak}");
        }
    }

    #[test]
    fn clear_discards_pending_audio() {
        let sink = sink();
        sink.write(&[1.0, 2.0, 3.0, 4.0]);
        sink.clear();
        assert_eq!(sink.buffered_frames(), 0);
    }
}
