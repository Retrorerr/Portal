//! Android game controllers as guest evdev pads.
//!
//! Android gives apps neither `/dev/input` nor `/dev/uinput`. Portal
//! exposes a raw pty for discovery and writes Linux input_event records
//! to its master. tawcroot requests a separate pty for each open of a pad,
//! so every reader gets the same events as it would from real evdev.
//! Both engines answer evdev ioctls for ptys registered in `fake-input/desc`.
//! Rumble comes back through each master to the controller's vibrator.

use std::collections::HashMap;
use std::ffi::CStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use jni::objects::{GlobalRef, JClass, JFloatArray, JValue};
use jni::sys::{jboolean, jint};
use jni::{JNIEnv, JavaVM};

use crate::core::gamepad::{self, EffectTable, PadState, Rumble, INPUT_EVENT_SIZE, PAD_NAME};

/// Device id of the on-screen touch controller (never a real Android id).
pub const TOUCH_PAD_ID: i32 = -100;

/// Host directory bound to the guest's `/dev/input`.
pub fn input_dir() -> PathBuf {
    Path::new(crate::core::config::APP_FILES_ROOT).join("fake-input/input")
}

/// Host directory PRoot reads pad registrations from
/// (`PROOT_FAKE_EVDEV_DIR`).
pub fn registry_dir() -> PathBuf {
    Path::new(crate::core::config::APP_FILES_ROOT).join("fake-input/desc")
}

struct PadStream {
    master: Arc<File>,
    pts: u32,
    stop: Arc<AtomicBool>,
    needs_state: bool,
}

struct Pad {
    slot: usize,
    // Keep the discovery node and its termios alive, including on PRoot.
    _discovery_slave: File,
    discovery: PadStream,
    streams: Vec<PadStream>,
    state: PadState,
    sent: PadState,
}

#[derive(Default)]
struct Hub {
    pads: HashMap<i32, Pad>,
    cleared: bool,
}

static HUB: OnceLock<Mutex<Hub>> = OnceLock::new();
static BRIDGE: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();
static READER_BROKER: OnceLock<()> = OnceLock::new();

fn hub() -> &'static Mutex<Hub> {
    HUB.get_or_init(|| Mutex::new(Hub::default()))
}

/// Remove pads a previous Portal process left behind: their ptys died
/// with it, so the links dangle.
fn clear_stale() {
    for dir in [input_dir(), registry_dir()] {
        let _ = fs::create_dir_all(&dir);
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

fn open_pty() -> std::io::Result<(File, File, u32)> {
    unsafe {
        // Non-blocking: a pad nobody reads must never stall Android's
        // input thread (see `flush`).
        let master =
            libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC | libc::O_NONBLOCK);
        if master < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let master = File::from_raw_fd(master);
        if libc::grantpt(master.as_raw_fd()) != 0 || libc::unlockpt(master.as_raw_fd()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut name = [0 as libc::c_char; 64];
        if libc::ptsname_r(master.as_raw_fd(), name.as_mut_ptr(), name.len()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let path = CStr::from_ptr(name.as_ptr()).to_string_lossy().into_owned();
        let pts = path
            .strip_prefix("/dev/pts/")
            .and_then(|number| number.parse::<u32>().ok())
            .ok_or_else(|| std::io::Error::other(format!("unexpected pty {path}")))?;
        let slave = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC)
            .open(&path)?;
        let mut termios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(slave.as_raw_fd(), &mut termios) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        libc::cfmakeraw(&mut termios);
        if libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &termios) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok((master, slave, pts))
    }
}

fn free_slot(hub: &Hub) -> usize {
    (0..)
        .find(|slot| hub.pads.values().all(|pad| pad.slot != *slot))
        .unwrap_or(0)
}

fn event_link(slot: usize) -> PathBuf {
    input_dir().join(format!("event{slot}"))
}

fn start_reader_broker() -> io::Result<()> {
    if READER_BROKER.get().is_some() {
        return Ok(());
    }
    let socket_path = input_dir().with_file_name("readers.sock");
    match fs::remove_file(&socket_path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = UnixListener::bind(socket_path)?;
    std::thread::Builder::new()
        .name("gamepad-open".into())
        .spawn(move || {
            for peer in listener.incoming().flatten() {
                if let Err(error) = serve_reader(peer) {
                    log::debug!("gamepad: could not open reader: {error}");
                }
            }
        })?;
    let _ = READER_BROKER.set(());
    Ok(())
}

/// Exchange native-endian pty numbers with tawcroot. Keep the slave live
/// until the engine has opened it and closed the request socket.
fn serve_reader(mut peer: UnixStream) -> io::Result<()> {
    let timeout = Some(Duration::from_millis(250));
    peer.set_read_timeout(timeout)?;
    peer.set_write_timeout(timeout)?;
    let mut request = [0; 4];
    peer.read_exact(&mut request)?;
    let discovery = u32::from_ne_bytes(request);
    let (slave, pts) = {
        let mut hub = hub()
            .lock()
            .map_err(|_| io::Error::other("pad hub unavailable"))?;
        let (&device_id, pad) = hub
            .pads
            .iter_mut()
            .find(|(_, pad)| pad.discovery.pts == discovery)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "pad was unplugged"))?;
        if pad.streams.len() >= 32 {
            return Err(io::Error::other("too many readers for the pad"));
        }
        let (master, slave, pts) = open_pty()?;
        let mut stream = PadStream {
            master: Arc::new(master),
            pts,
            stop: Arc::default(),
            needs_state: false,
        };
        let entry = registry_dir().join(pts.to_string());
        fs::write(&entry, PAD_NAME)?;
        if let Err(error) = spawn_reader(device_id, &stream, true) {
            let _ = fs::remove_file(entry);
            return Err(error);
        }
        let initial = pad.state.diff(&PadState::default());
        if !initial.is_empty() {
            stream.needs_state = !write_frame(&stream, &encode_frame(&initial));
        }
        pad.streams.push(stream);
        (slave, pts)
    };
    peer.write_all(&pts.to_ne_bytes())?;
    let _ = peer.read(&mut [0]);
    drop(slave);
    Ok(())
}

fn add_pad(hub: &mut Hub, device_id: i32) -> Option<&mut Pad> {
    if !hub.cleared {
        clear_stale();
        hub.cleared = true;
    }
    if !hub.pads.contains_key(&device_id) {
        if let Err(error) = start_reader_broker() {
            log::warn!("gamepad: could not start reader broker: {error}");
        }
        let (master, slave, pts) = match open_pty() {
            Ok(pty) => pty,
            Err(error) => {
                log::warn!("gamepad: could not create a pad pty: {error}");
                return None;
            }
        };
        let slot = free_slot(hub);
        // Register before linking, so a program reacting to the new node
        // already gets evdev answers from PRoot.
        if let Err(error) = fs::write(registry_dir().join(pts.to_string()), PAD_NAME) {
            log::warn!("gamepad: could not register pad pty {pts}: {error}");
            return None;
        }
        let link = event_link(slot);
        let _ = fs::remove_file(&link);
        if let Err(error) = std::os::unix::fs::symlink(format!("/dev/pts/{pts}"), &link) {
            log::warn!("gamepad: could not link {}: {error}", link.display());
            let _ = fs::remove_file(registry_dir().join(pts.to_string()));
            return None;
        }
        let discovery = PadStream {
            master: Arc::new(master),
            pts,
            stop: Arc::default(),
            needs_state: false,
        };
        if let Err(error) = spawn_reader(device_id, &discovery, false) {
            log::warn!("gamepad: could not start the rumble reader: {error}");
        }
        log::info!("gamepad: device {device_id} is /dev/input/event{slot} (pty {pts})");
        hub.pads.insert(
            device_id,
            Pad {
                slot,
                _discovery_slave: slave,
                discovery,
                streams: Vec::new(),
                state: PadState::default(),
                sent: PadState::default(),
            },
        );
    }
    hub.pads.get_mut(&device_id)
}

fn remove_pad(hub: &mut Hub, device_id: i32) {
    if let Some(pad) = hub.pads.remove(&device_id) {
        // Unlink first: SDL and Wine notice removals through inotify, then
        // the hang-up of the closed pty ends their reads.
        let _ = fs::remove_file(event_link(pad.slot));
        for stream in std::iter::once(&pad.discovery).chain(&pad.streams) {
            close_stream(stream);
        }
        log::info!("gamepad: device {device_id} removed (event{})", pad.slot);
    }
}

fn close_stream(stream: &PadStream) {
    let _ = fs::remove_file(registry_dir().join(stream.pts.to_string()));
    stream.stop.store(true, Ordering::Relaxed);
}

fn encode_frame(events: &[gamepad::EvdevEvent]) -> Vec<u8> {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut now) };
    gamepad::encode_events(events, now.tv_sec, now.tv_nsec / 1000)
}

fn flush(pad: &mut Pad) {
    let events = pad.state.diff(&pad.sent);
    if events.is_empty() {
        return;
    }
    let bytes = encode_frame(&events);
    let mut full_frame = None;
    for stream in std::iter::once(&mut pad.discovery).chain(&mut pad.streams) {
        if !stream.needs_state && write_frame(stream, &bytes) {
            continue;
        }
        // A slow reader must never stall input or affect another reader.
        // Drop only its backlog and enqueue the whole current state.
        stream.needs_state = true;
        let Ok(slave) = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
            .open(format!("/dev/pts/{}", stream.pts))
        else {
            continue;
        };
        if unsafe { libc::tcflush(slave.as_raw_fd(), libc::TCIFLUSH) } != 0 {
            continue;
        }
        let current = full_frame.get_or_insert_with(|| {
            let mut unknown = pad.state;
            for button in &mut unknown.buttons {
                *button = !*button;
            }
            for axis in &mut unknown.axes {
                *axis = axis.wrapping_add(1);
            }
            encode_frame(&pad.state.diff(&unknown))
        });
        stream.needs_state = !write_frame(stream, current);
    }
    pad.sent = pad.state;
}

/// Whole-frame write to the non-blocking master; false on a short write.
fn write_frame(stream: &PadStream, bytes: &[u8]) -> bool {
    loop {
        match (&*stream.master).write(bytes) {
            Ok(count) => return count == bytes.len(),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
}

fn spawn_reader(device_id: i32, stream: &PadStream, dedicated: bool) -> io::Result<()> {
    let master = stream.master.clone();
    let stop = stream.stop.clone();
    let pts = stream.pts;
    std::thread::Builder::new()
        .name(format!("gamepad-{device_id}"))
        .spawn(move || {
            let mut effects = EffectTable::default();
            let mut rumbling = false;
            let mut pending = Vec::with_capacity(INPUT_EVENT_SIZE * 16);
            let mut buffer = [0u8; INPUT_EVENT_SIZE * 16];
            while !stop.load(Ordering::Relaxed) {
                let mut poll = libc::pollfd {
                    fd: master.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ready = unsafe { libc::poll(&mut poll, 1, 500) };
                if dedicated && poll.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
                    break;
                }
                if ready <= 0 || poll.revents & libc::POLLIN == 0 {
                    continue;
                }
                let count = match (&*master).read(&mut buffer) {
                    Ok(0) | Err(_) => {
                        std::thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    Ok(count) => count,
                };
                pending.extend_from_slice(&buffer[..count]);
                let (records, _) = pending.as_chunks::<INPUT_EVENT_SIZE>();
                let whole = records.len() * INPUT_EVENT_SIZE;
                for record in records {
                    if let Some(rumble) =
                        gamepad::parse_request(record).and_then(|request| effects.apply(request))
                    {
                        rumbling = rumble.strong != 0 || rumble.weak != 0;
                        vibrate(device_id, rumble);
                    }
                }
                pending.drain(..whole);
            }
            if rumbling {
                vibrate(device_id, Rumble::default());
            }
            if dedicated {
                // Hold the master until the registry is gone so its pty
                // number cannot be reused by another stream meanwhile.
                if let Ok(mut hub) = hub().lock() {
                    if let Some(pad) = hub.pads.get_mut(&device_id) {
                        pad.streams.retain(|stream| {
                            if stream.pts != pts {
                                return true;
                            }
                            close_stream(stream);
                            false
                        });
                    }
                }
            }
        })?;
    Ok(())
}

fn vibrate(device_id: i32, rumble: Rumble) {
    let Some((vm, class)) = BRIDGE.get() else {
        return;
    };
    let Ok(mut env) = vm.attach_current_thread_permanently() else {
        return;
    };
    let class: &JClass = class.as_obj().into();
    if env
        .call_static_method(
            class,
            "rumble",
            "(IIII)V",
            &[
                JValue::Int(device_id),
                JValue::Int(i32::from(rumble.strong)),
                JValue::Int(i32::from(rumble.weak)),
                JValue::Int(i32::from(rumble.duration_ms)),
            ],
        )
        .is_err()
    {
        let _ = env.exception_clear();
    }
}

fn remember_bridge(env: &mut JNIEnv, class: &JClass) {
    if BRIDGE.get().is_some() {
        return;
    }
    if let (Ok(vm), Ok(class)) = (env.get_java_vm(), env.new_global_ref(class)) {
        let _ = BRIDGE.set((vm, class));
    }
}

/// Apply a change to one pad and send what changed.
fn with_pad(device_id: i32, change: impl FnOnce(&mut PadState) -> bool) -> bool {
    let Ok(mut hub) = hub().lock() else {
        return false;
    };
    let Some(pad) = add_pad(&mut hub, device_id) else {
        return false;
    };
    let handled = change(&mut pad.state);
    flush(pad);
    note_activity(&hub);
    handled
}

/// Pads reach the guest through their own evdev ptys, not the Anland
/// session, so tell its presentation pacing that the player is active.
fn note_activity(hub: &Hub) {
    let engaged = hub.pads.values().any(|pad| pad.state.engaged());
    crate::android::anland::note_gamepad_input(engaged);
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_GamepadBridge_nativeKey(
    mut env: JNIEnv,
    class: JClass,
    device_id: jint,
    key_code: jint,
    down: jboolean,
) -> jboolean {
    remember_bridge(&mut env, &class);
    u8::from(with_pad(device_id, |state| {
        state.apply_key(key_code, down != 0)
    }))
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_GamepadBridge_nativeAxes(
    mut env: JNIEnv,
    class: JClass,
    device_id: jint,
    axes: JFloatArray,
) {
    remember_bridge(&mut env, &class);
    let mut values = [0f32; 10];
    if env.get_float_array_region(&axes, 0, &mut values).is_err() {
        let _ = env.exception_clear();
        return;
    }
    with_pad(device_id, |state| {
        state.apply_axes(&values);
        true
    });
}

/// The on-screen controller sends its whole state at once.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_GamepadBridge_nativeTouchPad(
    mut env: JNIEnv,
    class: JClass,
    buttons: jint,
    axes: JFloatArray,
) {
    remember_bridge(&mut env, &class);
    let mut values = [0f32; 10];
    if env.get_float_array_region(&axes, 0, &mut values).is_err() {
        let _ = env.exception_clear();
        return;
    }
    with_pad(TOUCH_PAD_ID, |state| {
        state.apply_axes(&values);
        for (index, held) in state.buttons.iter_mut().enumerate() {
            *held = buttons & (1 << index) != 0;
        }
        true
    });
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_GamepadBridge_nativeRemoved(
    _env: JNIEnv,
    _class: JClass,
    device_id: jint,
) {
    if let Ok(mut hub) = hub().lock() {
        remove_pad(&mut hub, device_id);
        note_activity(&hub);
    }
}
