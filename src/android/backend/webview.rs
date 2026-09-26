//! The backend used while no desktop is running: setup, a setup failure, an unsupported device,
//! or a runtime failure. It owns no UI. Setup and its failures are shown by the Compose installer
//! (`compose_overlay`), and runtime and unsupported-device failures by the Compose recovery
//! screen (`recovery_screen`), whose buttons call native code directly over JNI.

use crate::android::proot::setup::SetupMessage;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};
use winit::platform::android::activity::AndroidApp;

/// Actions which the event loop consumes on behalf of the recovery screen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebviewAction {
    /// Clear the Plasma failure and restart the session.
    RetryPlasma,
}

/// Retry Plasma, set by the recovery screen's button and taken by the event loop. A flag rather
/// than a queue: repeated taps before the event loop runs mean one retry.
static RETRY_PLASMA_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Record a recovery action and wake the event loop to act on it.
pub fn request_action(action: WebviewAction) {
    match action {
        WebviewAction::RetryPlasma => RETRY_PLASMA_REQUESTED.store(true, Ordering::Release),
    }
    crate::android::utils::webview_handoff::wake_event_loop();
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ErrorVariant {
    None,
    /// The device, or the Android user profile Portal runs in, cannot run the guest.
    Unsupported(String),
    /// Setup failed after the installer had a safe retryable state. The Compose installer shows
    /// it with its Retry action; it is distinct from a launched-Plasma runtime failure.
    Setup(String),
    Runtime(String),
}

pub struct WebviewBackend {
    pub error: ErrorVariant,
}

impl WebviewBackend {
    /// A backend for setup in progress.
    ///
    /// Setup reports to Compose through the provisioning snapshot. Its message channel stays
    /// drained here so the worker's senders remain connected for the life of this backend.
    pub fn build(
        receiver: std::sync::mpsc::Receiver<SetupMessage>,
        _progress: Arc<Mutex<u16>>,
    ) -> Self {
        thread::spawn(move || for _ in receiver {});
        Self {
            error: ErrorVariant::None,
        }
    }

    /// A backend for an actionable runtime error screen.
    pub fn runtime_error(_android_app: AndroidApp, reason: impl Into<String>) -> Self {
        Self {
            error: ErrorVariant::Runtime(reason.into()),
        }
    }

    /// A first-run preference failure still belongs to setup, so it is shown by the installer
    /// with Retry Setup and never claims a completed desktop.
    pub fn setup_error(_android_app: AndroidApp, reason: impl Into<String>) -> Self {
        Self {
            error: ErrorVariant::Setup(reason.into()),
        }
    }

    /// A backend for a device, or Android user profile, that cannot run the guest.
    pub fn unsupported(_android_app: AndroidApp, reason: impl Into<String>) -> Self {
        Self {
            error: ErrorVariant::Unsupported(reason.into()),
        }
    }

    /// Take a pending recovery action of the requested kind.
    pub fn take_action(&self, action: WebviewAction) -> bool {
        match action {
            WebviewAction::RetryPlasma => RETRY_PLASMA_REQUESTED.swap(false, Ordering::AcqRel),
        }
    }
}
