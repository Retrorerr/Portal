//! The Android output stream, played on behalf of the PipeWire sink.
//!
//! The sink process drives the guest's audio graph; this thread, in the app's
//! own process, owns the AAudio stream and plays what the sink sends over
//! `core::audio_link`. Android attributes a stream to the process that opened
//! it, so opening it here is what lets per-app volume panels, playback
//! capture and effect apps see Portal like any other media app.
//!
//! By default the stream is an ordinary media stream, like any music app's:
//! Android applies the device's sound effects and tuning to it, on a mixer
//! with a deep buffer. `Output::LowLatency` asks for the shortest path
//! instead, which skips both the effects and that buffer.

use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CStr};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use libloading::Library;

use crate::core::audio_link::{self, Request, LATENCY_UNKNOWN};

macro_rules! note {
    ($($arg:tt)*) => {
        log::info!("[PipeWireAAudio] player: {}", format_args!($($arg)*))
    };
}

/// One blocking AAudio write, and how many may pass without a frame
/// accepted (3 s) before the stream counts as dead.
const WRITE_SLICE_NS: i64 = 100_000_000;
const WRITE_STALLED_SLICES: u32 = 30;
/// How often the output latency is sampled and the buffer size reviewed.
const SAMPLE_EVERY: Duration = Duration::from_millis(500);

/// Set by AAudio's error callback (route change, Bluetooth, headphones): the
/// player thread reopens the stream.
static DISCONNECTED: AtomicBool = AtomicBool::new(false);

/// Which Android output the stream asks for.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Output {
    /// The ordinary media path, with the device's effects and tuning.
    Media,
    /// The shortest path Android grants: less delay, no effects.
    LowLatency,
}

// -- AAudio, loaded at runtime (Portal still installs on Android 5) ----------

mod aaudio {
    use super::*;

    pub type Res = i32;
    pub enum Builder {}
    pub enum Stream {}

    pub const OK: Res = 0;
    pub const ERROR_TIMEOUT: Res = -885;
    pub const DIRECTION_OUTPUT: i32 = 0;
    pub const FORMAT_PCM_FLOAT: i32 = 2;
    pub const PERFORMANCE_MODE_POWER_SAVING: i32 = 11;
    pub const PERFORMANCE_MODE_LOW_LATENCY: i32 = 12;
    pub const SHARING_MODE_SHARED: i32 = 1;
    pub const USAGE_MEDIA: i32 = 1;
    pub const CONTENT_TYPE_MUSIC: i32 = 2;
    pub const SESSION_ID_ALLOCATE: i32 = 0;
    pub const CLOCK_MONOTONIC: i32 = 1;

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
        pub set_session_id: unsafe extern "C" fn(*mut Builder, i32),
        pub set_error_callback: unsafe extern "C" fn(*mut Builder, ErrorCallback, *mut c_void),
        pub open_stream: unsafe extern "C" fn(*mut Builder, *mut *mut Stream) -> Res,
        pub sample_rate: unsafe extern "C" fn(*mut Stream) -> i32,
        pub channel_count: unsafe extern "C" fn(*mut Stream) -> i32,
        pub session_id: unsafe extern "C" fn(*mut Stream) -> i32,
        pub frames_per_burst: unsafe extern "C" fn(*mut Stream) -> i32,
        pub buffer_capacity: unsafe extern "C" fn(*mut Stream) -> i32,
        pub buffer_size: unsafe extern "C" fn(*mut Stream) -> i32,
        pub set_buffer_size: unsafe extern "C" fn(*mut Stream, i32) -> Res,
        pub xrun_count: unsafe extern "C" fn(*mut Stream) -> i32,
        pub frames_written: unsafe extern "C" fn(*mut Stream) -> i64,
        pub timestamp: unsafe extern "C" fn(*mut Stream, i32, *mut i64, *mut i64) -> Res,
        pub write: unsafe extern "C" fn(*mut Stream, *const c_void, i32, i64) -> Res,
        pub request_start: unsafe extern "C" fn(*mut Stream) -> Res,
        pub request_stop: unsafe extern "C" fn(*mut Stream) -> Res,
        pub close: unsafe extern "C" fn(*mut Stream) -> Res,
    }

    /// # Safety
    /// `T` must be the type of the symbol `name` (NUL-terminated) in `lib`.
    unsafe fn sym<T: Copy>(lib: &Library, name: &[u8]) -> Result<T, String> {
        unsafe { lib.get::<T>(name) }.map(|s| *s).map_err(|e| {
            format!(
                "missing AAudio symbol {}: {e}",
                String::from_utf8_lossy(&name[..name.len() - 1])
            )
        })
    }

    impl Api {
        fn load() -> Result<Api, String> {
            // SAFETY: libaaudio.so is the platform's library, and every
            // symbol is given the signature its NDK header declares.
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
                    set_usage: sym(&lib, b"AAudioStreamBuilder_setUsage\0")?,
                    set_content_type: sym(&lib, b"AAudioStreamBuilder_setContentType\0")?,
                    set_session_id: sym(&lib, b"AAudioStreamBuilder_setSessionId\0")?,
                    set_error_callback: sym(&lib, b"AAudioStreamBuilder_setErrorCallback\0")?,
                    open_stream: sym(&lib, b"AAudioStreamBuilder_openStream\0")?,
                    sample_rate: sym(&lib, b"AAudioStream_getSampleRate\0")?,
                    channel_count: sym(&lib, b"AAudioStream_getChannelCount\0")?,
                    session_id: sym(&lib, b"AAudioStream_getSessionId\0")?,
                    frames_per_burst: sym(&lib, b"AAudioStream_getFramesPerBurst\0")?,
                    buffer_capacity: sym(&lib, b"AAudioStream_getBufferCapacityInFrames\0")?,
                    buffer_size: sym(&lib, b"AAudioStream_getBufferSizeInFrames\0")?,
                    set_buffer_size: sym(&lib, b"AAudioStream_setBufferSizeInFrames\0")?,
                    xrun_count: sym(&lib, b"AAudioStream_getXRunCount\0")?,
                    frames_written: sym(&lib, b"AAudioStream_getFramesWritten\0")?,
                    timestamp: sym(&lib, b"AAudioStream_getTimestamp\0")?,
                    write: sym(&lib, b"AAudioStream_write\0")?,
                    request_start: sym(&lib, b"AAudioStream_requestStart\0")?,
                    request_stop: sym(&lib, b"AAudioStream_requestStop\0")?,
                    close: sym(&lib, b"AAudioStream_close\0")?,
                    _lib: lib,
                })
            }
        }

        pub fn get() -> Result<&'static Api, String> {
            static API: OnceLock<Result<Api, String>> = OnceLock::new();
            API.get_or_init(Api::load).as_ref().map_err(Clone::clone)
        }

        pub fn text(&self, res: Res) -> String {
            // SAFETY: AAudio returns a static string for any result code.
            unsafe { CStr::from_ptr((self.result_text)(res)) }
                .to_string_lossy()
                .into_owned()
        }
    }
}

unsafe extern "C" fn aaudio_error_callback(
    _stream: *mut aaudio::Stream,
    _userdata: *mut c_void,
    error: aaudio::Res,
) {
    let text = aaudio::Api::get().map_or_else(|_| "unknown".to_string(), |api| api.text(error));
    note!("AAudio stream lost: {text}");
    DISCONNECTED.store(true, Ordering::Release);
}

/// The Android output stream. Only the player thread touches it.
struct Player {
    api: &'static aaudio::Api,
    stream: *mut aaudio::Stream,
    output: Output,
    rate: u32,
    channels: u32,
    burst: i32,
    capacity: i32,
    buffer: i32,
    xruns: i32,
    started: bool,
    /// The quantum the last start left room for.
    quantum: usize,
}

impl Player {
    /// Open a stream without starting it, so Android sees nothing playing.
    fn open(output: Output, rate: u32, channels: u32) -> Result<Player, String> {
        let api = aaudio::Api::get()?;
        // SAFETY: the builder and stream pointers come from AAudio and are
        // used as its API describes; the builder is deleted exactly once.
        unsafe {
            let mut builder: *mut aaudio::Builder = std::ptr::null_mut();
            if (api.create_builder)(&mut builder) != aaudio::OK {
                return Err("AAudio_createStreamBuilder failed".into());
            }

            (api.set_direction)(builder, aaudio::DIRECTION_OUTPUT);
            (api.set_format)(builder, aaudio::FORMAT_PCM_FLOAT);
            (api.set_usage)(builder, aaudio::USAGE_MEDIA);
            (api.set_content_type)(builder, aaudio::CONTENT_TYPE_MUSIC);
            (api.set_sharing_mode)(builder, aaudio::SHARING_MODE_SHARED);
            (api.set_sample_rate)(builder, rate as i32);
            (api.set_channel_count)(builder, channels as i32);
            (api.set_error_callback)(builder, aaudio_error_callback, std::ptr::null_mut());
            match output {
                // What a music app gets: the deep-buffer output, where the
                // device's effects run, and an audio session that equalizer
                // apps can attach to. Android applies its output effects on
                // that one mixer only, so neither the fast nor the default
                // mixer has them.
                Output::Media => {
                    (api.set_performance_mode)(builder, aaudio::PERFORMANCE_MODE_POWER_SAVING);
                    (api.set_session_id)(builder, aaudio::SESSION_ID_ALLOCATE);
                }
                Output::LowLatency => {
                    (api.set_performance_mode)(builder, aaudio::PERFORMANCE_MODE_LOW_LATENCY);
                }
            }

            let mut stream: *mut aaudio::Stream = std::ptr::null_mut();
            let res = (api.open_stream)(builder, &mut stream);
            (api.builder_delete)(builder);
            if res != aaudio::OK {
                return Err(format!(
                    "AAudioStreamBuilder_openStream failed: {}",
                    api.text(res)
                ));
            }

            let mut player = Player {
                api,
                stream,
                output,
                rate: (api.sample_rate)(stream) as u32,
                channels: (api.channel_count)(stream) as u32,
                burst: (api.frames_per_burst)(stream).max(1),
                capacity: (api.buffer_capacity)(stream),
                buffer: 0,
                xruns: 0,
                started: false,
                quantum: 1024,
            };
            if (player.rate, player.channels) != (rate, channels) {
                return Err(format!(
                    "AAudio stream opened with another format: rate={} channels={}",
                    player.rate, player.channels
                ));
            }
            // Blocking writes keep the buffer full, so its size is the
            // latency. Two bursts is the smallest that plays cleanly; it
            // grows when Android reports underruns.
            if player.capacity <= 0 {
                return Err(format!(
                    "invalid AAudio buffer capacity: {}",
                    player.capacity
                ));
            }
            let wanted = player.burst.saturating_mul(2).max(player.rate as i32 / 100);
            let mut buffer = (api.set_buffer_size)(stream, wanted.min(player.capacity));
            if buffer <= 0 {
                buffer = (api.buffer_size)(stream);
            }
            if buffer <= 0 || buffer > player.capacity {
                return Err(format!("invalid AAudio buffer size: {buffer}"));
            }
            player.buffer = buffer;
            note!(
                "opened AAudio stream: output={output:?} rate={} channels={} burst={} buffer={} capacity={}",
                player.rate,
                player.channels,
                player.burst,
                player.buffer,
                player.capacity
            );
            Ok(player)
        }
    }

    /// The stream's audio session, or 0 when it has none.
    fn session(&self) -> i32 {
        // SAFETY: the stream is open.
        unsafe { (self.api.session_id)(self.stream) }.max(0)
    }

    /// Start playing, with the buffer primed with silence up to where it
    /// sits once `next` frames follow. Quanta come one at a time, so an empty
    /// buffer would underrun until it had slowly filled.
    fn start(&mut self, next: usize) -> Result<(), aaudio::Res> {
        // SAFETY: the stream is open.
        let res = unsafe { (self.api.request_start)(self.stream) };
        if res != aaudio::OK {
            return Err(res);
        }
        self.started = true;
        let prime = (self.buffer as usize).saturating_sub(next);
        let silence = vec![0.0f32; prime * self.channels as usize];
        // The buffer is empty, so this does not block.
        // SAFETY: `silence` holds `prime` frames.
        let primed =
            unsafe { (self.api.write)(self.stream, silence.as_ptr().cast(), prime as i32, 0) };
        self.quantum = next;
        note!("AAudio started primed={primed}");
        Ok(())
    }

    fn stop(&mut self) {
        // SAFETY: the stream is open.
        let res = unsafe { (self.api.request_stop)(self.stream) };
        self.started = false;
        note!("AAudio stopped (silence) result={res} xruns={}", self.xruns);
    }

    /// Write interleaved frames, blocking until Android has taken them.
    fn write(&mut self, samples: &[f32]) -> Result<(), aaudio::Res> {
        let channels = self.channels as usize;
        let total = samples.len() / channels;
        let (mut done, mut stalled) = (0usize, 0u32);
        while done < total {
            // SAFETY: the slice holds `total - done` frames from `done` on.
            let written = unsafe {
                (self.api.write)(
                    self.stream,
                    samples[done * channels..].as_ptr().cast(),
                    (total - done) as i32,
                    WRITE_SLICE_NS,
                )
            };
            if written < 0 {
                return Err(written);
            }
            if written == 0 {
                stalled += 1;
                if stalled >= WRITE_STALLED_SLICES {
                    return Err(aaudio::ERROR_TIMEOUT);
                }
            } else {
                stalled = 0;
                done += written as usize;
            }
        }
        Ok(())
    }

    /// Close the stream and open a new one on Android's current route.
    fn reopen(&mut self) -> Result<(), String> {
        let started = self.started;
        self.close();
        let mut fresh = Player::open(self.output, self.rate, self.channels)?;
        if started {
            fresh
                .start(self.quantum)
                .map_err(|res| format!("AAudio restart failed: {}", self.api.text(res)))?;
        }
        *self = fresh;
        // The old stream's loss is dealt with.
        DISCONNECTED.store(false, Ordering::Release);
        Ok(())
    }

    /// Time from a frame being written to it leaving the speaker. None
    /// until AAudio reports a timestamp for the running stream.
    fn latency_ns(&self) -> Option<i64> {
        let (mut position, mut presented_ns) = (0i64, 0i64);
        // SAFETY: the stream is open and both out-pointers are valid.
        let written = unsafe {
            if (self.api.timestamp)(
                self.stream,
                aaudio::CLOCK_MONOTONIC,
                &mut position,
                &mut presented_ns,
            ) != aaudio::OK
            {
                return None;
            }
            (self.api.frames_written)(self.stream)
        };
        let mut now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `now` is a valid timespec to fill.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
        let now_ns = now.tv_sec * 1_000_000_000 + now.tv_nsec;
        let rate = self.rate as i64;
        let presented_now = position + (now_ns - presented_ns).max(0) * rate / 1_000_000_000;
        Some((written - presented_now).max(0) * 1_000_000_000 / rate)
    }

    /// Give the buffer one more burst after an underrun.
    fn grow_after_underrun(&mut self) {
        // SAFETY: the stream is open.
        let xruns = unsafe { (self.api.xrun_count)(self.stream) };
        if xruns <= self.xruns {
            return;
        }
        self.xruns = xruns;
        if self.buffer >= self.capacity {
            return;
        }
        let wanted = self.buffer.saturating_add(self.burst).min(self.capacity);
        // SAFETY: the stream is open.
        let buffer = unsafe { (self.api.set_buffer_size)(self.stream, wanted) };
        if buffer > 0 && buffer <= self.capacity {
            self.buffer = buffer;
            note!("AAudio underrun (total {xruns}): buffer now {buffer} frames");
        }
    }

    fn close(&mut self) {
        if !self.stream.is_null() {
            // SAFETY: the stream is open, and is not used after this.
            unsafe {
                (self.api.request_stop)(self.stream);
                (self.api.close)(self.stream);
            }
            self.stream = std::ptr::null_mut();
            self.started = false;
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.close();
    }
}

/// Play what the sink sends on `link` until it closes the link or Android
/// stops taking audio. `session_changed` hears the stream's audio session
/// (0: none) each time a stream opens.
pub fn serve(
    mut link: UnixStream,
    output: Output,
    rate: u32,
    channels: u32,
    session_changed: impl Fn(i32),
) -> Result<(), String> {
    // The priority Android gives its own audio threads; best effort.
    // SAFETY: plain system call on the calling thread.
    unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, -19) };

    DISCONNECTED.store(false, Ordering::Release);
    let mut player = Player::open(output, rate, channels)?;
    session_changed(player.session());

    let mut samples: Vec<f32> = Vec::new();
    let mut latencies: VecDeque<i64> = VecDeque::with_capacity(8);
    let mut latency_ns = LATENCY_UNKNOWN;
    let mut sampled = Instant::now();

    loop {
        let request = match audio_link::receive(&mut link, &mut samples) {
            Ok(Some(request)) => request,
            Ok(None) => return Ok(()),
            Err(error) => return Err(format!("audio link: {error}")),
        };

        if DISCONNECTED.swap(false, Ordering::AcqRel) {
            player.reopen()?;
            session_changed(player.session());
            // The new route has its own latency; average it afresh.
            latencies.clear();
        }

        match request {
            Request::Stop => {
                if player.started {
                    player.stop();
                }
            }
            Request::Play => {
                let frames = samples.len() / channels as usize;
                let played = if player.started {
                    Ok(())
                } else {
                    player.start(frames)
                }
                .and_then(|()| player.write(&samples));
                if let Err(res) = played {
                    // The quantum is lost; the next one plays on a new stream.
                    note!("AAudio write failed: {}; reopening", player.api.text(res));
                    player.started = true;
                    player.reopen()?;
                    session_changed(player.session());
                    latencies.clear();
                } else if sampled.elapsed() >= SAMPLE_EVERY {
                    sampled = Instant::now();
                    player.grow_after_underrun();
                    // A single sample swings by about one burst, so the
                    // average of the last 8 is what clients get.
                    if let Some(ns) = player.latency_ns() {
                        if latencies.len() == 8 {
                            latencies.pop_front();
                        }
                        latencies.push_back(ns);
                        if latencies.len() == 8 {
                            latency_ns = latencies.iter().sum::<i64>() / 8;
                        }
                    }
                }
            }
        }

        // The reply is what lets the sink run its next cycle.
        if let Err(error) = audio_link::send_reply(&mut link, latency_ns) {
            return if error.kind() == std::io::ErrorKind::BrokenPipe {
                Ok(())
            } else {
                Err(format!("audio link: {error}"))
            };
        }
    }
}
