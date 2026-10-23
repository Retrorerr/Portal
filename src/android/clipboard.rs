//! Android clipboard access for the nested Wayland session.
//!
//! The compositor owns the Wayland selection, while Android owns the system clipboard.  This
//! module deliberately contains only the Android/JNI side and a small change detector; the
//! compositor can call it from its selection callbacks without duplicating fragile JNI code.

use jni::{
    objects::{JObject, JString, JValue},
    JNIEnv,
};
use std::{
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    thread,
    time::Duration,
};
use winit::event_loop::EventLoopProxy;
use winit::platform::android::activity::AndroidApp;

use crate::android::accessibility::{event_loop_proxy, AppUserEvent};
use crate::android::clipboard_broker::{self, BrokerHandle, GuestClipboardCallback};
use crate::android::utils::ndk::run_in_jvm;
use crate::core::clipboard_policy::{validate_clip_text, MAX_CLIPBOARD_BYTES};
use crate::core::clipboard_sync::ExternalClipboardSync;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardError(String);

impl fmt::Display for ClipboardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ClipboardError {}

fn jni_error(context: &str, error: impl fmt::Display) -> ClipboardError {
    ClipboardError(format!("{context}: {error}"))
}

fn activity(android_app: &AndroidApp) -> JObject<'static> {
    // AndroidApp owns the NativeActivity reference for the process.  The JNI local reference is
    // used only during the attached call and is not retained by this module.
    unsafe { JObject::from_raw(android_app.activity_as_ptr() as *mut _) }
}

fn clipboard_manager<'local>(
    env: &mut JNIEnv<'local>,
    activity: &JObject<'local>,
) -> Result<JObject<'local>, ClipboardError> {
    let service = env
        .new_string("clipboard")
        .map_err(|error| jni_error("create clipboard service name", error))?;
    env.call_method(
        activity,
        "getSystemService",
        "(Ljava/lang/String;)Ljava/lang/Object;",
        &[(&service).into()],
    )
    .map_err(|error| jni_error("get ClipboardManager", error))?
    .l()
    .map_err(|error| jni_error("read ClipboardManager", error))
}

fn read_text_inner<'local>(
    env: &mut JNIEnv<'local>,
    android_app: &AndroidApp,
) -> Result<Option<String>, ClipboardError> {
    let activity = activity(android_app);
    let manager = clipboard_manager(env, &activity)?;
    if manager.is_null() {
        return Ok(None);
    }

    let clip = env
        .call_method(
            &manager,
            "getPrimaryClip",
            "()Landroid/content/ClipData;",
            &[],
        )
        .map_err(|error| jni_error("get Android primary clip", error))?
        .l()
        .map_err(|error| jni_error("read Android primary clip", error))?;
    // Android answers a denied read (the activity is not focused yet) with a
    // null clip, indistinguishable from an empty clipboard. Treat it as
    // unavailable: a clear manufactured from a denial would wipe the guest.
    if clip.is_null() {
        return Err(ClipboardError("no readable Android clip".into()));
    }

    let item_count = env
        .call_method(&clip, "getItemCount", "()I", &[])
        .map_err(|error| jni_error("get Android clip item count", error))?
        .i()
        .map_err(|error| jni_error("read Android clip item count", error))?;
    if item_count <= 0 {
        return Err(ClipboardError("Android clip has no items".into()));
    }

    let item = env
        .call_method(
            &clip,
            "getItemAt",
            "(I)Landroid/content/ClipData$Item;",
            &[JValue::Int(0)],
        )
        .map_err(|error| jni_error("get Android clip item", error))?
        .l()
        .map_err(|error| jni_error("read Android clip item", error))?;
    if item.is_null() {
        return Err(ClipboardError("Android clip item is null".into()));
    }

    let text = env
        .call_method(
            &item,
            "coerceToText",
            "(Landroid/content/Context;)Ljava/lang/CharSequence;",
            &[JValue::Object(&activity)],
        )
        .map_err(|error| jni_error("coerce Android clip to text", error))?
        .l()
        .map_err(|error| jni_error("read Android clip text", error))?;
    if text.is_null() {
        return Ok(None);
    }

    let text = env
        .call_method(&text, "toString", "()Ljava/lang/String;", &[])
        .map_err(|error| jni_error("convert Android clip text", error))?
        .l()
        .map_err(|error| jni_error("read Android clip string", error))?;
    if text.is_null() {
        return Ok(None);
    }
    let text = env
        .get_string(&JString::from(text))
        .map_err(|error| jni_error("decode Android clip string", error))?
        .to_string_lossy()
        .into_owned();
    if text.is_empty() {
        return Ok(None);
    }
    if validate_clip_text(&text).is_none() {
        log::warn!(
            "Ignoring Android clipboard selection larger than {} bytes",
            MAX_CLIPBOARD_BYTES
        );
        // An error, not `None`: `None` means "cleared" and would wipe the
        // guest clipboard; an oversized clip is just not synced.
        return Err(ClipboardError(format!(
            "clip larger than {MAX_CLIPBOARD_BYTES} bytes"
        )));
    }
    Ok(Some(text))
}

fn write_text_inner<'local>(
    env: &mut JNIEnv<'local>,
    android_app: &AndroidApp,
    text: &str,
) -> Result<(), ClipboardError> {
    let activity = activity(android_app);
    let manager = clipboard_manager(env, &activity)?;
    if manager.is_null() {
        return Err(ClipboardError(
            "Android ClipboardManager is unavailable".into(),
        ));
    }
    let label = env
        .new_string("Portal")
        .map_err(|error| jni_error("create clipboard label", error))?;
    let contents = env
        .new_string(text)
        .map_err(|error| jni_error("create clipboard contents", error))?;
    let clip = env
        .call_static_method(
            "android/content/ClipData",
            "newPlainText",
            "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
            &[(&label).into(), (&contents).into()],
        )
        .map_err(|error| jni_error("create Android plain-text clip", error))?
        .l()
        .map_err(|error| jni_error("read Android plain-text clip", error))?;
    env.call_method(
        &manager,
        "setPrimaryClip",
        "(Landroid/content/ClipData;)V",
        &[(&clip).into()],
    )
    .map_err(|error| jni_error("set Android primary clip", error))?;
    Ok(())
}

/// Read the Android primary clipboard, coercing the first item to UTF-8 text.
pub fn read_text(android_app: &AndroidApp) -> Result<Option<String>, ClipboardError> {
    run_in_jvm(|env, app| read_text_inner(env, app), android_app.clone())
}

/// Replace the Android primary clipboard with a plain-text item.
pub fn write_text(android_app: &AndroidApp, text: &str) -> Result<(), ClipboardError> {
    run_in_jvm(
        |env, app| write_text_inner(env, app, text),
        android_app.clone(),
    )
}

/// Bridge between Smithay selection callbacks and Android's clipboard.
///
/// Android clipboard reads and all Wayland selection FD I/O happen on worker threads. The
/// compositor only drains already-completed Android changes, enqueues protocol messages, and
/// hands immutable bytes to a writer. This keeps a slow Android binder call or a large paste from
/// stalling frame dispatch and presentation.
///
/// Ordering: the worker reads once at startup, whenever Android reports a
/// primary-clip change (`ClipboardWatch.kt`, delivered only while Portal is
/// focused), and on resume / focus regain, then queues external changes into
/// the shared [`ExternalClipboardSync`] (the same host-testable state machine
/// covered by `tests/clipboard_sync.rs`) and wakes the event loop immediately
/// with the dedicated [`AppUserEvent::AndroidClipboardChanged`] event. The
/// event loop drains and flushes before forwarding later keyboard/paste
/// input, so the first Ctrl+V after returning from another app sees the fresh
/// selection. Android denies reads until its focus bookkeeping catches up
/// with the window, so a denied resync read is retried a few times with a
/// short backoff; otherwise the worker sleeps until the next signal.
pub struct ClipboardBridge {
    resync: ResyncHandle,
    stop: Arc<AtomicBool>,
    broker: Option<BrokerHandle>,
}

/// Wakes the Android read worker. Shared with the clip-changed JNI callback.
#[derive(Clone)]
struct ResyncHandle {
    sync: Arc<Mutex<ExternalClipboardSync>>,
    signal: Arc<(Mutex<bool>, Condvar)>,
}

impl ResyncHandle {
    fn request(&self) {
        if let Ok(mut tracker) = self.sync.lock() {
            tracker.request_resync();
        }
        let (lock, cvar) = &*self.signal;
        if let Ok(mut requested) = lock.lock() {
            *requested = true;
            cvar.notify_one();
        }
    }
}

static ACTIVE_RESYNC: Mutex<Option<ResyncHandle>> = Mutex::new(None);

/// Delays between retries of a denied resync read.
const DENIED_READ_RETRY_MS: [u64; 4] = [100, 250, 500, 1000];

impl ClipboardBridge {
    /// Start the Android -> Wayland read worker and the guest -> Android writer.
    pub fn new(android_app: AndroidApp) -> Self {
        let sync = Arc::new(Mutex::new(ExternalClipboardSync::new()));
        let resync = ResyncHandle {
            sync: Arc::clone(&sync),
            signal: Arc::new((Mutex::new(false), Condvar::new())),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let wake_proxy = event_loop_proxy();
        let guest_writes = spawn_guest_android_writer(android_app.clone(), Arc::clone(&sync));
        let on_guest_change: GuestClipboardCallback = Arc::new(move |value| {
            if let Ok(writes) = guest_writes.lock() {
                let _ = writes.send(value);
            }
        });
        let broker = clipboard_broker::start(Arc::clone(&stop), on_guest_change);
        spawn_android_poller(
            android_app.clone(),
            sync,
            Arc::clone(&stop),
            Arc::clone(&resync.signal),
            wake_proxy,
        );
        if let Ok(mut active) = ACTIVE_RESYNC.lock() {
            *active = Some(resync.clone());
        }
        Self {
            resync,
            stop,
            broker,
        }
    }

    /// Inject the authenticated broker variables into the next PRoot Plasma
    /// launch. The guest helper uses these to reach KWin's inner server.
    pub fn broker_environment() -> Vec<(String, String)> {
        clipboard_broker::launch_environment()
    }

    /// Publish an Android clipboard observation to the inner-KWin helper.
    pub fn publish_android_clipboard(&self, value: Option<&str>) {
        let Some(broker) = self.broker.as_ref() else {
            return;
        };
        match broker.publish(value) {
            Ok(true) => log::info!("clipdiag broker published Android clipboard update"),
            Ok(false) => {}
            Err(error) => log::warn!("clipdiag broker rejected Android clipboard update: {error}"),
        }
    }

    /// Drain Android clipboard changes observed since the previous compositor iteration.
    ///
    /// Non-blocking: copies already-queued values under a short mutex without
    /// Binder or FD work, so it is safe before input on the event-loop thread.
    pub fn drain_events(&self) -> Vec<Option<String>> {
        self.resync
            .sync
            .lock()
            .map(|mut tracker| tracker.drain_pending())
            .unwrap_or_default()
    }

    /// Request an immediate resync after Activity resume / focus regain.
    ///
    /// Non-blocking with no Binder work: marks the shared tracker and wakes
    /// the worker early. The next successful read is evaluated promptly; an
    /// unchanged value queues nothing. Safe on the event-loop thread.
    pub fn request_resync(&self) {
        self.resync.request();
    }
}

/// Android's `OnPrimaryClipChangedListener` fired (`ClipboardWatch.kt`).
#[no_mangle]
pub extern "system" fn Java_app_polarbear_ClipboardWatch_nativeClipChanged(
    _env: JNIEnv,
    _class: JObject,
) {
    if let Some(resync) = ACTIVE_RESYNC.lock().ok().and_then(|active| active.clone()) {
        log::debug!("clipdiag resync reason=clip-changed");
        resync.request();
    }
}

/// One writer thread applies guest copies to Android in order. Copies that
/// arrive while a Binder write is in flight collapse to the newest one.
fn spawn_guest_android_writer(
    android_app: AndroidApp,
    sync: Arc<Mutex<ExternalClipboardSync>>,
) -> Mutex<mpsc::Sender<String>> {
    let (sender, receiver) = mpsc::channel::<String>();
    thread::spawn(move || {
        while let Ok(mut text) = receiver.recv() {
            while let Ok(newer) = receiver.try_recv() {
                text = newer;
            }
            if let Ok(mut tracker) = sync.lock() {
                tracker.mark_guest_write(Some(text.clone()));
            }
            if let Err(error) = write_text(&android_app, &text) {
                log::warn!("Failed copying guest clipboard to Android: {error}");
                if let Ok(mut tracker) = sync.lock() {
                    tracker.clear_echo_for(Some(text));
                }
            }
        }
    });
    Mutex::new(sender)
}

impl Drop for ClipboardBridge {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE_RESYNC.lock() {
            if active
                .as_ref()
                .is_some_and(|handle| Arc::ptr_eq(&handle.signal, &self.resync.signal))
            {
                *active = None;
            }
        }
        self.stop.store(true, Ordering::Release);
        // Wake the worker so it can observe `stop` immediately.
        let (lock, cvar) = &*self.resync.signal;
        if let Ok(mut requested) = lock.lock() {
            *requested = true;
            cvar.notify_one();
        }
    }
}

fn spawn_android_poller(
    android_app: AndroidApp,
    sync: Arc<Mutex<ExternalClipboardSync>>,
    stop: Arc<AtomicBool>,
    resync_signal: Arc<(Mutex<bool>, Condvar)>,
    wake_proxy: Option<EventLoopProxy<AppUserEvent>>,
) {
    thread::spawn(move || {
        let mut first_read = true;
        // Index into DENIED_READ_RETRY_MS while a denied resync read is
        // being retried; `None` sleeps until the next signal.
        let mut retry: Option<usize> = None;
        loop {
            if !first_read {
                let (lock, cvar) = &*resync_signal;
                let Ok(mut requested) = lock.lock() else {
                    break;
                };
                match retry.and_then(|attempt| DENIED_READ_RETRY_MS.get(attempt)) {
                    Some(&delay_ms) => {
                        // A new signal ends the backoff early.
                        let Ok((next, _)) = cvar.wait_timeout_while(
                            requested,
                            Duration::from_millis(delay_ms),
                            |requested| !*requested && !stop.load(Ordering::Acquire),
                        ) else {
                            return;
                        };
                        requested = next;
                    }
                    None => {
                        while !*requested && !stop.load(Ordering::Acquire) {
                            let Ok(next) = cvar.wait(requested) else {
                                return;
                            };
                            requested = next;
                        }
                    }
                }
                if *requested {
                    retry = None;
                }
                *requested = false;
            } else {
                first_read = false;
            }
            if stop.load(Ordering::Acquire) {
                break;
            }
            // Binder read happens off the event-loop/render thread.
            let observed = match read_text(&android_app) {
                Ok(current) => Ok(current),
                Err(error) => {
                    // Clipboard access is denied while Android's activity is
                    // backgrounded and briefly after it regains focus.
                    log::debug!("Android clipboard read unavailable: {error}");
                    Err(())
                }
            };
            let (queued, pending_len, retry_denied) = sync
                .lock()
                .map(|mut tracker| {
                    let queued = tracker.observe_poll(observed);
                    (queued, tracker.pending_len(), tracker.needs_resync())
                })
                .unwrap_or((false, 0, false));
            retry = if retry_denied {
                Some(retry.map_or(0, |attempt| attempt + 1))
            } else {
                None
            };
            log::debug!("clipdiag detect queued={queued} pending={pending_len}");
            if queued {
                // Wake the event loop immediately with the dedicated event so
                // the queued update is applied and flushed before later
                // keyboard/paste input. Fall back to the current global proxy
                // when the stored one predates registration.
                let proxy = wake_proxy.clone().or_else(event_loop_proxy);
                if let Some(proxy) = proxy {
                    match proxy.send_event(AppUserEvent::AndroidClipboardChanged) {
                        Ok(()) => {}
                        Err(error) => {
                            log::debug!("Android clipboard wake failed: {error}");
                            break;
                        }
                    }
                } else {
                    log::debug!("Android clipboard event loop is not ready");
                }
            }
        }
    });
}
