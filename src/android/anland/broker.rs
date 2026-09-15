//! In-process Anland broker (absorbed daemon role).
//!
//! The reference design runs a separate `daemon` binary that brokers the
//! handshake between consumer and producer. Portal absorbs both the consumer
//! *and* the broker: this listener implements exactly the daemon's wire
//! behavior (first `screen_info` wins, `PICKUP_FDS` → `FDS_READY` + 5 fds)
//! without an extra process hop on the frame path. The daemon is off the hot
//! path by construction: after the handshake every frame travels over the
//! shared shm page, the buf_ready eventfd and the fence socketpair.
//!
//! Reference: `SuperTurtleDev/anland` `daemon/`, protocol §8.

use std::io;
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::protocol::*;
use super::sys;

/// Producer-end fds deposited for one generation. The broker keeps the
/// masters; every served `PICKUP_FDS` gets fresh `dup()`s (SCM_RIGHTS dups
/// again into the producer), so retries never reuse a closed number.
pub struct Deposit {
    pub generation: u64,
    /// Masters in hello slot order: { buf_ready, fence_write, data_peer,
    /// shm, audio_peer }. Shared fds (buf_ready, shm) are the consumer's own
    /// masters; socket ends are the producer's peers.
    pub fds: [OwnedFd; HELLO_FD_COUNT],
}

struct Slot {
    screen: ScreenInfo,
    deposit: Option<Deposit>,
    /// Fired (with the generation id) each time a deposit is handed over, so
    /// the consumer can follow up with `BUFS_READY` on the data channel.
    attach_notify: Vec<std::sync::mpsc::Sender<u64>>,
}

pub struct Broker {
    slot: Arc<Mutex<Slot>>,
    socket_path: PathBuf,
}

impl Broker {
    pub fn new(screen: ScreenInfo, socket_path: PathBuf) -> Self {
        Self {
            slot: Arc::new(Mutex::new(Slot {
                screen,
                deposit: None,
                attach_notify: Vec::new(),
            })),
            socket_path,
        }
    }

    pub fn set_screen(&self, screen: ScreenInfo) {
        if let Ok(mut slot) = self.slot.lock() {
            slot.screen = screen;
        }
    }

    /// Screen size currently served to new producer hellos. Updated on
    /// every rotation rebind (previously write-only dead code): any fresh
    /// producer connection observes the current geometry.
    pub fn screen(&self) -> ScreenInfo {
        self.slot.lock().map(|s| s.screen).unwrap_or(ScreenInfo {
            width: 0,
            height: 0,
            format: 1,
            refresh: 0,
        })
    }

    /// Install a fresh deposit for `generation`, replacing any previous one.
    pub fn deposit(&self, deposit: Deposit) {
        if let Ok(mut slot) = self.slot.lock() {
            slot.deposit = Some(deposit);
        }
    }

    /// Withdraw the deposit (fallback teardown). Producer retries will find
    /// nothing until the next deposit, exactly like the reference daemon
    /// after the consumer re-deposits.
    pub fn withdraw(&self) {
        if let Ok(mut slot) = self.slot.lock() {
            slot.deposit = None;
        }
    }

    /// Subscribe to producer-attach notifications (one per served pickup).
    pub fn subscribe_attach(&self) -> std::sync::mpsc::Receiver<u64> {
        let (tx, rx) = std::sync::mpsc::channel();
        if let Ok(mut slot) = self.slot.lock() {
            slot.attach_notify.push(tx);
        }
        rx
    }

    /// Serve the handshake until `shutdown` fires. Runs on its own thread.
    pub fn serve(self: Arc<Self>, shutdown: Arc<std::sync::atomic::AtomicBool>) -> io::Result<()> {
        let _ = std::fs::remove_file(&self.socket_path);
        let listener = UnixListener::bind(&self.socket_path)?;
        listener.set_nonblocking(true)?;
        let (sw0, sh0, sfmt0, sref0) = {
            let slot = self
                .slot
                .lock()
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "broker slot poisoned"))?;
            (
                slot.screen.width,
                slot.screen.height,
                slot.screen.format,
                slot.screen.refresh,
            )
        };
        log::info!(
            "anland.broker=listening socket={} screen={sw0}x{sh0} fmt={sfmt0} refresh_mhz={sref0}",
            self.socket_path.display(),
        );
        while !shutdown.load(std::sync::atomic::Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let broker = self.clone();
                    std::thread::spawn(move || broker.serve_producer(stream));
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => {
                    log::warn!("anland.broker accept error: {e}");
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            }
        }
        let _ = std::fs::remove_file(&self.socket_path);
        Ok(())
    }

    fn serve_producer(&self, stream: std::os::unix::net::UnixStream) {
        use std::os::unix::io::AsRawFd;
        let raw = stream.as_raw_fd();
        // Wrap without owning: the stream owns the fd; guard drops nothing.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let result = self.handshake_loop(&fd);
        std::mem::forget(fd);
        if let Err(e) = result {
            log::info!("anland.broker producer session ended: {e}");
        }
    }

    fn handshake_loop(&self, fd: &OwnedFd) -> io::Result<()> {
        // Protocol v3 makes the version/features payload part of the control
        // handshake. Rejecting here prevents an incompatible producer from
        // ever receiving the consumer dma-buf fds.
        let mut hdr = [0u8; 8];
        sys::recv_all(fd, &mut hdr)?;
        let msg_type = u32::from_ne_bytes(hdr[0..4].try_into().unwrap());
        let size = u32::from_ne_bytes(hdr[4..8].try_into().unwrap()) as usize;
        let expected_hello_size = std::mem::size_of::<ProtocolHello>();
        if msg_type != CTRL_MSG_PRODUCER_HELLO || size != expected_hello_size {
            log::warn!(
                "anland.broker unexpected first message type={msg_type} size={size}; expected type={} size={expected_hello_size}",
                CTRL_MSG_PRODUCER_HELLO
            );
            Self::send_protocol_reject(fd)?;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected PRODUCER_HELLO",
            ));
        }
        let mut hello_bytes = [0u8; 8];
        sys::recv_all(fd, &mut hello_bytes)?;
        let hello = ProtocolHello {
            version: u32::from_ne_bytes(hello_bytes[0..4].try_into().unwrap()),
            features: u32::from_ne_bytes(hello_bytes[4..8].try_into().unwrap()),
        };
        let hello_version = hello.version;
        let hello_features = hello.features;
        if hello_version != PROTOCOL_VERSION
            || (hello_features & PROTOCOL_REQUIRED_FEATURES) != PROTOCOL_REQUIRED_FEATURES
        {
            log::warn!(
                "anland.broker rejecting protocol version={} features=0x{:x}; required version={} features=0x{:x}",
                hello_version,
                hello_features,
                PROTOCOL_VERSION,
                PROTOCOL_REQUIRED_FEATURES
            );
            Self::send_protocol_reject(fd)?;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "incompatible Anland protocol",
            ));
        }
        let screen = self.slot.lock().map(|s| s.screen).unwrap_or(ScreenInfo {
            width: 0,
            height: 0,
            format: 1,
            refresh: 0,
        });
        let mut out = [0u8; 8 + 16];
        out[0..4].copy_from_slice(&CTRL_MSG_SCREEN_INFO.to_ne_bytes());
        out[4..8].copy_from_slice(&16u32.to_ne_bytes());
        // Packed struct: copy fields to locals before use (no field borrows).
        let (sw, sh, sfmt, sref) = (screen.width, screen.height, screen.format, screen.refresh);
        out[8..12].copy_from_slice(&sw.to_ne_bytes());
        out[12..16].copy_from_slice(&sh.to_ne_bytes());
        out[16..20].copy_from_slice(&sfmt.to_ne_bytes());
        out[20..24].copy_from_slice(&sref.to_ne_bytes());
        sys::send_all(fd, &out)?;
        log::info!(
            "anland.broker producer hello protocol={} features=0x{:x}; screen={sw}x{sh} refresh_mhz={sref}",
            hello_version,
            hello_features
        );
        // Serve PICKUP_FDS until the producer goes away.
        loop {
            let mut hdr = [0u8; 8];
            if sys::recv_all(fd, &mut hdr).is_err() {
                return Ok(());
            }
            let msg_type = u32::from_ne_bytes(hdr[0..4].try_into().unwrap());
            let size = u32::from_ne_bytes(hdr[4..8].try_into().unwrap()) as usize;
            if msg_type != CTRL_MSG_PICKUP_FDS {
                if size > 0 && size <= (1 << 20) {
                    let mut ignored = vec![0u8; size];
                    if sys::recv_all(fd, &mut ignored).is_err() {
                        return Ok(());
                    }
                }
                continue;
            }
            self.serve_pickup(fd)?;
        }
    }

    fn send_protocol_reject(fd: &OwnedFd) -> io::Result<()> {
        let mut reject = [0u8; 8 + 8];
        reject[0..4].copy_from_slice(&CTRL_MSG_REJECT.to_ne_bytes());
        reject[4..8].copy_from_slice(&8u32.to_ne_bytes());
        reject[8..12].copy_from_slice(&PROTOCOL_VERSION.to_ne_bytes());
        reject[12..16].copy_from_slice(&PROTOCOL_REQUIRED_FEATURES.to_ne_bytes());
        sys::send_all(fd, &reject)
    }

    fn serve_pickup(&self, fd: &OwnedFd) -> io::Result<()> {
        let (gen, duped, sw, sh) = {
            let slot = self
                .slot
                .lock()
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "broker slot poisoned"))?;
            let deposit = match slot.deposit.as_ref() {
                Some(d) => d,
                None => return Ok(()), // no consumer yet; producer retries in 200ms
            };
            let mut duped = [-1 as libc::c_int; HELLO_FD_COUNT];
            for (i, master) in deposit.fds.iter().enumerate() {
                let dup = unsafe { libc::dup(master.as_raw_fd()) };
                if dup < 0 {
                    for j in 0..i {
                        unsafe { libc::close(duped[j]) };
                    }
                    return Err(io::Error::last_os_error());
                }
                duped[i] = dup;
            }
            (
                deposit.generation,
                duped,
                slot.screen.width,
                slot.screen.height,
            )
        };
        let hdr = {
            let mut h = [0u8; 8];
            h[0..4].copy_from_slice(&CTRL_MSG_FDS_READY.to_ne_bytes());
            h
        };
        // SAFETY: duped fds are owned here; send_fds dups them into the peer.
        let send = sys::send_fds(fd, &hdr, &duped);
        for dup in duped {
            unsafe { libc::close(dup) };
        }
        send?;
        log::info!("anland.broker FDS_READY generation={gen} screen={sw}x{sh}");
        if let Ok(slot) = self.slot.lock() {
            for tx in slot.attach_notify.iter() {
                let _ = tx.send(gen);
            }
        }
        Ok(())
    }
}

impl Clone for Broker {
    fn clone(&self) -> Self {
        Self {
            slot: self.slot.clone(),
            socket_path: self.socket_path.clone(),
        }
    }
}
