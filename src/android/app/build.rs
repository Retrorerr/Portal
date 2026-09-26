use winit::platform::android::activity::AndroidApp;

use crate::android::{
    backend::{wayland::WaylandBackend, webview::WebviewBackend},
    proot::setup::{setup_with_completion, SetupCompletionCallback},
    utils::webview_handoff,
};
use std::sync::Arc;

pub struct PolarBearApp {
    pub frontend: PolarBearFrontend,
    pub backend: PolarBearBackend,
    /// Retry Plasma waits for the failed guest session to stop before the
    /// Wayland backend is rebuilt.
    pub pending_runtime_retry: bool,
    /// A committed install may fail while binding/resuming Wayland. The Compose
    /// veil is dismissed before the recovery screen shows, so this state never
    /// looks like unfinished installation.
    pub pending_runtime_error_page: bool,
}

pub struct PolarBearFrontend {
    pub android_app: AndroidApp,
}

pub enum PolarBearBackend {
    /// No desktop is running: setup, a setup failure, an unsupported device or
    /// a runtime failure. Compose screens report these to the user.
    WebView(WebviewBackend),

    /// Use a wayland compositor to render Linux GUI applications back to the Android Native Activity
    Wayland(WaylandBackend),
}

impl PolarBearApp {
    pub fn build(android_app: AndroidApp) -> Self {
        let completion_app = android_app.clone();
        let completion: SetupCompletionCallback = Arc::new(move || {
            webview_handoff::complete_setup(completion_app.clone());
        });
        let backend = setup_with_completion(android_app.clone(), Some(completion));
        Self {
            backend,
            frontend: PolarBearFrontend { android_app },
            pending_runtime_retry: false,
            pending_runtime_error_page: false,
        }
    }
}
