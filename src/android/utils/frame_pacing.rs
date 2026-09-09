//! Choreographer-aligned redraw scheduling for the Android-owned surface.
//!
//! Wayland traffic remains event-driven. This layer only coalesces an already
//! requested redraw onto Android's next display frame callback so Portal does
//! not enter a vsynced EGL swap arbitrarily early and wait in the buffer queue.

use crate::android::accessibility::AppUserEvent;
use libloading::Library;
use std::{
    ffi::{c_longlong, c_void},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use winit::event_loop::EventLoopProxy;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AndroidFrameTimeline {
    pub frame_time_ns: i64,
    pub deadline_ns: i64,
    pub expected_present_ns: i64,
    pub vsync_id: i64,
}

impl AndroidFrameTimeline {
    pub fn is_precise(self) -> bool {
        self.deadline_ns > 0 && self.expected_present_ns > 0 && self.vsync_id >= 0
    }
}

#[derive(Debug)]
pub struct FrameTimelineStats {
    started: std::time::Instant,
    callbacks: u64,
    submissions: u64,
    late_submissions: u64,
    lateness_ns: u128,
}

impl Default for FrameTimelineStats {
    fn default() -> Self {
        Self {
            started: std::time::Instant::now(),
            callbacks: 0,
            submissions: 0,
            late_submissions: 0,
            lateness_ns: 0,
        }
    }
}

impl FrameTimelineStats {
    pub fn note_callback(&mut self) {
        self.callbacks += 1;
    }

    pub fn note_submit(&mut self, timeline: AndroidFrameTimeline) {
        self.submissions += 1;
        let now = monotonic_time_ns();
        if timeline.deadline_ns > 0 && now > timeline.deadline_ns {
            self.late_submissions += 1;
            self.lateness_ns += (now - timeline.deadline_ns) as u128;
        }
        if self.started.elapsed() >= std::time::Duration::from_secs(5) {
            let elapsed_seconds = self.started.elapsed().as_secs_f64().max(0.001);
            let average_late_us = if self.late_submissions == 0 {
                0
            } else {
                (self.lateness_ns / self.late_submissions as u128 / 1_000) as u64
            };
            log::info!(
                "frame.timeline callback_hz={:.2} callbacks={} submissions={} late={} avg_late_us={}",
                self.callbacks as f64 / elapsed_seconds,
                self.callbacks,
                self.submissions,
                self.late_submissions,
                average_late_us,
            );
            *self = Self::default();
        }
    }
}

pub fn monotonic_time_ns() -> i64 {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value) };
    if result == 0 {
        value
            .tv_sec
            .saturating_mul(1_000_000_000)
            .saturating_add(value.tv_nsec) as i64
    } else {
        0
    }
}

type Choreographer = c_void;
type GetInstance = unsafe extern "C" fn() -> *mut Choreographer;
type PostFrameCallback64 =
    unsafe extern "C" fn(*mut Choreographer, extern "C" fn(c_longlong, *mut c_void), *mut c_void);
type FrameCallbackData = c_void;
type PostVsyncCallback = unsafe extern "C" fn(
    *mut Choreographer,
    extern "C" fn(*const FrameCallbackData, *mut c_void),
    *mut c_void,
);
type GetFrameTime = unsafe extern "C" fn(*const FrameCallbackData) -> c_longlong;
type GetPreferredTimeline = unsafe extern "C" fn(*const FrameCallbackData) -> usize;
type GetTimelineCount = unsafe extern "C" fn(*const FrameCallbackData) -> usize;
type GetTimelineDeadline = unsafe extern "C" fn(*const FrameCallbackData, usize) -> c_longlong;
type GetTimelineExpectedPresent =
    unsafe extern "C" fn(*const FrameCallbackData, usize) -> c_longlong;
type GetTimelineVsyncId = unsafe extern "C" fn(*const FrameCallbackData, usize) -> c_longlong;

#[derive(Clone, Copy)]
struct TimelineFunctions {
    post: PostVsyncCallback,
    frame_time: GetFrameTime,
    preferred: GetPreferredTimeline,
    count: GetTimelineCount,
    deadline: GetTimelineDeadline,
    expected_present: GetTimelineExpectedPresent,
    vsync_id: GetTimelineVsyncId,
}

unsafe fn load_timeline_functions(library: &Library) -> Option<TimelineFunctions> {
    Some(TimelineFunctions {
        post: *library
            .get::<PostVsyncCallback>(b"AChoreographer_postVsyncCallback\0")
            .ok()?,
        frame_time: *library
            .get::<GetFrameTime>(b"AChoreographerFrameCallbackData_getFrameTimeNanos\0")
            .ok()?,
        preferred: *library
            .get::<GetPreferredTimeline>(
                b"AChoreographerFrameCallbackData_getPreferredFrameTimelineIndex\0",
            )
            .ok()?,
        count: *library
            .get::<GetTimelineCount>(b"AChoreographerFrameCallbackData_getFrameTimelinesLength\0")
            .ok()?,
        deadline: *library
            .get::<GetTimelineDeadline>(
                b"AChoreographerFrameCallbackData_getFrameTimelineDeadlineNanos\0",
            )
            .ok()?,
        expected_present: *library
            .get::<GetTimelineExpectedPresent>(
                b"AChoreographerFrameCallbackData_getFrameTimelineExpectedPresentationTimeNanos\0",
            )
            .ok()?,
        vsync_id: *library
            .get::<GetTimelineVsyncId>(b"AChoreographerFrameCallbackData_getFrameTimelineVsyncId\0")
            .ok()?,
    })
}

struct CallbackState {
    pending: AtomicBool,
    proxy: EventLoopProxy<AppUserEvent>,
    timeline: Option<TimelineFunctions>,
}

/// Main-thread Android frame scheduler. If the NDK Choreographer API is not
/// available, callers transparently fall back to winit's immediate redraw.
pub struct AndroidFramePacer {
    choreographer: *mut Choreographer,
    post_frame_callback: PostFrameCallback64,
    state: Arc<CallbackState>,
    // Retain the symbol owner for the lifetime of the function pointer.
    _library: Library,
}

impl AndroidFramePacer {
    /// Resolve the API dynamically so the APK remains loadable below API 29.
    /// Portal's Pad 3 path uses `AChoreographer_postFrameCallback64`; older
    /// Android versions retain the immediate event-driven fallback.
    pub fn new(proxy: EventLoopProxy<AppUserEvent>) -> Option<Self> {
        let library = unsafe { Library::new("libandroid.so") }.ok()?;
        let get_instance = unsafe {
            *library
                .get::<GetInstance>(b"AChoreographer_getInstance\0")
                .ok()?
        };
        let post_frame_callback = unsafe {
            *library
                .get::<PostFrameCallback64>(b"AChoreographer_postFrameCallback64\0")
                .ok()?
        };
        let choreographer = unsafe { get_instance() };
        if choreographer.is_null() {
            return None;
        }
        Some(Self {
            choreographer,
            post_frame_callback,
            state: Arc::new(CallbackState {
                pending: AtomicBool::new(false),
                proxy,
                timeline: unsafe { load_timeline_functions(&library) },
            }),
            _library: library,
        })
    }

    /// Queue at most one callback. Returns true when Choreographer owns the
    /// redraw request, including when a callback was already pending.
    pub fn request_redraw(&self) -> bool {
        if self
            .state
            .pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return true;
        }
        let callback_state = Arc::into_raw(self.state.clone()) as *mut c_void;
        unsafe {
            if let Some(timeline) = self.state.timeline {
                (timeline.post)(
                    self.choreographer,
                    choreographer_vsync_callback,
                    callback_state,
                );
            } else {
                (self.post_frame_callback)(
                    self.choreographer,
                    choreographer_frame_callback,
                    callback_state,
                );
            }
        }
        true
    }
}

extern "C" fn choreographer_frame_callback(frame_time_ns: c_longlong, data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let state = unsafe { Arc::from_raw(data.cast::<CallbackState>()) };
    state.pending.store(false, Ordering::Release);
    let _ = state.proxy.send_event(AppUserEvent::ChoreographerFrame {
        frame_time_ns: frame_time_ns.max(0),
        deadline_ns: 0,
        expected_present_ns: 0,
        vsync_id: -1,
    });
}

extern "C" fn choreographer_vsync_callback(data: *const FrameCallbackData, state: *mut c_void) {
    if state.is_null() {
        return;
    }
    let state = unsafe { Arc::from_raw(state.cast::<CallbackState>()) };
    state.pending.store(false, Ordering::Release);
    if data.is_null() {
        return;
    }
    let Some(functions) = state.timeline else {
        return;
    };
    let preferred = unsafe { (functions.preferred)(data) };
    if preferred >= unsafe { (functions.count)(data) } {
        return;
    }
    let _ = state.proxy.send_event(AppUserEvent::ChoreographerFrame {
        frame_time_ns: unsafe { (functions.frame_time)(data) }.max(0),
        deadline_ns: unsafe { (functions.deadline)(data, preferred) }.max(0),
        expected_present_ns: unsafe { (functions.expected_present)(data, preferred) }.max(0),
        vsync_id: unsafe { (functions.vsync_id)(data, preferred) },
    });
}

/// Give the NativeActivity thread Android's standard display priority after
/// guest/audio workers have been spawned, so they retain normal scheduling.
/// Failure is harmless and leaves the platform default unchanged.
pub fn prioritize_current_render_thread() {
    const THREAD_PRIORITY_DISPLAY: libc::c_int = -4;
    let result = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, THREAD_PRIORITY_DISPLAY) };
    if result != 0 {
        log::warn!(
            "Could not apply Android display-thread priority: {}",
            std::io::Error::last_os_error()
        );
    }
}
