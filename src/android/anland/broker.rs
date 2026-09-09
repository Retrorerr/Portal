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
        // First message must be PRODUCER_HELLO (8 bytes, no fds).
        let mut hdr = [0u8; 8];
        sys::recv_all(fd, &mut hdr)?;
        let msg_type = u32::from_ne_bytes(hdr[0..4].try_into().unwrap());
        if msg_type != CTRL_MSG_PRODUCER_HELLO {
            log::warn!("anland.broker unexpected first message type={msg_type}");
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected PRODUCER_HELLO",
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
        log::info!("anland.broker producer hello; screen={sw}x{sh} refresh_mhz={sref}");
        // Serve PICKUP_FDS until the producer goes away.
        loop {
            let mut hdr = [0u8; 8];
            if sys::recv_all(fd, &mut hdr).is_err() {
                return Ok(());
            }
            let msg_type = u32::from_ne_bytes(hdr[0..4].try_into().unwrap());
            if msg_type != CTRL_MSG_PICKUP_FDS {
                continue;
            }
            self.serve_pickup(fd)?;
        }
    }

    fn serve_pickup(&self, fd: &OwnedFd) -> io::Result<()> {
        let (gen, duped) = {
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
            (deposit.generation, duped)
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
        log::info!("anland.broker FDS_READY generation={gen}");
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
