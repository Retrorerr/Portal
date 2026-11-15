//! Portal PipeWire standalone-client AAudio sink.
//!
//! A normal PipeWire client process, not a PipeWire/SPA plugin. It registers
//! the `Audio/Sink` node that drives the guest's audio graph and hands what
//! the graph delivers to the app, which plays it on one Android AAudio
//! stream (`src/android/backend/aaudio_player.rs`). The stream lives in the
//! app's process so that Android counts it as the app's; the two talk over
//! the socket this process is started with as its standard input
//! (`src/core/audio_link.rs`).
//!
//! One driver thread does all of it: ask the graph for a quantum, wait for
//! it, send it, wait for the app to say Android has taken it. That blocking
//! write paces the graph at the speed Android plays, so there is no ring
//! buffer and no second clock; cycles are kept most of a quantum apart,
//! because Android takes audio a burst at a time and clients need the time
//! between cycles to refill. Android only sees a started stream while there
//! is something to hear: the stream starts with the first audible quantum
//! and stops after a few seconds of silence. Meanwhile the graph keeps
//! running on a timer, so guest clocks never stall. A started stream that
//! plays silence would hold audio routes for nothing, and a Bluetooth headset
//! shared with a phone could not tell that the tablet has stopped playing.
//!
//! Android-only, and behind the `pipewire-sink` feature because it needs an
//! Android PipeWire sysroot the normal APK build does not. Build it with
//! `scripts/build-pipewire-aaudio-sink.sh`, which cross-compiles it and installs
//! the result as `assets/libs/arm64-v8a/liblocaldesktop_pipewire_aaudio_sink.so`.
//! `src/android/backend/pipewire_standalone_aaudio.rs` supervises it at runtime.
//!
//! The silence gate and argument parsing below build on any host, so
//! `cargo test --features pipewire-sink --bin localdesktop_pipewire_aaudio_sink`
//! covers them without an Android sysroot.

// Keep the helper independent of the Android host library.
#[cfg(any(target_os = "android", test))]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "The helper uses only its side of the shared audio protocol"
    )
)]
#[path = "../core/audio_link.rs"]
mod audio_link;

const DEFAULT_NODE_NAME: &str = "portal-audio-output";
const DEFAULT_RATE: u32 = 48000;
const DEFAULT_CHANNELS: u32 = 2;

/// Samples up to one 16-bit step are silence. That covers digital zero and
/// the dither some clients add to it, and nothing a listener could hear.
#[cfg(any(target_os = "android", test))]
const SILENCE_PEAK: f32 = 1.0 / 32768.0;
/// Silence played before the Android stream stops, so the gaps between
/// sounds and between songs don't restart it each time.
#[cfg(any(target_os = "android", test))]
const SILENCE_HOLD_MS: u64 = 3000;

macro_rules! note {
    ($($arg:tt)*) => {
        eprintln!("[pipewire-aaudio-sink] {}", format_args!($($arg)*))
    };
}

// ----------------------------------------------------------------------------
// Silence gate
// ----------------------------------------------------------------------------

#[cfg(any(target_os = "android", test))]
fn is_audible(samples: &[f32]) -> bool {
    samples.iter().any(|sample| sample.abs() > SILENCE_PEAK)
}

/// What to do with one quantum.
#[cfg(any(target_os = "android", test))]
#[derive(Debug, PartialEq, Clone, Copy)]
enum Step {
    /// Play the quantum, which starts the Android stream.
    Start,
    Play,
    /// Stop the Android stream; the quantum is silence and is dropped.
    Stop,
    /// The stream is stopped and the quantum is silence: let its time pass.
    Idle,
}

/// Decides when the Android stream runs.
#[cfg(any(target_os = "android", test))]
struct Gate {
    hold_frames: u64,
    silent_frames: u64,
    playing: bool,
}

#[cfg(any(target_os = "android", test))]
impl Gate {
    fn new(rate: u32) -> Gate {
        Gate {
            hold_frames: rate as u64 * SILENCE_HOLD_MS / 1000,
            silent_frames: 0,
            playing: false,
        }
    }

    fn step(&mut self, audible: bool, frames: u64) -> Step {
        if audible {
            self.silent_frames = 0;
            if self.playing {
                return Step::Play;
            }
            self.playing = true;
            return Step::Start;
        }
        if !self.playing {
            return Step::Idle;
        }
        self.silent_frames += frames;
        if self.silent_frames < self.hold_frames {
            return Step::Play;
        }
        self.silent_frames = 0;
        self.playing = false;
        Step::Stop
    }
}

// ----------------------------------------------------------------------------
// Arguments
// ----------------------------------------------------------------------------

struct Args {
    node_name: String,
    rate: u32,
    channels: u32,
}

enum Parsed {
    Run(Args),
    Help,
}

fn usage() {
    eprintln!(
        "Usage: localdesktop-pipewire-aaudio-sink [--node-name NAME] [--rate HZ] [--channels N]\n\
         The audio link is supplied as a socket on standard input."
    );
}

fn parse_args(argv: &[String]) -> Result<Parsed, String> {
    let mut args = Args {
        node_name: DEFAULT_NODE_NAME.to_string(),
        rate: DEFAULT_RATE,
        channels: DEFAULT_CHANNELS,
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
            other => return Err(format!("unknown argument {other}")),
        }
        i += 2;
    }

    Ok(Parsed::Run(args))
}

// ----------------------------------------------------------------------------
// Android: the PipeWire client
// ----------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod android {
    use super::*;

    use std::ffi::c_void;
    use std::io::Cursor;
    use std::os::fd::FromRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::{Condvar, Mutex, MutexGuard};
    use std::time::{Duration, Instant};

    use pipewire as pw;
    use pw::spa;

    /// Quantum assumed until the graph has delivered one.
    const DEFAULT_QUANTUM: usize = 1024;
    const MAX_QUANTUM: usize = 8192;
    /// How long a requested graph cycle may stay unanswered before it is
    /// requested again, and how many in a row (10 s) before the sink gives
    /// up. Exiting is safe: the supervisor starts a new sink and the guest's
    /// connections stay up.
    const LOST_CYCLE: Duration = Duration::from_millis(200);
    const LOST_CYCLES_GIVE_UP: u32 = 50;
    /// How often a changed output latency is passed on to the graph.
    const REPORT_EVERY: Duration = Duration::from_millis(500);

    // -- State shared by the PipeWire loops and the driver thread --------------

    struct Cycle {
        /// The node is streaming: clients are linked and the graph wants a
        /// driver.
        streaming: bool,
        quit: bool,
        /// The requested graph cycle finished.
        done: bool,
        /// What that cycle delivered, interleaved.
        samples: Vec<f32>,
    }

    struct Shared {
        rate: u32,
        channels: usize,
        cycle: Mutex<Cycle>,
        wake: Condvar,
        /// The app's averaged output latency, or -1 before it has one.
        latency_ns: AtomicI64,
    }

    impl Shared {
        fn lock(&self) -> MutexGuard<'_, Cycle> {
            self.cycle.lock().unwrap_or_else(|e| e.into_inner())
        }

        fn update(&self, change: impl FnOnce(&mut Cycle)) {
            change(&mut self.lock());
            self.wake.notify_all();
        }

        fn frame_bytes(&self) -> u32 {
            (std::mem::size_of::<f32>() * self.channels) as u32
        }
    }

    // -- The driver thread ---------------------------------------------------

    struct StreamPtr(*mut pw::sys::pw_stream);
    // SAFETY: `pw_stream_trigger_process` may be called from any thread, and
    // the stream outlives the driver thread, which is joined before it drops.
    unsafe impl Send for StreamPtr {}

    /// Run graph cycles and play what they deliver, until told to quit.
    fn drive(shared: &Shared, stream: StreamPtr, mut link: UnixStream) -> Result<(), String> {
        // The app answers each request once Android has taken the audio, or
        // closes the link when it cannot play (the supervisor then starts a
        // new sink and a new stream).
        fn finish(link: &mut UnixStream, sent: std::io::Result<()>) -> Result<i64, String> {
            sent.and_then(|()| audio_link::receive_reply(link))
                .map_err(|error| format!("the app closed the audio link ({error})"))
        }

        // The priority Android gives its own audio threads; best effort.
        unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, -19) };

        let channels = shared.channels;
        let mut gate = Gate::new(shared.rate);
        let mut samples: Vec<f32> = Vec::with_capacity(MAX_QUANTUM * channels);
        let mut quantum = DEFAULT_QUANTUM;
        let (mut lost, mut lost_in_a_row, mut lost_noted) = (0u64, 0u32, Instant::now());
        // When the next quantum is due while the Android stream is stopped.
        let mut idle_due: Option<Instant> = None;
        let mut cycle_started = Instant::now();

        loop {
            samples.clear();
            if gate.playing {
                // Android frees buffer space a burst at a time, so the app's
                // replies alone would run several cycles back to back and then
                // none. Clients refill between cycles: a PulseAudio stream
                // asked again before it has refilled plays a whole quantum of
                // silence. Keep cycles most of a quantum apart; the rest is
                // room to catch up after a late one.
                let beat =
                    Duration::from_nanos(quantum as u64 * 1_000_000_000 / shared.rate as u64);
                let earliest = cycle_started + beat * 15 / 16;
                std::thread::sleep(earliest.saturating_duration_since(Instant::now()));
            }
            cycle_started = Instant::now();
            {
                let mut cycle = shared.lock();
                // Nothing to drive and nothing playing: sleep.
                while !cycle.quit && !cycle.streaming && !gate.playing {
                    cycle = shared.wake.wait(cycle).unwrap_or_else(|e| e.into_inner());
                }
                if cycle.quit {
                    return Ok(());
                }
                if cycle.streaming {
                    cycle.done = false;
                    cycle.samples.clear();
                    drop(cycle);
                    let res = unsafe { pw::sys::pw_stream_trigger_process(stream.0) };
                    let (mut cycle, wait) = shared
                        .wake
                        .wait_timeout_while(shared.lock(), LOST_CYCLE, |cycle| {
                            !cycle.done && !cycle.quit && cycle.streaming
                        })
                        .unwrap_or_else(|e| e.into_inner());
                    if cycle.quit {
                        return Ok(());
                    }
                    if wait.timed_out() {
                        lost += 1;
                        lost_in_a_row += 1;
                        if lost_in_a_row >= LOST_CYCLES_GIVE_UP {
                            return Err(format!(
                                "no graph cycle for {} ms lost_cycles={lost} trigger_result={res}",
                                lost_in_a_row * LOST_CYCLE.as_millis() as u32
                            ));
                        }
                        if lost_noted.elapsed() >= Duration::from_secs(10) {
                            lost_noted = Instant::now();
                            note!("recovered lost graph cycles total={lost} trigger_result={res}");
                        }
                    } else {
                        lost_in_a_row = 0;
                    }
                    std::mem::swap(&mut samples, &mut cycle.samples);
                }
            }

            // A cycle that delivered nothing, or no graph at all while the
            // silence hold runs out: that time is silence.
            if samples.is_empty() {
                samples.resize(quantum * channels, 0.0);
            } else {
                quantum = samples.len() / channels;
            }

            let step = gate.step(is_audible(&samples), quantum as u64);
            if step != Step::Idle {
                idle_due = None;
            }
            match step {
                Step::Start | Step::Play => {
                    let sent = audio_link::send_play(&mut link, &samples);
                    let latency_ns = finish(&mut link, sent)?;
                    if latency_ns >= 0 {
                        shared.latency_ns.store(latency_ns, Ordering::Release);
                    }
                }
                Step::Stop => {
                    let sent = audio_link::send_stop(&mut link);
                    finish(&mut link, sent)?;
                }
                Step::Idle => {
                    // No Android stream to pace the graph: the clock does.
                    let now = Instant::now();
                    let length =
                        Duration::from_nanos(quantum as u64 * 1_000_000_000 / shared.rate as u64);
                    let due = idle_due.map_or(now, |due| {
                        due.max(now.checked_sub(LOST_CYCLE).unwrap_or(now))
                    }) + length;
                    idle_due = Some(due);
                    std::thread::sleep(due.saturating_duration_since(now));
                }
            }
        }
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
        // Without positions the channels are AUX: pipewire-pulse then reports
        // an empty channel map and 0% volume, and Plasma shows the output as
        // muted.
        let mut position = [0u32; spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
        match channels {
            1 => position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO,
            2 => {
                position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
                position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
            }
            _ => {}
        }
        info.set_position(position);

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
        shared: &mut &'static Shared,
        old: pw::stream::StreamState,
        new: pw::stream::StreamState,
    ) {
        shared.update(|cycle| cycle.streaming = new == pw::stream::StreamState::Streaming);
        note!("stream state {old:?} -> {new:?}");
    }

    fn on_param_changed(
        stream: &pw::stream::Stream,
        shared: &mut &'static Shared,
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
        if info.rate() != shared.rate || info.channels() != shared.channels as u32 {
            note!("warning: negotiated format differs from the Android stream");
        }

        let buffers = buffers_pod(shared.rate, shared.frame_bytes());
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

    /// Hand what the cycle delivered to the driver thread.
    fn on_process(stream: &pw::stream::Stream, shared: &mut &'static Shared) {
        let frame_bytes = shared.frame_bytes() as usize;
        let mut cycle = shared.lock();
        // Normally one buffer; more if an earlier cycle was given up on.
        while let Some(mut buffer) = stream.dequeue_buffer() {
            let Some(data) = buffer.datas_mut().first_mut() else {
                continue;
            };
            let offset = data.chunk().offset() as usize;
            let size = data.chunk().size() as usize;
            let Some(bytes) = data.data() else { continue };
            let offset = offset.min(bytes.len());
            let frames = size.min(bytes.len() - offset) / frame_bytes;
            // MAP_BUFFERS memory holds F32 interleaved samples.
            let samples = unsafe {
                std::slice::from_raw_parts(
                    bytes[offset..].as_ptr() as *const f32,
                    frames * shared.channels,
                )
            };
            cycle.samples.extend_from_slice(samples);
        }
    }

    /// The requested graph cycle is over, whether or not it delivered a
    /// buffer (`process` only runs when it did).
    unsafe extern "C" fn on_trigger_done(data: *mut c_void) {
        // SAFETY: registered with the leaked `Shared` in `run_pipewire`.
        let shared = unsafe { &*(data as *const Shared) };
        shared.update(|cycle| cycle.done = true);
    }

    // -- Entry point ---------------------------------------------------------

    fn run_pipewire(
        shared: &'static Shared,
        link: UnixStream,
        node_name: &str,
    ) -> Result<(), String> {
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
            *pw::keys::AUDIO_RATE => shared.rate.to_string(),
            *pw::keys::AUDIO_CHANNELS => shared.channels.to_string(),
            "audio.position" => if shared.channels == 1 { "MONO" } else { "FL,FR" },
        };

        let stream = pw::stream::StreamRc::new(core, node_name, props)
            .map_err(|e| format!("failed to create PipeWire stream: {e}"))?;

        let _listener = stream
            .add_local_listener_with_user_data(shared)
            .state_changed(on_state_changed)
            .param_changed(on_param_changed)
            .process(on_process)
            .register()
            .map_err(|e| format!("failed to register stream listener: {e}"))?;

        // The Rust binding has no `trigger_done`, so it gets a listener of
        // its own. Both live as long as the process.
        unsafe {
            let events: &'static mut pw::sys::pw_stream_events =
                Box::leak(Box::new(std::mem::zeroed()));
            events.version = pw::sys::PW_VERSION_STREAM_EVENTS;
            events.trigger_done = Some(on_trigger_done);
            let hook: &'static mut spa::sys::spa_hook = Box::leak(Box::new(std::mem::zeroed()));
            pw::sys::pw_stream_add_listener(
                stream.as_raw_ptr(),
                hook,
                events,
                shared as *const Shared as *mut c_void,
            );
        }

        let format = enum_format_pod(shared.rate, shared.channels as u32);
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

        let driver = {
            let stream = StreamPtr(stream.as_raw_ptr());
            std::thread::Builder::new()
                .name("aaudio-driver".into())
                .spawn(move || {
                    if let Err(reason) = drive(shared, stream, link) {
                        // The supervisor starts a new sink.
                        note!("{reason}; exiting");
                        std::process::exit(1);
                    }
                })
                .map_err(|e| format!("failed to start the driver thread: {e}"))?
        };

        // Report the Android output latency so clients keep audio and video
        // in sync (Firefox reads it through pipewire-pulse). It moves with
        // the route, so it is reported whenever it has moved by 10 ms.
        let latency_stream = stream.clone();
        let reported_ns = std::cell::Cell::new(0i64);
        let latency_timer = mainloop.loop_().add_timer(move |_| {
            let ns = shared.latency_ns.load(Ordering::Acquire);
            if ns < 0 || (ns - reported_ns.get()).abs() < 10_000_000 {
                return;
            }
            let bytes = process_latency_pod(ns);
            let Some(pod) = spa::pod::Pod::from_bytes(&bytes) else {
                return;
            };
            match latency_stream.update_params(&mut [pod]) {
                Ok(()) => {
                    note!("output latency {} ms", ns / 1_000_000);
                    reported_ns.set(ns);
                }
                Err(e) => note!("failed to report latency: {e}"),
            }
        });
        latency_timer.update_timer(Some(REPORT_EVERY), Some(REPORT_EVERY));

        note!(
            "running node={node_name} rate={} channels={}",
            shared.rate,
            shared.channels
        );
        mainloop.run();

        shared.update(|cycle| cycle.quit = true);
        let _ = driver.join();
        note!("stopped");
        Ok(())
    }

    pub fn run(args: Args) -> Result<(), String> {
        pw::init();

        // SAFETY: the supervisor starts the sink with the audio link as its
        // standard input, which nothing else in this process reads.
        let link = unsafe { UnixStream::from_raw_fd(0) };
        let shared: &'static Shared = Box::leak(Box::new(Shared {
            rate: args.rate,
            channels: args.channels as usize,
            cycle: Mutex::new(Cycle {
                streaming: false,
                quit: false,
                done: false,
                samples: Vec::with_capacity(MAX_QUANTUM * args.channels as usize),
            }),
            wake: Condvar::new(),
            latency_ns: AtomicI64::new(audio_link::LATENCY_UNKNOWN),
        }));
        let result = run_pipewire(shared, link, &args.node_name);

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
            (d.node_name.as_str(), d.rate, d.channels),
            (DEFAULT_NODE_NAME, 48000, 2)
        );

        let a = args(&["--node-name", "x", "--rate", "44100", "--channels", "1"]);
        assert_eq!((a.node_name.as_str(), a.rate, a.channels), ("x", 44100, 1));

        assert!(parse_args(&["--rate".into(), "0".into()]).is_err());
        assert!(parse_args(&["--rate".into()]).is_err());
        assert!(parse_args(&["--nope".into(), "1".into()]).is_err());
        assert!(matches!(parse_args(&["--help".into()]), Ok(Parsed::Help)));
    }

    #[test]
    fn zero_and_dither_are_silence() {
        assert!(!is_audible(&[0.0; 64]));
        assert!(!is_audible(&[SILENCE_PEAK, -SILENCE_PEAK, f32::NAN]));
        assert!(is_audible(&[0.0, 0.0, -2.0 * SILENCE_PEAK]));
    }

    #[test]
    fn the_stream_starts_with_the_first_sound() {
        let mut gate = Gate::new(48000);
        // Clients connected but silent: Android sees no stream.
        for _ in 0..1000 {
            assert_eq!(gate.step(false, 1024), Step::Idle);
        }
        assert_eq!(gate.step(true, 1024), Step::Start);
        assert_eq!(gate.step(true, 1024), Step::Play);
    }

    #[test]
    fn the_stream_stops_after_the_silence_hold() {
        let mut gate = Gate::new(48000);
        assert_eq!(gate.step(true, 1024), Step::Start);
        // 3 s at 48 kHz is 140.6 quanta of 1024 frames.
        for _ in 0..140 {
            assert_eq!(gate.step(false, 1024), Step::Play);
        }
        assert_eq!(gate.step(false, 1024), Step::Stop);
        assert_eq!(gate.step(false, 1024), Step::Idle);
        assert_eq!(gate.step(true, 1024), Step::Start);
    }

    #[test]
    fn a_short_gap_keeps_the_stream_running() {
        let mut gate = Gate::new(48000);
        assert_eq!(gate.step(true, 1024), Step::Start);
        for _ in 0..10 {
            for _ in 0..100 {
                assert_eq!(gate.step(false, 1024), Step::Play);
            }
            assert_eq!(gate.step(true, 1024), Step::Play);
        }
    }
}
