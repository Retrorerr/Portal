use crate::android::accessibility::AppUserEvent;
use std::{
    os::unix::io::RawFd,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
};
use winit::event_loop::EventLoopProxy;

/// Monitors the Wayland listening socket and Display poll descriptor in the background.
/// When client traffic or a new connection arrives, it wakes the winit event loop via
/// `EventLoopProxy<AppUserEvent>`, enabling `ControlFlow::Wait` without polling loops.
pub struct WaylandSocketWatcher {
    stop_eventfd: RawFd,
    shutdown: Arc<AtomicBool>,
    pending_lock: Arc<(Mutex<bool>, Condvar)>,
    thread_handle: Option<JoinHandle<()>>,
}

impl WaylandSocketWatcher {
    pub fn spawn(
        listener_fd: RawFd,
        display_fd: RawFd,
        proxy: EventLoopProxy<AppUserEvent>,
    ) -> Result<Self, String> {
        let stop_eventfd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
        if stop_eventfd < 0 {
            return Err(format!(
                "Failed to create eventfd for WaylandSocketWatcher: errno {}",
                std::io::Error::last_os_error()
            ));
        }

        let shutdown = Arc::new(AtomicBool::new(false));
        let pending_lock = Arc::new((Mutex::new(false), Condvar::new()));

        let shutdown_clone = shutdown.clone();
        let pending_clone = pending_lock.clone();

        let thread_handle = thread::Builder::new()
            .name("wayland-socket-watcher".into())
            .spawn(move || {
                Self::worker_loop(
                    stop_eventfd,
                    listener_fd,
                    display_fd,
                    proxy,
                    shutdown_clone,
                    pending_clone,
                );
            })
            .map_err(|err| format!("Failed to spawn wayland-socket-watcher thread: {err}"))?;

        Ok(Self {
            stop_eventfd,
            shutdown,
            pending_lock,
            thread_handle: Some(thread_handle),
        })
    }

    /// Inform the watcher that the main thread has finished dispatching pending Wayland
    /// messages, so it may resume kernel polling.
    pub fn resume_polling(&self) {
        let (lock, cvar) = &*self.pending_lock;
        if let Ok(mut pending) = lock.lock() {
            if *pending {
                *pending = false;
                cvar.notify_one();
            }
        }
    }

    fn worker_loop(
        stop_fd: RawFd,
        listener_fd: RawFd,
        display_fd: RawFd,
        proxy: EventLoopProxy<AppUserEvent>,
        shutdown: Arc<AtomicBool>,
        pending_lock: Arc<(Mutex<bool>, Condvar)>,
    ) {
        let mut fds = [
            libc::pollfd {
                fd: stop_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: listener_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: display_fd,
                events: libc::POLLIN,
                revents: 0,
            },
        ];

        log::info!(
            "WaylandSocketWatcher started: stop_fd={stop_fd}, listener_fd={listener_fd}, display_fd={display_fd}"
        );

        while !shutdown.load(Ordering::SeqCst) {
            fds[0].revents = 0;
            fds[1].revents = 0;
            fds[2].revents = 0;

            let ret = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
            if ret < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                log::warn!("WaylandSocketWatcher libc::poll failed: {err}");
                break;
            }

            if shutdown.load(Ordering::SeqCst) {
                break;
            }

            // Stop requested via eventfd
            if (fds[0].revents & libc::POLLIN) != 0 {
                break;
            }

            let has_listener_event = (fds[1].revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP)) != 0;
            let has_display_event = (fds[2].revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP)) != 0;

            if has_listener_event || has_display_event {
                let _ = proxy.send_event(AppUserEvent::WaylandTraffic);

                // Wait for the main thread to complete dispatching before polling level-triggered FDs again
                let (lock, cvar) = &*pending_lock;
                if let Ok(mut pending) = lock.lock() {
                    *pending = true;
                    while *pending && !shutdown.load(Ordering::SeqCst) {
                        match cvar.wait(pending) {
                            Ok(guard) => pending = guard,
                            Err(poisoned) => pending = poisoned.into_inner(),
                        }
                    }
                }
            }
        }

        log::info!("WaylandSocketWatcher thread exiting cleanly");
    }
}

impl Drop for WaylandSocketWatcher {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);

        // Wake eventfd if blocked in poll
        let val: u64 = 1;
        unsafe {
            libc::write(
                self.stop_eventfd,
                &val as *const _ as *const libc::c_void,
                std::mem::size_of::<u64>(),
            );
        }

        // Wake condvar if waiting for main thread
        let (lock, cvar) = &*self.pending_lock;
        if let Ok(mut pending) = lock.lock() {
            *pending = false;
            cvar.notify_all();
        }

        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }

        unsafe {
            libc::close(self.stop_eventfd);
        }
    }
}
