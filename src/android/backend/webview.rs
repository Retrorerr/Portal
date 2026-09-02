//! The localhost WebSocket bridge used by the provisioning and recovery pages.
//!
//! The first version of this bridge only retained a WebSocket writer. That was enough for setup
//! progress, but it meant that the buttons in the HTML pages could never reach the native side.
//! A connection is now split into an independent reader and writer. The reader validates a
//! per-page token and puts the supported native actions in a bounded queue. Diagnostics export is
//! handled immediately because it is a self-contained user operation.

use crate::android::{diagnostics, proot::setup::SetupMessage};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::Read,
    net::TcpStream,
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            Arc, Condvar, Mutex, OnceLock,
        },
        thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use websocket::{
    sync::{Reader, Server, Writer},
    OwnedMessage,
};
use winit::platform::android::activity::AndroidApp;

/// Actions which the native application or setup worker must consume.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebviewAction {
    /// Ask the setup worker to retry the failed provisioning stage.
    RetrySetup,
    /// Ask the runtime to clear the Plasma failure marker and restart the session.
    RetryPlasma,
}

/// A small blocking queue shared by the WebSocket reader and the setup/runtime owner.
///
/// Keeping the queue separate from the WebSocket connection is important: the browser can
/// reconnect while setup is still running, and setup must not borrow a socket that may be
/// replaced by a newer page instance.
#[derive(Clone, Default)]
pub struct WebviewActionQueue {
    state: Arc<(Mutex<VecDeque<WebviewAction>>, Condvar)>,
}

impl WebviewActionQueue {
    fn push(&self, action: WebviewAction) {
        let (queue, wake) = &*self.state;
        if let Ok(mut queue) = queue.lock() {
            // A stuck or malicious page must not be able to consume unbounded native memory.
            const MAX_ACTIONS: usize = 32;
            if queue.len() >= MAX_ACTIONS {
                queue.pop_front();
            }
            queue.push_back(action);
            wake.notify_one();
        }
    }

    /// Remove all currently queued actions without blocking the event loop.
    pub fn drain(&self) -> Vec<WebviewAction> {
        let (queue, _) = &*self.state;
        queue
            .lock()
            .map(|mut queue| queue.drain(..).collect())
            .unwrap_or_default()
    }

    /// Remove the first action of the requested kind while preserving actions owned by another
    /// lifecycle phase. Setup and runtime pages share one queue, so a runtime retry must not
    /// silently consume a pending `RetrySetup` action.
    pub fn take(&self, wanted: WebviewAction) -> bool {
        let (queue, _) = &*self.state;
        let Ok(mut queue) = queue.lock() else {
            return false;
        };
        let Some(index) = queue.iter().position(|action| *action == wanted) else {
            return false;
        };
        queue.remove(index).is_some()
    }

    /// Wait for one action. Setup uses this only while it is already waiting for a failed stage;
    /// the event loop uses [`Self::drain`] so it never blocks on browser input.
    pub fn recv(&self) -> Option<WebviewAction> {
        let (queue, wake) = &*self.state;
        let mut queue = queue.lock().ok()?;
        loop {
            if let Some(action) = queue.pop_front() {
                return Some(action);
            }
            queue = wake.wait(queue).ok()?;
        }
    }
}

struct ActiveWriter {
    generation: u64,
    writer: Writer<TcpStream>,
}

struct WebviewState {
    actions: WebviewActionQueue,
    active_writer: Mutex<Option<ActiveWriter>>,
    next_generation: AtomicU64,
    closed: AtomicBool,
    android_app: Mutex<Option<AndroidApp>>,
    auth_token: String,
}

impl Default for WebviewState {
    fn default() -> Self {
        Self {
            actions: WebviewActionQueue::default(),
            active_writer: Mutex::new(None),
            next_generation: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            android_app: Mutex::new(None),
            auth_token: String::new(),
        }
    }
}

impl WebviewState {
    fn with_token() -> Self {
        let mut state = Self::default();
        state.auth_token = new_auth_token();
        state
    }
}

// Setup currently constructs the unsupported backend with a struct literal. Keep the public
// backend shape source-compatible while storing connection state out-of-line; this also makes it
// possible for a runtime error screen to outlive the setup worker cleanly.
static STATES: OnceLock<Mutex<HashMap<u16, Arc<WebviewState>>>> = OnceLock::new();

fn states() -> &'static Mutex<HashMap<u16, Arc<WebviewState>>> {
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_state(port: u16, state: Arc<WebviewState>) {
    if let Ok(mut states) = states().lock() {
        states.insert(port, state);
    }
}

fn state_for(port: u16) -> Arc<WebviewState> {
    if let Ok(mut states) = states().lock() {
        return states
            .entry(port)
            .or_insert_with(|| Arc::new(WebviewState::default()))
            .clone();
    }
    Arc::new(WebviewState::default())
}

/// Generate an opaque, per-WebView token. Android exposes `/dev/urandom` through the native
/// process; the timestamp/hash fallback is only for unusual test hosts where that device is not
/// readable and is still unique per process instance.
fn new_auth_token() -> String {
    let mut bytes = [0u8; 32];
    if File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .is_err()
    {
        let seed = format!(
            "{}:{}:{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default(),
            std::process::id(),
            NEXT_FALLBACK_TOKEN.fetch_add(1, Ordering::Relaxed)
        );
        bytes.copy_from_slice(&Sha256::digest(seed.as_bytes()));
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

static NEXT_FALLBACK_TOKEN: AtomicU64 = AtomicU64::new(1);

fn send_to_writer(writer: &mut Writer<TcpStream>, payload: OwnedMessage) -> bool {
    writer.send_message(&payload).is_ok()
}

fn send_json(state: &WebviewState, payload: Value) -> bool {
    let message = OwnedMessage::Text(payload.to_string());
    let Ok(mut active) = state.active_writer.lock() else {
        return false;
    };
    let Some(writer) = active.as_mut() else {
        return false;
    };
    if writer.writer.send_message(&message).is_ok() {
        true
    } else {
        log::info!("Setup progress client disconnected while sending a response");
        *active = None;
        false
    }
}

fn send_json_for_generation(state: &WebviewState, generation: u64, payload: Value) -> bool {
    let message = OwnedMessage::Text(payload.to_string());
    let Ok(mut active) = state.active_writer.lock() else {
        return false;
    };
    let Some(writer) = active
        .as_mut()
        .filter(|writer| writer.generation == generation)
    else {
        return false;
    };
    if send_to_writer(&mut writer.writer, message) {
        true
    } else {
        *active = None;
        false
    }
}

fn send_pong_for_generation(state: &WebviewState, generation: u64, payload: Vec<u8>) -> bool {
    let Ok(mut active) = state.active_writer.lock() else {
        return false;
    };
    let Some(writer) = active
        .as_mut()
        .filter(|writer| writer.generation == generation)
    else {
        return false;
    };
    if send_to_writer(&mut writer.writer, OwnedMessage::Pong(payload)) {
        true
    } else {
        *active = None;
        false
    }
}

fn is_active_generation(state: &WebviewState, generation: u64) -> bool {
    state
        .active_writer
        .lock()
        .map(|active| {
            active
                .as_ref()
                .is_some_and(|writer| writer.generation == generation)
        })
        .unwrap_or(false)
}

fn clear_writer(state: &WebviewState, generation: u64) {
    if let Ok(mut active) = state.active_writer.lock() {
        if active
            .as_ref()
            .is_some_and(|writer| writer.generation == generation)
        {
            *active = None;
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum ParsedAction {
    Hello,
    ExportDiagnostics,
    Queue(WebviewAction),
}

fn parse_action(text: &str, expected_token: &str) -> Result<ParsedAction, String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| format!("Invalid WebSocket action JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "WebSocket action must be a JSON object".to_string())?;
    if object.get("token").and_then(Value::as_str) != Some(expected_token) {
        return Err("WebSocket action token is missing or invalid".into());
    }
    let Some(action) = object.get("action").and_then(Value::as_str) else {
        return Err("WebSocket action is missing the action field".into());
    };
    match action {
        "hello" => Ok(ParsedAction::Hello),
        "export_diagnostics" => Ok(ParsedAction::ExportDiagnostics),
        "retry_setup" => Ok(ParsedAction::Queue(WebviewAction::RetrySetup)),
        "retry_plasma" => Ok(ParsedAction::Queue(WebviewAction::RetryPlasma)),
        other => Err(format!("Unknown WebSocket action: {other}")),
    }
}

fn handle_message(state: &WebviewState, generation: u64, text: &str) {
    match parse_action(text, &state.auth_token) {
        Ok(ParsedAction::Hello) => {
            let _ = send_json_for_generation(
                state,
                generation,
                json!({"message": "WebView is authenticated."}),
            );
        }
        Ok(ParsedAction::ExportDiagnostics) => {
            let app = state
                .android_app
                .lock()
                .ok()
                .and_then(|app| app.clone());
            let result = app
                .ok_or_else(|| "Android activity is not ready for diagnostics export".to_string())
                .and_then(|app| {
                    diagnostics::export_and_share(&app).map(|path| path.display().to_string())
                });
            match result {
                Ok(path) => {
                    let _ = send_json_for_generation(
                        state,
                        generation,
                        json!({
                            "message": format!("Diagnostics exported: {path}"),
                            "exported": true,
                        }),
                    );
                }
                Err(error) => {
                    log::warn!("Failed to export diagnostics: {error}");
                    let _ = send_json_for_generation(
                        state,
                        generation,
                        json!({
                            "message": format!("Diagnostics export failed: {error}"),
                            "isError": true,
                        }),
                    );
                }
            }
        }
        Ok(ParsedAction::Queue(action)) => {
            state.actions.push(action);
            crate::android::utils::webview_handoff::wake_event_loop();
            let message = match action {
                WebviewAction::RetrySetup => "Retry requested; setup will resume shortly.",
                WebviewAction::RetryPlasma => "Retry requested; starting the recovery session.",
            };
            let _ = send_json_for_generation(
                state,
                generation,
                json!({"message": message, "actionAccepted": true}),
            );
        }
        Err(error) => {
            let _ = send_json_for_generation(
                state,
                generation,
                json!({"message": error, "isError": true}),
            );
        }
    }
}

fn authenticate(text: &str, expected_token: &str) -> bool {
    matches!(parse_action(text, expected_token), Ok(ParsedAction::Hello))
}

fn serve_reader(mut reader: Reader<TcpStream>, state: Arc<WebviewState>, generation: u64) {
    loop {
        // Replacing an authenticated page shuts down the previous writer. This check also closes
        // a stale reader promptly if its socket happened to deliver a frame in the same instant.
        if !is_active_generation(&state, generation) {
            break;
        }
        match reader.recv_message() {
            Ok(OwnedMessage::Text(text)) => handle_message(&state, generation, &text),
            Ok(OwnedMessage::Ping(payload)) => {
                let _ = send_pong_for_generation(&state, generation, payload);
            }
            Ok(OwnedMessage::Close(_)) => break,
            Ok(_) => {}
            Err(error) => {
                log::info!("Setup progress client read failed: {error}");
                break;
            }
        }
    }
    clear_writer(&state, generation);
}

fn start_socket(
    server: Server<websocket::server::NoTlsAcceptor>,
    state: Arc<WebviewState>,
    progress: Arc<Mutex<u16>>,
) -> u16 {
    let socket_port = server
        .local_addr()
        .map(|address| address.port())
        .unwrap_or_default();
    register_state(socket_port, state.clone());

    let mut server = server;
    let _ = server.set_nonblocking(true);
    thread::spawn(move || {
        while !state.closed.load(Ordering::Acquire) {
            let request = match server.accept() {
                Ok(request) => request,
                Err(_) => {
                    // A non-blocking listener reports WouldBlock through the websocket crate's
                    // handshake error wrapper. Avoid spinning and re-check `closed` frequently
                    // so a retry/handoff can release this listener.
                    thread::sleep(Duration::from_millis(25));
                    continue;
                }
            };
            if !request.protocols().iter().any(|protocol| protocol == "rust-websocket") {
                if let Err(error) = request.reject() {
                    log::warn!("Failed to reject setup progress client: {error:?}");
                }
                continue;
            }

            let mut client = match request
                .use_protocol("rust-websocket")
                .accept_with_limits(64 * 1024, 128 * 1024)
            {
                Ok(client) => client,
                Err(error) => {
                    log::warn!("Failed to accept setup progress client: {error:?}");
                    continue;
                }
            };
            let _ = client
                .stream_ref()
                .set_read_timeout(Some(Duration::from_secs(5)));
            let (mut reader, writer) = match client.split() {
                Ok(parts) => parts,
                Err(error) => {
                    log::warn!("Failed to split setup progress client: {error}");
                    continue;
                }
            };
            // Authenticate before replacing the active writer or sending setup logs. Any local
            // app can connect to 127.0.0.1, but only the page with this opaque token can take over
            // the progress stream.
            let authenticated = match reader.recv_message() {
                Ok(OwnedMessage::Text(text)) => authenticate(&text, &state.auth_token),
                _ => false,
            };
            if !authenticated {
                log::warn!("Rejected unauthenticated setup progress client");
                continue;
            }

            let generation = state.next_generation.fetch_add(1, Ordering::AcqRel) + 1;
            if let Ok(mut active) = state.active_writer.lock() {
                if let Some(previous) = active.replace(ActiveWriter { generation, writer }) {
                    let _ = previous.writer.shutdown_all();
                    log::info!("Replaced stale setup progress client");
                }
            }

            let progress = progress.lock().map(|value| *value).unwrap_or_default();
            let _ = send_json_for_generation(
                &state,
                generation,
                json!({
                    "progress": progress,
                    "message": "Connected to installer",
                }),
            );

            let reader_state = state.clone();
            thread::spawn(move || serve_reader(reader, reader_state, generation));
        }
    });
    socket_port
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ErrorVariant {
    None,
    Unsupported,
    Runtime(String),
}

pub struct WebviewBackend {
    pub socket_port: u16,
    pub progress: Arc<Mutex<u16>>, // 0-100
    pub error: ErrorVariant,
}

impl WebviewBackend {
    /// Start accepting connections and listening for messages.
    ///
    /// This keeps the historical two-argument constructor source-compatible. The current
    /// activity is attached by `PolarBearApp::build` before the first page can export a report.
    pub fn build(
        receiver: std::sync::mpsc::Receiver<SetupMessage>,
        progress: Arc<Mutex<u16>>,
    ) -> Self {
        let socket = Server::bind("127.0.0.1:0").expect("Failed to bind socket");
        let state = Arc::new(WebviewState::with_token());
        let socket_port = start_socket(socket, state.clone(), progress.clone());
        let message_state = state_for(socket_port);
        let progress_for_messages = progress.clone();

        thread::spawn(move || {
            for message in receiver {
                let progress = progress_for_messages
                    .lock()
                    .map(|value| *value)
                    .unwrap_or_default();
                let json_message = match message {
                    SetupMessage::Progress(msg) => json!({
                        "progress": progress,
                        "message": msg,
                    }),
                    SetupMessage::Error(msg) => {
                        log::info!("Setup error [{}%]: {}", progress, msg);
                        json!({
                            "progress": progress,
                            "message": msg,
                            "isError": true
                        })
                    }
                };
                let _ = send_json(&message_state, json_message);
            }
        });

        Self {
            socket_port,
            progress,
            error: ErrorVariant::None,
        }
    }

    /// Build a WebView backend for an actionable runtime error screen.
    pub fn runtime_error(android_app: AndroidApp, reason: impl Into<String>) -> Self {
        let progress = Arc::new(Mutex::new(100));
        let (_sender, receiver) = std::sync::mpsc::channel();
        let mut backend = Self::build(receiver, progress);
        backend.error = ErrorVariant::Runtime(reason.into());
        backend.attach_android_app(android_app);
        backend
    }

    /// Build an authenticated WebView backend for a support-probe failure.
    ///
    /// Unsupported devices still need a live localhost bridge so the graphical page can export
    /// diagnostics. Keeping this constructor separate from `runtime_error` preserves the
    /// user-facing error variant while avoiding the old `socket_port: 0` placeholder.
    pub fn unsupported(android_app: AndroidApp) -> Self {
        let progress = Arc::new(Mutex::new(100));
        let (_sender, receiver) = std::sync::mpsc::channel();
        let mut backend = Self::build(receiver, progress);
        backend.error = ErrorVariant::Unsupported;
        backend.attach_android_app(android_app);
        backend
    }

    /// Attach the current activity to this page's state so export can invoke the Android
    /// Sharesheet. This is safe to call again after activity recreation.
    pub fn attach_android_app(&self, android_app: AndroidApp) {
        if let Ok(mut app) = state_for(self.socket_port).android_app.lock() {
            *app = Some(android_app);
        }
    }

    /// Return the per-instance token that HTML actions must echo.
    pub fn auth_token(&self) -> String {
        state_for(self.socket_port).auth_token.clone()
    }

    /// Return the action queue consumed by setup or the event loop.
    pub fn action_queue(&self) -> WebviewActionQueue {
        state_for(self.socket_port).actions.clone()
    }

    /// Remove all browser actions accumulated since the last event-loop turn.
    pub fn drain_actions(&self) -> Vec<WebviewAction> {
        self.action_queue().drain()
    }

    /// Take only a runtime action, leaving setup-owned actions in the shared queue.
    pub fn take_action(&self, action: WebviewAction) -> bool {
        self.action_queue().take(action)
    }

    /// Clear this backend's side-channel state after its popup has been dismissed.
    pub fn close(&self) {
        let state = state_for(self.socket_port);
        state.closed.store(true, Ordering::Release);
        if let Ok(mut active) = state.active_writer.lock() {
            if let Some(writer) = active.take() {
                let _ = writer.writer.shutdown_all();
            }
        }
        if let Ok(mut states) = states().lock() {
            states.remove(&self.socket_port);
        }
    }
}

impl Drop for WebviewBackend {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_are_bounded_and_drained_in_order() {
        let queue = WebviewActionQueue::default();
        for _ in 0..33 {
            queue.push(WebviewAction::RetrySetup);
        }
        queue.push(WebviewAction::RetryPlasma);
        let actions = queue.drain();
        assert_eq!(actions.len(), 32);
        assert_eq!(actions.last(), Some(&WebviewAction::RetryPlasma));
    }

    #[test]
    fn parser_accepts_only_supported_actions_and_tokens() {
        let token = "0123456789abcdef";
        assert_eq!(
            parse_action(
                r#"{"action":"hello","token":"0123456789abcdef"}"#,
                token
            )
            .unwrap(),
            ParsedAction::Hello
        );
        assert_eq!(
            parse_action(
                r#"{"action":"retry_setup","token":"0123456789abcdef"}"#,
                token
            )
            .unwrap(),
            ParsedAction::Queue(WebviewAction::RetrySetup)
        );
        assert!(parse_action(r#"{"action":"retry_setup"}"#, token).is_err());
        assert!(parse_action(
            r#"{"action":"run_shell","token":"0123456789abcdef"}"#,
            token
        )
        .is_err());
        assert!(parse_action("not-json", token).is_err());
    }
}
