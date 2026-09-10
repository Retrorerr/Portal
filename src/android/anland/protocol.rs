//! Project Anland wire protocol (Portal-native Rust).
//!
//! Byte-compatible reimplementation of `third_party/anland/common/protocol.h`
//! (SuperTurtleDev/anland, `legacy`). Only the frame steady-state subset is
//! implemented: handshake, dma-buf set exchange, buffer select, render fence,
//! fixed-size input events and output-event draining. Audio/camera service fds
//! are deferred (guest runs with `ANLAND_DISABLE_AUDIO=1`; service requests
//! are logged and left unanswered, which the producer treats as disabled).
//!
//! Layout is verified at compile time against the C header sizes.

use std::mem::size_of;

// ---- control channel -------------------------------------------------------

pub const CTRL_MSG_CONSUMER_HELLO: u32 = 1;
pub const CTRL_MSG_PRODUCER_HELLO: u32 = 2;
pub const CTRL_MSG_SCREEN_INFO: u32 = 7;
pub const CTRL_MSG_REJECT: u32 = 8;
pub const CTRL_MSG_PICKUP_FDS: u32 = 9;
pub const CTRL_MSG_FDS_READY: u32 = 10;

// ---- data channel ----------------------------------------------------------

pub const DATA_MSG_BUF_READY: u32 = 100;
#[allow(dead_code)]
pub const DATA_MSG_REFRESH_DONE: u32 = 101;
pub const DATA_MSG_INPUT_EVENT: u32 = 102;
pub const DATA_MSG_OUTPUT_EVENT: u32 = 103;
#[allow(dead_code)]
pub const DATA_MSG_INPUT_EXTEND_FDS: u32 = 104;
pub const DATA_MSG_BUFS_READY: u32 = 200;

pub const MAX_BUFS: usize = 8;

/// Hello fd slot order. Must match the producer's `pickup_fds()` and the
/// reference `send_hello_fds()`: { buf_ready, fence, data, shm, audio }.
pub const HELLO_FD_COUNT: usize = 5;

// ---- input event types -----------------------------------------------------

pub const INPUT_TYPE_TOUCH: u32 = 1;
pub const INPUT_TYPE_KEY: u32 = 2;
pub const INPUT_TYPE_POINTER_MOTION: u32 = 3;
pub const INPUT_TYPE_POINTER_BUTTON: u32 = 4;
pub const INPUT_TYPE_POINTER_AXIS: u32 = 5;
pub const INPUT_TYPE_TOUCH_FRAME: u32 = 6;
pub const INPUT_TYPE_DISPLAY_REFRESH: u32 = 7;
#[allow(dead_code)]
pub const INPUT_TYPE_CLIPBOARD: u32 = 8;
#[allow(dead_code)]
pub const INPUT_TYPE_TEXT_INPUT: u32 = 9;
/// Touchpad finger-source smooth scroll (buffer-px delta). The rebuilt KWin
/// backend emits these with PointerAxisSource::Finger (kinetic scrolling)
/// and applies the Portal Touchpad kcminputrc settings (factor/direction).
/// Unlike INPUT_TYPE_POINTER_AXIS there is no discrete component.
pub const INPUT_TYPE_POINTER_AXIS_FINGER: u32 = 13;
/// Terminates an active finger scroll stream (zero-delta event preserving
/// the finger source, like the nested backend's axisStopped handler).
pub const INPUT_TYPE_POINTER_AXIS_STOP: u32 = 14;

pub const INPUT_ACTION_DOWN: i32 = 0;
pub const INPUT_ACTION_UP: i32 = 1;
pub const INPUT_ACTION_MOVE: i32 = 2;

// ---- output event types ----------------------------------------------------

pub const OUTPUT_TYPE_CLIPBOARD: u32 = 1;
pub const OUTPUT_TYPE_RESOURCES_REQUEST: u32 = 2;
pub const OUTPUT_TYPE_SET_CONSUMER_VAR: u32 = 3;
pub const OUTPUT_TYPE_SCHEDULING: u32 = 4;

// ---- pixel format (consumer-side enum; KWin maps 1 -> DRM ABGR8888) --------

pub const PIXEL_FORMAT_RGBA_8888: u32 = 1;

// ---- packed wire structs ---------------------------------------------------

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct CtrlMsg {
    pub msg_type: u32,
    pub size: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct DataMsg {
    pub msg_type: u32,
    pub size: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default, Debug)]
pub struct ScreenInfo {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub refresh: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default, Debug)]
pub struct BufInfo {
    pub stride: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub modifier: u64,
    pub offset: u32,
}

/// Fixed 20-byte event body. The trailing 16 bytes are the type-specific
/// union; constructors below fill them. Keeps the hot path free of Rust
/// `union` unsafety while staying wire-identical.
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct InputEvent {
    pub ev_type: u32,
    pub payload: [u8; 16],
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct OutputEvent {
    pub ev_type: u32,
    pub payload: [u8; 16],
}

const _: [(); 8] = [(); size_of::<CtrlMsg>()];
const _: [(); 8] = [(); size_of::<DataMsg>()];
const _: [(); 16] = [(); size_of::<ScreenInfo>()];
const _: [(); 28] = [(); size_of::<BufInfo>()];
const _: [(); 20] = [(); size_of::<InputEvent>()];
const _: [(); 20] = [(); size_of::<OutputEvent>()];

fn put_i32(buf: &mut [u8; 16], off: usize, v: i32) {
    buf[off..off + 4].copy_from_slice(&v.to_ne_bytes());
}

fn put_u32(buf: &mut [u8; 16], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_ne_bytes());
}

fn put_f32(buf: &mut [u8; 16], off: usize, v: f32) {
    buf[off..off + 4].copy_from_slice(&v.to_ne_bytes());
}

impl InputEvent {
    pub fn touch(action: i32, x: f32, y: f32, pointer_id: i32) -> Self {
        let mut payload = [0u8; 16];
        put_i32(&mut payload, 0, action);
        put_f32(&mut payload, 4, x);
        put_f32(&mut payload, 8, y);
        put_i32(&mut payload, 12, pointer_id);
        Self {
            ev_type: INPUT_TYPE_TOUCH,
            payload,
        }
    }

    pub fn touch_frame() -> Self {
        Self {
            ev_type: INPUT_TYPE_TOUCH_FRAME,
            payload: [0u8; 16],
        }
    }

    pub fn key(action: i32, keycode: i32) -> Self {
        let mut payload = [0u8; 16];
        put_i32(&mut payload, 0, action);
        put_i32(&mut payload, 4, keycode);
        Self {
            ev_type: INPUT_TYPE_KEY,
            payload,
        }
    }

    pub fn pointer_motion(x: f32, y: f32, dx: f32, dy: f32) -> Self {
        let mut payload = [0u8; 16];
        put_f32(&mut payload, 0, x);
        put_f32(&mut payload, 4, y);
        put_f32(&mut payload, 8, dx);
        put_f32(&mut payload, 12, dy);
        Self {
            ev_type: INPUT_TYPE_POINTER_MOTION,
            payload,
        }
    }

    pub fn pointer_button(button: u32, pressed: bool) -> Self {
        let mut payload = [0u8; 16];
        put_u32(&mut payload, 0, button);
        put_i32(&mut payload, 4, if pressed { 1 } else { 0 });
        Self {
            ev_type: INPUT_TYPE_POINTER_BUTTON,
            payload,
        }
    }

    pub fn pointer_axis(axis: u32, value: f32, discrete: i32) -> Self {
        let mut payload = [0u8; 16];
        put_u32(&mut payload, 0, axis);
        put_f32(&mut payload, 4, value);
        put_i32(&mut payload, 8, discrete);
        Self {
            ev_type: INPUT_TYPE_POINTER_AXIS,
            payload,
        }
    }

    /// Finger-source scroll value (axis 0 = vertical, 1 = horizontal).
    pub fn finger_axis(axis: u32, value: f32) -> Self {
        let mut payload = [0u8; 16];
        put_u32(&mut payload, 0, axis);
        put_f32(&mut payload, 4, value);
        Self {
            ev_type: INPUT_TYPE_POINTER_AXIS_FINGER,
            payload,
        }
    }

    /// Finger scroll stream terminator (axis 0 = vertical, 1 = horizontal).
    pub fn finger_stop(axis: u32) -> Self {
        let mut payload = [0u8; 16];
        put_u32(&mut payload, 0, axis);
        Self {
            ev_type: INPUT_TYPE_POINTER_AXIS_STOP,
            payload,
        }
    }

    pub fn display_refresh(refresh_mhz: u32) -> Self {
        let mut payload = [0u8; 16];
        put_u32(&mut payload, 0, refresh_mhz);
        Self {
            ev_type: INPUT_TYPE_DISPLAY_REFRESH,
            payload,
        }
    }

    /// TEXT_INPUT header (type 9): `{size}` + `size` trailing UTF-8 bytes sent
    /// as a second write on the data channel (same framing as clipboard).
    /// The KWin backend feeds the bytes to `inputMethod()->commitText()`.
    pub fn text_input(size: u32) -> Self {
        let mut payload = [0u8; 16];
        put_u32(&mut payload, 0, size);
        Self {
            ev_type: INPUT_TYPE_TEXT_INPUT,
            payload,
        }
    }
}

impl OutputEvent {
    /// Clipboard payload length as carried in the fixed header. A non-zero
    /// value means `size` trailing bytes must be drained from the data channel
    /// even when the payload itself is discarded.
    pub fn clipboard_size(&self) -> u32 {
        u32::from_ne_bytes([
            self.payload[0],
            self.payload[1],
            self.payload[2],
            self.payload[3],
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_sizes_match_c_header() {
        assert_eq!(size_of::<CtrlMsg>(), 8);
        assert_eq!(size_of::<DataMsg>(), 8);
        assert_eq!(size_of::<ScreenInfo>(), 16);
        assert_eq!(size_of::<BufInfo>(), 28);
        assert_eq!(size_of::<InputEvent>(), 20);
        assert_eq!(size_of::<OutputEvent>(), 20);
    }

    #[test]
    fn touch_encodes_action_xy_id() {
        let ev = InputEvent::touch(INPUT_ACTION_DOWN, 100.5, 200.25, 3);
        assert_eq!(ev.ev_type, INPUT_TYPE_TOUCH);
        assert_eq!(i32::from_ne_bytes(ev.payload[0..4].try_into().unwrap()), 0);
        assert_eq!(
            f32::from_ne_bytes(ev.payload[4..8].try_into().unwrap()),
            100.5
        );
        assert_eq!(
            f32::from_ne_bytes(ev.payload[8..12].try_into().unwrap()),
            200.25
        );
        assert_eq!(
            i32::from_ne_bytes(ev.payload[12..16].try_into().unwrap()),
            3
        );
    }

    #[test]
    fn key_and_button_layout() {
        let ev = InputEvent::key(INPUT_ACTION_DOWN, 30);
        assert_eq!(i32::from_ne_bytes(ev.payload[4..8].try_into().unwrap()), 30);
        let ev = InputEvent::pointer_button(272, true);
        assert_eq!(
            u32::from_ne_bytes(ev.payload[0..4].try_into().unwrap()),
            272
        );
        assert_eq!(i32::from_ne_bytes(ev.payload[4..8].try_into().unwrap()), 1);
    }

    #[test]
    fn hello_slot_count_matches_producer() {
        assert_eq!(HELLO_FD_COUNT, 5);
        assert!(MAX_BUFS <= 8);
    }
}
