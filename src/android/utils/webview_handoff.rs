//! In-process handoffs from background workers to the winit event loop.
//!
//! The NativeActivity is never recreated when setup finishes. The setup worker sets a handoff
//! flag and wakes the event loop through its proxy; the event loop then swaps to the Wayland
//! backend in the same activity.

use crate::android::accessibility::AppUserEvent;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};
use winit::{event_loop::EventLoopProxy, platform::android::activity::AndroidApp};

static EVENT_LOOP_PROXY: OnceLock<Mutex<Option<EventLoopProxy<AppUserEvent>>>> = OnceLock::new();
/// Transient event-loop wake only. The durable runtime marker is the sole
/// installation truth; this bit is never consulted as proof that setup
/// succeeded.
static SETUP_HANDOFF_PENDING: AtomicBool = AtomicBool::new(false);
/// A provisional first-run Plasma launch must not borrow `SETUP_HANDOFF_PENDING`:
/// that flag means the durable completion marker already exists. Keeping this
/// edge distinct prevents a pending KScreen/appearance proof from triggering the
/// ordinary completed-install handoff.
static INITIAL_PREFERENCES_HANDOFF_PENDING: AtomicBool = AtomicBool::new(false);

fn event_loop_proxy() -> &'static Mutex<Option<EventLoopProxy<AppUserEvent>>> {
    EVENT_LOOP_PROXY.get_or_init(|| Mutex::new(None))
}

pub fn register_event_loop_proxy(proxy: EventLoopProxy<AppUserEvent>) {
    if let Ok(mut current) = event_loop_proxy().lock() {
        *current = Some(proxy);
    }
}

pub fn wake_event_loop() {
    let proxy = event_loop_proxy()
        .lock()
        .ok()
        .and_then(|proxy| proxy.clone());
    if let Some(proxy) = proxy {
        if let Err(error) = proxy.send_event(AppUserEvent::AccessibilityInputReady) {
            log::debug!("Failed to wake event loop for a handoff: {error}");
        }
    }
}

pub fn complete_setup(_android_app: AndroidApp) {
    SETUP_HANDOFF_PENDING.store(true, Ordering::Release);
    wake_event_loop();
}

pub fn take_setup_handoff() -> bool {
    SETUP_HANDOFF_PENDING.swap(false, Ordering::AcqRel)
}

pub fn request_initial_preferences_handoff(_android_app: AndroidApp) {
    INITIAL_PREFERENCES_HANDOFF_PENDING.store(true, Ordering::Release);
    wake_event_loop();
}

pub fn take_initial_preferences_handoff() -> bool {
    INITIAL_PREFERENCES_HANDOFF_PENDING.swap(false, Ordering::AcqRel)
}

pub fn requeue_initial_preferences_handoff() {
    INITIAL_PREFERENCES_HANDOFF_PENDING.store(true, Ordering::Release);
}

pub fn cancel_initial_preferences_handoff() {
    INITIAL_PREFERENCES_HANDOFF_PENDING.store(false, Ordering::Release);
}
