//! Android game controllers as guest evdev pads.
//!
//! Android gives apps neither `/dev/input` nor `/dev/uinput`, so each pad
//! is a raw pseudo-terminal: Portal holds the master and writes
//! `struct input_event` records to it, and `/dev/input/eventN` in the guest
//! (a bind of `fake-input/input`) links to the slave. PRoot answers the
//! evdev ioctls for every pty registered in `fake-input/desc`
//! (`syscall/evdev.c`), so SDL, Wine, Steam and x86 programs under FEX all
//! see an ordinary Xbox 360 pad. Rumble the guest plays comes back on the
//! master and is forwarded to the controller's vibrator.

use std::collections::HashMap;
use std::ffi::CStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use jni::objects::{GlobalRef, JClass, JFloatArray, JValue};
use jni::sys::{jboolean, jint};
use jni::{JNIEnv, JavaVM};

use crate::core::gamepad::{
    self, EffectTable, PadState, Rumble, INPUT_EVENT_SIZE, PAD_NAME,
};

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

struct Pad {
    slot: usize,
    master: Arc<File>,
    // Held so the master never reads EIO while no guest program has the
    // pad open, and so the raw termios settings persist.
    _slave: File,
    pts: u32,
    state: PadState,
    sent: PadState,
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
struct Hub {
    pads: HashMap<i32, Pad>,
    cleared: bool,
}

static HUB: OnceLock<Mutex<Hub>> = OnceLock::new();
static BRIDGE: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();

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
        let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
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

fn add_pad(hub: &mut Hub, device_id: i32) -> Option<&mut Pad> {
    if !hub.cleared {
        clear_stale();
        hub.cleared = true;
    }
    if !hub.pads.contains_key(&device_id) {
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
        let master = Arc::new(master);
        let stop = Arc::new(AtomicBool::new(false));
        spawn_reader(device_id, master.clone(), stop.clone());
        log::info!("gamepad: device {device_id} is /dev/input/event{slot} (pty {pts})");
        hub.pads.insert(
            device_id,
            Pad {
                slot,
                master,
                _slave: slave,
                pts,
                state: PadState::default(),
                sent: PadState::default(),
                stop,
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
        let _ = fs::remove_file(registry_dir().join(pad.pts.to_string()));
        pad.stop.store(true, Ordering::Relaxed);
        log::info!("gamepad: device {device_id} removed (event{})", pad.slot);
    }
}

fn flush(pad: &mut Pad) {
    let events = pad.state.diff(&pad.sent);
    if events.is_empty() {
        return;
    }
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut now) };
    let bytes = gamepad::encode_events(&events, now.tv_sec, now.tv_nsec / 1000);
    // One write per frame keeps a frame contiguous in the pty buffer.
    match (&*pad.master).write_all(&bytes) {
        Ok(()) => pad.sent = pad.state,
        Err(error) => log::debug!("gamepad: event write failed: {error}"),
    }
}

fn spawn_reader(device_id: i32, master: Arc<File>, stop: Arc<AtomicBool>) {
    let spawned = std::thread::Builder::new()
        .name(format!("gamepad-{device_id}"))
        .spawn(move || {
            let mut effects = EffectTable::default();
            let mut pending = Vec::with_capacity(INPUT_EVENT_SIZE * 16);
            let mut buffer = [0u8; INPUT_EVENT_SIZE * 16];
            while !stop.load(Ordering::Relaxed) {
                let mut poll = libc::pollfd { fd: master.as_raw_fd(), events: libc::POLLIN, revents: 0 };
                let ready = unsafe { libc::poll(&mut poll, 1, 500) };
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
                let whole = pending.len() / INPUT_EVENT_SIZE * INPUT_EVENT_SIZE;
                for record in pending[..whole].chunks_exact(INPUT_EVENT_SIZE) {
                    let record: &[u8; INPUT_EVENT_SIZE] = record.try_into().unwrap();
                    if let Some(rumble) = gamepad::parse_request(record).and_then(|request| effects.apply(request)) {
                        vibrate(device_id, rumble);
                    }
                }
                pending.drain(..whole);
            }
            vibrate(device_id, Rumble::default());
        });
    if let Err(error) = spawned {
        log::warn!("gamepad: could not start the rumble reader: {error}");
    }
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
    handled
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
    u8::from(with_pad(device_id, |state| state.apply_key(key_code, down != 0)))
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
    }
}
