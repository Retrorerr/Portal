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
    /// Runtime recovery actions wait for the old WebView Looper to exit before
    /// the Wayland surface is rebound, preventing a stale popup from covering
    /// the resumed desktop.
    pub pending_runtime_retry: bool,
}

pub struct PolarBearFrontend {
    pub android_app: AndroidApp,
}

pub enum PolarBearBackend {
    /// Use a webview to report setup progress to the user
    /// The setup progress should only be done once, when the user first installed the app
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
        let mut backend = setup_with_completion(android_app.clone(), Some(completion));
        // The support probe may return the historical socket-less placeholder. Replace it with
        // a real authenticated backend before the first `resumed` callback so even unsupported
        // devices can use the graphical page's diagnostics export action.
        if matches!(
            &backend,
            PolarBearBackend::WebView(webview)
                if webview.socket_port == 0 && webview.error == crate::android::backend::webview::ErrorVariant::Unsupported
        ) {
            backend = PolarBearBackend::WebView(WebviewBackend::unsupported(android_app.clone()));
        }
        if let PolarBearBackend::WebView(webview) = &mut backend {
            // The setup worker predates the action-capable WebView constructor. Attach the
            // activity here so export_diagnostics can invoke the Android Sharesheet for both
            // supported and unsupported startup paths.
            webview.attach_android_app(android_app.clone());
        }
        Self {
            backend,
            frontend: PolarBearFrontend { android_app },
            pending_runtime_retry: false,
        }
    }
}
