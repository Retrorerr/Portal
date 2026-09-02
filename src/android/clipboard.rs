//! Android clipboard access for the nested Wayland session.
//!
//! The compositor owns the Wayland selection, while Android owns the system clipboard.  This
//! module deliberately contains only the Android/JNI side and a small change detector; the
//! compositor can call it from its selection callbacks without duplicating fragile JNI code.

use jni::{
    objects::{JObject, JString, JValue},
    JNIEnv,
};
use smithay::wayland::selection::SelectionSource;
use std::{
    fmt,
    io::{Read, Write},
    os::fd::OwnedFd,
    os::unix::net::UnixStream,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};
use winit::platform::android::activity::AndroidApp;

use crate::android::utils::ndk::run_in_jvm;

/// MIME types understood by Android's plain-text `ClipData` representation.
pub const TEXT_MIME: &str = "text/plain";
pub const UTF8_TEXT_MIME: &str = "text/plain;charset=utf-8";

/// Maximum clipboard payload accepted from a Wayland client. This protects the background
/// transfer worker from an accidentally unbounded `wl_data_offer.receive` stream while keeping
/// normal copied documents and shell commands usable.
const MAX_CLIPBOARD_BYTES: usize = 4 * 1024 * 1024;

/// Data backing a compositor-provided Android clipboard selection.
///
/// The bytes are immutable and reference counted so `send_selection` can hand the transfer to a
/// worker without borrowing the Smithay state or blocking its event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardSelectionData {
    bytes: Arc<[u8]>,
}

impl ClipboardSelectionData {
    pub fn from_text(text: String) -> Self {
        Self {
            bytes: Arc::from(text.into_bytes()),
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Events produced by the Android clipboard polling worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardEvent {
    AndroidChanged(Option<String>),
}

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
    if clip.is_null() {
        return Ok(None);
    }

    let item_count = env
        .call_method(&clip, "getItemCount", "()I", &[])
        .map_err(|error| jni_error("get Android clip item count", error))?
        .i()
        .map_err(|error| jni_error("read Android clip item count", error))?;
    if item_count <= 0 {
        return Ok(None);
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
        return Ok(None);
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
        .new_string("Local Desktop")
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
/// compositor only enqueues protocol messages, drains already-completed Android changes, and
/// hands immutable bytes to a writer. This keeps a slow Android binder call or a large paste from
/// stalling frame dispatch and presentation.
pub struct ClipboardBridge {
    android_app: AndroidApp,
    events: Receiver<ClipboardEvent>,
    stop: Arc<AtomicBool>,
    last_guest_write: Arc<Mutex<Option<String>>>,
}

impl ClipboardBridge {
    /// Start the Android -> Wayland polling worker.
    pub fn new(android_app: AndroidApp) -> Self {
        let (event_sender, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let last_guest_write = Arc::new(Mutex::new(None));
        spawn_android_poller(
            android_app.clone(),
            event_sender,
            Arc::clone(&stop),
            Arc::clone(&last_guest_write),
        );
        Self {
            android_app,
            events,
            stop,
            last_guest_write,
        }
    }

    /// Drain Android clipboard changes observed since the previous compositor iteration.
    pub fn drain_events(&self) -> Vec<ClipboardEvent> {
        let mut changes = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(event) => changes.push(event),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        changes
    }

    /// Request that a client-owned Wayland selection be copied into Android.
    ///
    /// The source's `send` request is a small Smithay protocol enqueue. Reading the resulting FD,
    /// decoding UTF-8, and calling Android's clipboard manager all happen off the compositor
    /// thread.
    pub fn forward_guest_selection(
        &self,
        source: SelectionSource,
        mime_type: String,
    ) -> Result<(), String> {
        if !supports_mime_type(&mime_type) {
            return Err(format!("unsupported clipboard MIME type: {mime_type}"));
        }

        let (reader, writer) = UnixStream::pair()
            .map_err(|error| format!("create clipboard transfer pipe: {error}"))?;
        let writer: OwnedFd = writer.into();
        // Smithay's source owns and closes this FD after sending wl_data_source.send. The reader
        // therefore observes EOF once the client finishes the transfer.
        source.send(mime_type, writer);

        let android_app = self.android_app.clone();
        let last_guest_write = Arc::clone(&self.last_guest_write);
        thread::spawn(move || {
            let mut reader = reader.take((MAX_CLIPBOARD_BYTES + 1) as u64);
            let mut bytes = Vec::new();
            if let Err(error) = reader.read_to_end(&mut bytes) {
                log::warn!("Failed reading guest clipboard selection: {error}");
                return;
            }
            if bytes.len() > MAX_CLIPBOARD_BYTES {
                log::warn!(
                    "Ignoring guest clipboard selection larger than {} bytes",
                    MAX_CLIPBOARD_BYTES
                );
                return;
            }
            let Ok(text) = String::from_utf8(bytes) else {
                log::warn!("Ignoring guest clipboard selection that is not UTF-8");
                return;
            };

            // Mark before the binder write so the poller cannot echo this own write if it races
            // the Android clipboard notification. Clear the marker when the write fails.
            if let Ok(mut previous) = last_guest_write.lock() {
                *previous = Some(text.clone());
            }
            if let Err(error) = write_text(&android_app, &text) {
                log::warn!("Failed copying guest clipboard to Android: {error}");
                if let Ok(mut previous) = last_guest_write.lock() {
                    if previous.as_deref() == Some(text.as_str()) {
                        *previous = None;
                    }
                }
            } else {
                log::debug!(
                    "Copied {} bytes from guest clipboard to Android",
                    text.len()
                );
            }
        });
        Ok(())
    }

    /// Propagate a client clearing its Wayland clipboard to Android.
    pub fn clear_guest_selection(&self) {
        let android_app = self.android_app.clone();
        let last_guest_write = Arc::clone(&self.last_guest_write);
        thread::spawn(move || {
            if let Ok(mut previous) = last_guest_write.lock() {
                *previous = Some(String::new());
            }
            if let Err(error) = write_text(&android_app, "") {
                log::warn!("Failed clearing Android clipboard after guest clear: {error}");
                if let Ok(mut previous) = last_guest_write.lock() {
                    *previous = None;
                }
            }
        });
    }

    /// Send Android clipboard bytes to a Wayland client without blocking dispatch.
    pub fn send_selection(&self, selection: &ClipboardSelectionData, mime_type: &str, fd: OwnedFd) {
        if !supports_mime_type(mime_type) {
            log::debug!("Dropping unsupported Android clipboard MIME request: {mime_type}");
            return;
        }
        let bytes = Arc::clone(&selection.bytes);
        thread::spawn(move || {
            let mut file = std::fs::File::from(fd);
            if let Err(error) = file.write_all(&bytes) {
                log::debug!("Wayland clipboard receiver closed early: {error}");
            }
            if let Err(error) = file.flush() {
                log::debug!("Failed flushing Wayland clipboard transfer: {error}");
            }
        });
    }
}

impl Drop for ClipboardBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn spawn_android_poller(
    android_app: AndroidApp,
    event_sender: Sender<ClipboardEvent>,
    stop: Arc<AtomicBool>,
    last_guest_write: Arc<Mutex<Option<String>>>,
) {
    thread::spawn(move || {
        let mut last_seen: Option<Option<String>> = None;
        while !stop.load(Ordering::Acquire) {
            match read_text(&android_app) {
                Ok(current) => {
                    if last_seen.as_ref() != Some(&current) {
                        last_seen = Some(current.clone());
                        let suppress = last_guest_write.lock().ok().and_then(|value| value.clone());
                        if suppress == current {
                            // Consume the one matching observation; an external later change is
                            // still delivered even if it happens to reuse the same text after a
                            // different clipboard value.
                            if let Ok(mut value) = last_guest_write.lock() {
                                if *value == current {
                                    *value = None;
                                }
                            }
                        } else if event_sender
                            .send(ClipboardEvent::AndroidChanged(current))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
                Err(error) => {
                    // Clipboard access can be denied while Android's activity is backgrounded;
                    // retry after the normal interval without manufacturing a clear selection.
                    log::debug!("Android clipboard poll unavailable: {error}");
                }
            }
            thread::sleep(Duration::from_millis(250));
        }
    });
}

/// Return whether a MIME type can be represented by the Android text bridge.
pub fn supports_mime_type(mime_type: &str) -> bool {
    mime_type.eq_ignore_ascii_case(TEXT_MIME) || mime_type.eq_ignore_ascii_case(UTF8_TEXT_MIME)
}

/// Pick the strongest supported text MIME type offered by a Wayland client.
pub fn choose_text_mime<'a, I>(mime_types: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut plain = None;
    for mime_type in mime_types {
        if mime_type.eq_ignore_ascii_case(UTF8_TEXT_MIME) {
            return Some(mime_type);
        }
        if plain.is_none() && mime_type.eq_ignore_ascii_case(TEXT_MIME) {
            plain = Some(mime_type);
        }
    }
    plain
}

/// Small state machine for polling Android -> Wayland clipboard changes.
#[derive(Debug, Default)]
pub struct ClipboardPoller {
    last: Mutex<Option<Option<String>>>,
}

impl ClipboardPoller {
    /// Read once and return `Some(new_value)` only when the Android clipboard changed. The inner
    /// `None` represents a cleared or unavailable clipboard and is distinct from no change.
    pub fn poll(&self, android_app: &AndroidApp) -> Result<Option<Option<String>>, ClipboardError> {
        let current = read_text(android_app)?;
        let mut last = self
            .last
            .lock()
            .map_err(|_| ClipboardError("clipboard poller lock poisoned".into()))?;
        if last.as_ref() == Some(&current) {
            return Ok(None);
        }
        *last = Some(current.clone());
        Ok(Some(current))
    }

    pub fn reset(&self) {
        if let Ok(mut last) = self.last.lock() {
            *last = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_plain_text_mime_types() {
        assert!(supports_mime_type("text/plain"));
        assert!(supports_mime_type("TEXT/PLAIN;CHARSET=UTF-8"));
        assert!(!supports_mime_type("text/html"));
    }

    #[test]
    fn prefers_utf8_text_when_offered() {
        assert_eq!(
            choose_text_mime(["text/html", UTF8_TEXT_MIME]),
            Some(UTF8_TEXT_MIME)
        );
        assert_eq!(choose_text_mime(["text/html"]), None);
    }
}
