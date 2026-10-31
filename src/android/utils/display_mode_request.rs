//! Window-level high-refresh request, driven by Anland's demand state.
//!
//! OxygenOS votes Portal's GameActivity window down to a 60 Hz app request,
//! which outranks the SurfaceView's `ANativeWindow_setFrameRate` hint: every
//! interactive burst presented at exactly 60 Hz on a 120/144 Hz panel. The
//! supported override is the window's `preferredDisplayModeId`
//! (`PortalActivity.setHighRefreshPreferred`), still capped by the user's
//! peak-refresh setting. Holding it permanently would pin the panel at its
//! peak on an idle desktop, so it follows presentation demand instead.
//!
//! Callers are latency-sensitive (the Anland render thread), so requests are
//! deduplicated here and the JNI call runs on a dedicated worker thread.

use std::sync::{
    atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering},
    mpsc, Mutex, OnceLock,
};
use std::time::Instant;

use jni::objects::{JObject, JValue};
use jni::sys::_jobject;
use winit::platform::android::activity::AndroidApp;

const UNKNOWN: u8 = 0;
const LOW: u8 = 1;
const HIGH: u8 = 2;

/// Last state handed to the worker (dedupe for per-frame callers).
static REQUESTED: AtomicU8 = AtomicU8::new(UNKNOWN);

/// State of the surface frame-rate vote, which follows the same demand.
static SURFACE_VOTE: AtomicU8 = AtomicU8::new(UNKNOWN);

/// When the held high-refresh request was last resolved (ms since `epoch()`).
static RESOLVED_MS: AtomicU64 = AtomicU64::new(0);

/// A held high-refresh request is resolved again this often. OnePlus Games
/// applies its per-app refresh override just after Portal returns to the
/// foreground, so a resume can read the system-wide peak (120 Hz) while the
/// panel then runs at the app's 144 Hz. Resolving only on a fresh request left
/// a continuously busy session requesting the 120 Hz mode and advertising
/// 120 Hz to KWin on a 144 Hz panel.
const RESOLVE_INTERVAL_MS: u64 = 2_000;

fn epoch() -> &'static Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now)
}

/// Refresh (millihertz) of the mode the latest high-refresh request asked
/// for, re-resolved against the live peak-refresh setting. 0 = unknown.
static TARGET_MILLIHZ: AtomicU32 = AtomicU32::new(0);

/// The refresh rate the display runs at while Portal holds its high-refresh
/// request, as of the latest request. Lets the session follow a peak-refresh
/// setting changed while Portal is running.
pub fn target_millihz() -> Option<u32> {
    Some(TARGET_MILLIHZ.load(Ordering::Acquire)).filter(|millihz| *millihz > 0)
}

fn app_slot() -> &'static Mutex<Option<AndroidApp>> {
    static APP: OnceLock<Mutex<Option<AndroidApp>>> = OnceLock::new();
    APP.get_or_init(|| Mutex::new(None))
}

fn worker() -> Option<&'static Mutex<mpsc::Sender<bool>>> {
    static WORKER: OnceLock<Option<Mutex<mpsc::Sender<bool>>>> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<bool>();
            std::thread::Builder::new()
                .name("portal-refresh".into())
                .spawn(move || {
                    while let Ok(mut enable) = rx.recv() {
                        // Only the newest request matters.
                        while let Ok(next) = rx.try_recv() {
                            enable = next;
                        }
                        apply(enable);
                    }
                })
                .map_err(|error| log::warn!("display-mode request worker unavailable: {error}"))
                .ok()
                .map(|_| Mutex::new(tx))
        })
        .as_ref()
}

/// Remember the activity used for requests. Called on every surface attach;
/// forgets the previous request because a new window starts without one.
pub fn attach(android_app: &AndroidApp) {
    if let Ok(mut slot) = app_slot().lock() {
        *slot = Some(android_app.clone());
    }
    REQUESTED.store(UNKNOWN, Ordering::Release);
    SURFACE_VOTE.store(UNKNOWN, Ordering::Release);
    TARGET_MILLIHZ.store(0, Ordering::Release);
}

/// Request (or release) the high-refresh display mode. Cheap and
/// non-blocking; repeated identical requests are dropped, except that a held
/// high-refresh request is re-resolved every `RESOLVE_INTERVAL_MS`.
pub fn request(enable: bool) {
    let state = if enable { HIGH } else { LOW };
    let now_ms = epoch().elapsed().as_millis() as u64;
    if REQUESTED.swap(state, Ordering::AcqRel) == state
        && (!enable
            || now_ms.saturating_sub(RESOLVED_MS.load(Ordering::Acquire)) < RESOLVE_INTERVAL_MS)
    {
        return;
    }
    RESOLVED_MS.store(now_ms, Ordering::Release);
    let sent = worker()
        .and_then(|tx| tx.lock().ok())
        .is_some_and(|tx| tx.send(enable).is_ok());
    if !sent {
        REQUESTED.store(UNKNOWN, Ordering::Release);
    }
}

fn apply(enable: bool) {
    let Some(app) = app_slot().lock().ok().and_then(|slot| slot.clone()) else {
        return;
    };
    super::ndk::run_in_jvm(
        |env, app| {
            let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as *mut _jobject) };
            let mut changed = !enable;
            if enable {
                match env
                    .call_method(&activity, "highRefreshTargetMillihz", "()I", &[])
                    .and_then(|value| value.i())
                {
                    Ok(millihz) if millihz > 0 => {
                        changed = TARGET_MILLIHZ.swap(millihz as u32, Ordering::AcqRel)
                            != millihz as u32;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        log::warn!("display-mode target query failed: {error}");
                        let _ = env.exception_clear();
                    }
                }
            }
            if let Err(error) = env.call_method(
                activity,
                "setHighRefreshPreferred",
                "(Z)V",
                &[JValue::Bool(enable.into())],
            ) {
                log::warn!("display-mode request (enable={enable}) failed: {error}");
                let _ = env.exception_clear();
                REQUESTED.store(UNKNOWN, Ordering::Release);
            } else if changed {
                log::info!("anland.refresh window high-refresh request enable={enable}");
            }
        },
        app.clone(),
    );
    // The surface's AT_LEAST vote follows the same demand: held while idle it
    // keeps the panel at peak refresh even with the window request released.
    let state = if enable { HIGH } else { LOW };
    let transition = SURFACE_VOTE.swap(state, Ordering::AcqRel) != state;
    let retarget = changed_target();
    if transition || (enable && retarget) {
        if enable {
            let rate_hz = target_millihz()
                .map(|millihz| millihz as f32 / 1000.0)
                .unwrap_or_else(|| super::frame_rate::preferred_frame_rate_hz(&app));
            super::frame_rate::ensure_high_refresh_rate_hz(&app, rate_hz);
        } else {
            super::frame_rate::clear_frame_rate(&app);
        }
    }
}

/// The target refresh changed since the surface vote was last set.
fn changed_target() -> bool {
    static VOTED_MILLIHZ: AtomicU32 = AtomicU32::new(0);
    let target = TARGET_MILLIHZ.load(Ordering::Acquire);
    VOTED_MILLIHZ.swap(target, Ordering::AcqRel) != target
}
