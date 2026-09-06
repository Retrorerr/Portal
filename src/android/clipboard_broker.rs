//! Android-side loopback broker for the clipboard owned by nested KWin.
//!
//! Portal is the outer Wayland compositor and KWin is its client. KWin then
//! creates the inner Wayland server used by Plasma applications. The outer
//! `wl_data_device` selection therefore cannot become the inner KWin
//! clipboard. This small broker carries text between Android and a guest
//! helper which talks to KWin's inner data-control protocol.

use crate::core::clipboard_broker::{
    self as protocol, AuthToken, BrokerEvent, BrokerState, ClientRequest, CodecError, RequestKind,
};
use std::io::{BufReader, ErrorKind, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc::{self, Sender},
    Arc, Mutex, OnceLock,
};
use std::thread;

pub type GuestClipboardCallback = Arc<dyn Fn(Option<String>) + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokerEnvironment {
    pub port: u16,
    pub token: String,
}

type Subscribers = Arc<Mutex<Vec<(usize, Sender<BrokerEvent>)>>>;

/// Handle retained by the Android clipboard bridge for the lifetime of the
/// compositor. The listener itself is stopped by the bridge's shared stop
/// flag; retaining this handle also keeps the launch environment registered.
pub struct BrokerHandle {
    environment: BrokerEnvironment,
    state: Arc<Mutex<BrokerState>>,
    subscribers: Subscribers,
}

static ACTIVE_ENVIRONMENT: OnceLock<Mutex<Option<BrokerEnvironment>>> = OnceLock::new();

fn active_environment() -> &'static Mutex<Option<BrokerEnvironment>> {
    ACTIVE_ENVIRONMENT.get_or_init(|| Mutex::new(None))
}

/// Return the broker variables to inject into the PRoot Plasma process.
/// Clipboard contents are never part of this environment; the token is only
/// the per-session authorization secret.
pub fn launch_environment() -> Vec<(String, String)> {
    let Some(environment) = active_environment()
        .lock()
        .ok()
        .and_then(|environment| environment.clone())
    else {
        return Vec::new();
    };
    vec![
        (
            "LOCALDESKTOP_CLIPBOARD_HOST".to_owned(),
            "127.0.0.1".to_owned(),
        ),
        (
            "LOCALDESKTOP_CLIPBOARD_PORT".to_owned(),
            environment.port.to_string(),
        ),
        ("LOCALDESKTOP_CLIPBOARD_TOKEN".to_owned(), environment.token),
    ]
}

/// Start a loopback listener with a fresh per-session token.
pub fn start(
    stop: Arc<AtomicBool>,
    on_guest_change: GuestClipboardCallback,
) -> Option<BrokerHandle> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| {
            log::warn!("clipdiag broker bind failed: {error}");
            error
        })
        .ok()?;
    listener
        .set_nonblocking(true)
        .map_err(|error| {
            log::warn!("clipdiag broker nonblocking setup failed: {error}");
            error
        })
        .ok()?;
    let port = listener.local_addr().ok()?.port();
    let token = random_token()?;
    let environment = BrokerEnvironment {
        port,
        token: token.to_hex(),
    };
    if let Ok(mut active) = active_environment().lock() {
        *active = Some(environment.clone());
    } else {
        log::warn!("clipdiag broker environment lock is poisoned");
        return None;
    }

    let state = Arc::new(Mutex::new(BrokerState::default()));
    let subscribers = Arc::new(Mutex::new(Vec::new()));
    let next_subscriber = Arc::new(AtomicUsize::new(1));
    log::info!("clipdiag broker listening port={port}");

    let expected_token = token;
    let state_for_listener = Arc::clone(&state);
    let subscribers_for_listener = Arc::clone(&subscribers);
    let next_subscriber_for_listener = Arc::clone(&next_subscriber);
    thread::spawn(move || {
        accept_loop(
            listener,
            stop,
            expected_token,
            state_for_listener,
            subscribers_for_listener,
            next_subscriber_for_listener,
            on_guest_change,
        )
    });

    Some(BrokerHandle {
        environment,
        state,
        subscribers,
    })
}

fn random_token() -> Option<AuthToken> {
    let mut bytes = [0u8; protocol::TOKEN_BYTES];
    let mut source = std::fs::File::open("/dev/urandom")
        .map_err(|error| {
            log::warn!("clipdiag broker random source unavailable: {error}");
            error
        })
        .ok()?;
    std::io::Read::read_exact(&mut source, &mut bytes)
        .map_err(|error| {
            log::warn!("clipdiag broker random token read failed: {error}");
            error
        })
        .ok()?;
    Some(AuthToken::from_bytes(bytes))
}

impl BrokerHandle {
    /// Publish a successful Android clipboard observation to the subscribed
    /// inner-KWin helper.
    pub fn publish(&self, value: Option<&str>) -> Result<bool, CodecError> {
        let event = match value {
            Some(text) => {
                // Validate the same bounded UTF-8/base64 representation that
                // will be sent on the wire before mutating broker state.
                protocol::encode_value_event(text)?;
                BrokerEvent::Value(text.to_owned())
            }
            None => BrokerEvent::Clear,
        };
        let Ok(mut state) = self.state.lock() else {
            return Ok(false);
        };
        let changed = state.apply(value)?;
        if !changed {
            return Ok(false);
        }

        // Lock state before subscribers, matching the subscription snapshot
        // path so an update cannot overtake a newly subscribed client's first
        // snapshot.
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.retain(|(_, subscriber)| subscriber.send(event.clone()).is_ok());
        }
        Ok(true)
    }

    pub fn environment(&self) -> &BrokerEnvironment {
        &self.environment
    }
}

impl Drop for BrokerHandle {
    fn drop(&mut self) {
        if let Ok(mut active) = active_environment().lock() {
            if active.as_ref() == Some(&self.environment) {
                *active = None;
            }
        }
    }
}

fn accept_loop(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    expected_token: AuthToken,
    state: Arc<Mutex<BrokerState>>,
    subscribers: Subscribers,
    next_subscriber: Arc<AtomicUsize>,
    on_guest_change: GuestClipboardCallback,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _address)) => {
                let _ = stream.set_nodelay(true);
                let state = Arc::clone(&state);
                let subscribers = Arc::clone(&subscribers);
                let next_subscriber = Arc::clone(&next_subscriber);
                let on_guest_change = Arc::clone(&on_guest_change);
                let expected_token = expected_token.clone();
                thread::spawn(move || {
                    handle_client(
                        stream,
                        expected_token,
                        state,
                        subscribers,
                        next_subscriber,
                        on_guest_change,
                    )
                });
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::park_timeout(std::time::Duration::from_millis(25));
            }
            Err(error) => {
                log::warn!("clipdiag broker accept failed: {error}");
                break;
            }
        }
    }
}

fn handle_client(
    stream: TcpStream,
    expected_token: AuthToken,
    state: Arc<Mutex<BrokerState>>,
    subscribers: Subscribers,
    next_subscriber: Arc<AtomicUsize>,
    on_guest_change: GuestClipboardCallback,
) {
    let reader_stream = match stream.try_clone() {
        Ok(stream) => stream,
        Err(error) => {
            log::debug!("clipdiag broker client clone failed: {error}");
            return;
        }
    };
    let mut reader = BufReader::new(reader_stream);
    let mut writer = stream;

    let authenticated = match protocol::read_request(&mut reader) {
        Ok(ClientRequest::Hello(token)) if expected_token.constant_time_eq_token(&token) => {
            send_event(&mut writer, BrokerEvent::Ack(RequestKind::Hello)).is_ok()
        }
        Ok(ClientRequest::Hello(_)) => {
            log::warn!("clipdiag broker rejected client with invalid token");
            false
        }
        Ok(_) | Err(_) => false,
    };
    if !authenticated {
        return;
    }
    match protocol::read_request(&mut reader) {
        Ok(ClientRequest::Subscribe) => {}
        Ok(request @ (ClientRequest::Push(_) | ClientRequest::Clear)) => {
            // wl-paste invokes a one-shot publisher: HELLO then PUSH/CLEAR.
            // It must not subscribe (and receive the host snapshot) to write.
            if let Some(kind) = apply_guest_request(request, &state, &on_guest_change) {
                let _ = send_event(&mut writer, BrokerEvent::Ack(kind));
            }
            return;
        }
        _ => return,
    }

    let (out_tx, out_rx) = mpsc::channel();
    let writer_thread = thread::spawn(move || {
        let mut writer = writer;
        while let Ok(event) = out_rx.recv() {
            if send_event(&mut writer, event).is_err() {
                break;
            }
        }
    });

    let subscriber_id = next_subscriber.fetch_add(1, Ordering::Relaxed);
    // Lock state before subscribers. `publish()` uses the same order, which
    // makes the initial snapshot and a concurrent Android update ordered.
    {
        let Ok(state) = state.lock() else {
            return;
        };
        let current = state.value().map(str::to_owned);
        if let Ok(mut subscribers) = subscribers.lock() {
            subscribers.push((subscriber_id, out_tx.clone()));
        } else {
            return;
        }
        // Queue the snapshot while holding state, before publish can queue
        // a newer value for this subscriber.
        let _ = out_tx.send(BrokerEvent::Ack(RequestKind::Subscribe));
        let _ = out_tx.send(match current {
            Some(value) => BrokerEvent::Value(value),
            None => BrokerEvent::Clear,
        });
    }

    loop {
        let request = match protocol::read_request(&mut reader) {
            Ok(request) => request,
            Err(_) => break,
        };
        let Some(kind) = apply_guest_request(request, &state, &on_guest_change) else {
            break;
        };
        if out_tx.send(BrokerEvent::Ack(kind)).is_err() {
            break;
        }
    }

    remove_subscriber(&subscribers, subscriber_id);
    drop(out_tx);
    let _ = writer_thread.join();
}

fn apply_guest_request(
    request: ClientRequest,
    state: &Mutex<BrokerState>,
    on_guest_change: &GuestClipboardCallback,
) -> Option<RequestKind> {
    let (value, kind) = match request {
        ClientRequest::Push(text) => (Some(text), RequestKind::Push),
        ClientRequest::Clear => (None, RequestKind::Clear),
        _ => return None,
    };
    let changed = state.lock().ok()?.apply(value.as_deref()).ok()?;
    if changed {
        on_guest_change(value);
    }
    Some(kind)
}

fn remove_subscriber(subscribers: &Subscribers, subscriber_id: usize) {
    if let Ok(mut subscribers) = subscribers.lock() {
        subscribers.retain(|(id, _)| *id != subscriber_id);
    }
}

fn send_event(writer: &mut impl Write, event: BrokerEvent) -> std::io::Result<()> {
    let frame = match event {
        BrokerEvent::Value(value) => protocol::encode_value_event(&value)
            .map_err(|error| std::io::Error::new(ErrorKind::InvalidData, error.to_string()))?,
        BrokerEvent::Clear => protocol::encode_clear_event().to_vec(),
        BrokerEvent::Ack(kind) => protocol::encode_ack(kind).to_vec(),
    };
    protocol::write_all(writer, &frame)?;
    writer.flush()
}
