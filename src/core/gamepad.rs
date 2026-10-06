//! Android game controllers presented to the guest as Linux evdev pads.
//!
//! Every pad, whatever its make, is presented with the identity and layout
//! of an Xbox 360 controller as the kernel's xpad driver reports it
//! (BUS_USB 045e:028e). SDL's controller database, Wine's XInput layer and
//! Steam Input all map that identity without configuration, which matters
//! more here than a faithful vendor name: Portal cannot offer the guest
//! hidraw, so nothing could use vendor-specific features anyway.
//!
//! This module is pure state: Android key/axis values in, evdev
//! `(type, code, value)` frames out. The transport lives in
//! `android::gamepad`.

pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;
pub const EV_FF: u16 = 0x15;
pub const SYN_REPORT: u16 = 0;

pub const BTN_A: u16 = 0x130;
pub const BTN_B: u16 = 0x131;
pub const BTN_X: u16 = 0x133;
pub const BTN_Y: u16 = 0x134;
pub const BTN_TL: u16 = 0x136;
pub const BTN_TR: u16 = 0x137;
pub const BTN_SELECT: u16 = 0x13a;
pub const BTN_START: u16 = 0x13b;
pub const BTN_MODE: u16 = 0x13c;
pub const BTN_THUMBL: u16 = 0x13d;
pub const BTN_THUMBR: u16 = 0x13e;

pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_Z: u16 = 0x02;
pub const ABS_RX: u16 = 0x03;
pub const ABS_RY: u16 = 0x04;
pub const ABS_RZ: u16 = 0x05;
pub const ABS_HAT0X: u16 = 0x10;
pub const ABS_HAT0Y: u16 = 0x11;

pub const FF_RUMBLE: u16 = 0x50;

pub const BUS_USB: u16 = 0x03;
pub const XBOX360_VENDOR: u16 = 0x045e;
pub const XBOX360_PRODUCT: u16 = 0x028e;
pub const XBOX360_VERSION: u16 = 0x0114;
pub const PAD_NAME: &str = "Microsoft X-Box 360 pad";

/// Buttons in the order xpad registers them.
pub const BUTTONS: [u16; 11] = [
    BTN_A, BTN_B, BTN_X, BTN_Y, BTN_TL, BTN_TR, BTN_SELECT, BTN_START, BTN_MODE, BTN_THUMBL,
    BTN_THUMBR,
];

/// Absolute axes and their (min, max, fuzz, flat), as xpad reports them.
pub const AXES: [(u16, i32, i32, i32, i32); 8] = [
    (ABS_X, -32768, 32767, 16, 128),
    (ABS_Y, -32768, 32767, 16, 128),
    (ABS_Z, 0, 255, 0, 0),
    (ABS_RX, -32768, 32767, 16, 128),
    (ABS_RY, -32768, 32767, 16, 128),
    (ABS_RZ, 0, 255, 0, 0),
    (ABS_HAT0X, -1, 1, 0, 0),
    (ABS_HAT0Y, -1, 1, 0, 0),
];

// Android KeyEvent codes.
const KEYCODE_DPAD_UP: i32 = 19;
const KEYCODE_DPAD_DOWN: i32 = 20;
const KEYCODE_DPAD_LEFT: i32 = 21;
const KEYCODE_DPAD_RIGHT: i32 = 22;
const KEYCODE_BACK: i32 = 4;
const KEYCODE_BUTTON_A: i32 = 96;
const KEYCODE_BUTTON_B: i32 = 97;
const KEYCODE_BUTTON_X: i32 = 99;
const KEYCODE_BUTTON_Y: i32 = 100;
const KEYCODE_BUTTON_L1: i32 = 102;
const KEYCODE_BUTTON_R1: i32 = 103;
const KEYCODE_BUTTON_L2: i32 = 104;
const KEYCODE_BUTTON_R2: i32 = 105;
const KEYCODE_BUTTON_THUMBL: i32 = 106;
const KEYCODE_BUTTON_THUMBR: i32 = 107;
const KEYCODE_BUTTON_START: i32 = 108;
const KEYCODE_BUTTON_SELECT: i32 = 109;
const KEYCODE_BUTTON_MODE: i32 = 110;

/// Android MotionEvent axes Kotlin sends, in this order.
pub const ANDROID_AXES: [i32; 10] = [
    0,  // AXIS_X: left stick X
    1,  // AXIS_Y: left stick Y
    11, // AXIS_Z: right stick X
    14, // AXIS_RZ: right stick Y
    17, // AXIS_LTRIGGER
    18, // AXIS_RTRIGGER
    23, // AXIS_BRAKE (left trigger on some pads)
    22, // AXIS_GAS (right trigger on some pads)
    15, // AXIS_HAT_X
    16, // AXIS_HAT_Y
];

/// One evdev event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvdevEvent {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

impl EvdevEvent {
    const fn new(kind: u16, code: u16, value: i32) -> Self {
        Self { kind, code, value }
    }
}

/// What a pad currently reports.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PadState {
    /// Pressed buttons, indexed like [`BUTTONS`].
    pub buttons: [bool; 11],
    /// Axis values, indexed like [`AXES`].
    pub axes: [i32; 8],
    /// D-pad held as keys (pads that report it that way, not as a hat).
    dpad_keys: [bool; 4],
    /// Trigger held as a digital L2/R2 key (pads with no analog trigger).
    digital_triggers: [bool; 2],
    /// Analog trigger values seen from the axes, so a key release does not
    /// zero an analog trigger that is still held.
    analog_triggers: [i32; 2],
}

fn button_index(code: u16) -> Option<usize> {
    BUTTONS.iter().position(|&button| button == code)
}

fn stick(value: f32) -> i32 {
    // Android reports -1..1; xpad's range is asymmetric.
    let value = value.clamp(-1.0, 1.0);
    if value >= 0.0 {
        (value * 32767.0).round() as i32
    } else {
        (value * 32768.0).round() as i32
    }
}

fn trigger(value: f32) -> i32 {
    (value.clamp(0.0, 1.0) * 255.0).round() as i32
}

fn hat(value: f32) -> i32 {
    if value > 0.5 {
        1
    } else if value < -0.5 {
        -1
    } else {
        0
    }
}

impl PadState {
    /// Apply an Android key event. Returns false for keys a pad does not
    /// have, which the caller hands back to Android.
    pub fn apply_key(&mut self, keycode: i32, down: bool) -> bool {
        let button = match keycode {
            KEYCODE_BUTTON_A => BTN_A,
            KEYCODE_BUTTON_B => BTN_B,
            KEYCODE_BUTTON_X => BTN_X,
            KEYCODE_BUTTON_Y => BTN_Y,
            KEYCODE_BUTTON_L1 => BTN_TL,
            KEYCODE_BUTTON_R1 => BTN_TR,
            KEYCODE_BUTTON_THUMBL => BTN_THUMBL,
            KEYCODE_BUTTON_THUMBR => BTN_THUMBR,
            KEYCODE_BUTTON_START => BTN_START,
            // Many pads send their View/Share/Back button as KEYCODE_BACK.
            KEYCODE_BUTTON_SELECT | KEYCODE_BACK => BTN_SELECT,
            KEYCODE_BUTTON_MODE => BTN_MODE,
            KEYCODE_BUTTON_L2 | KEYCODE_BUTTON_R2 => {
                let side = usize::from(keycode == KEYCODE_BUTTON_R2);
                self.digital_triggers[side] = down;
                self.update_trigger(side);
                return true;
            }
            KEYCODE_DPAD_UP | KEYCODE_DPAD_DOWN | KEYCODE_DPAD_LEFT | KEYCODE_DPAD_RIGHT => {
                let index = (keycode - KEYCODE_DPAD_UP) as usize;
                self.dpad_keys[index] = down;
                let [up, down_held, left, right] = self.dpad_keys;
                self.axes[6] = i32::from(right) - i32::from(left);
                self.axes[7] = i32::from(down_held) - i32::from(up);
                return true;
            }
            _ => return false,
        };
        if let Some(index) = button_index(button) {
            self.buttons[index] = down;
        }
        true
    }

    fn update_trigger(&mut self, side: usize) {
        let digital = if self.digital_triggers[side] { 255 } else { 0 };
        self.axes[2 + side * 3] = self.analog_triggers[side].max(digital);
    }

    /// Apply an Android motion sample: values for [`ANDROID_AXES`].
    pub fn apply_axes(&mut self, values: &[f32; 10]) {
        self.axes[0] = stick(values[0]);
        self.axes[1] = stick(values[1]);
        self.axes[3] = stick(values[2]);
        self.axes[4] = stick(values[3]);
        // Pads report a trigger as LTRIGGER/RTRIGGER, BRAKE/GAS, or both.
        self.analog_triggers = [
            trigger(values[4].max(values[6])),
            trigger(values[5].max(values[7])),
        ];
        self.update_trigger(0);
        self.update_trigger(1);
        // A pad with a hat reports it here; keys-only d-pads leave it at 0
        // and are handled in apply_key.
        if values[8] != 0.0 || values[9] != 0.0 || self.dpad_keys == [false; 4] {
            self.axes[6] = hat(values[8]);
            self.axes[7] = hat(values[9]);
        }
    }

    /// Events that turn `previous` into `self`, ending in SYN_REPORT, or
    /// nothing when nothing changed.
    pub fn diff(&self, previous: &PadState) -> Vec<EvdevEvent> {
        let mut events = Vec::new();
        for (index, &button) in BUTTONS.iter().enumerate() {
            if self.buttons[index] != previous.buttons[index] {
                events.push(EvdevEvent::new(EV_KEY, button, i32::from(self.buttons[index])));
            }
        }
        for (index, &(axis, ..)) in AXES.iter().enumerate() {
            if self.axes[index] != previous.axes[index] {
                events.push(EvdevEvent::new(EV_ABS, axis, self.axes[index]));
            }
        }
        if !events.is_empty() {
            events.push(EvdevEvent::new(EV_SYN, SYN_REPORT, 0));
        }
        events
    }
}

/// Rumble strengths (0..=0xffff) from an `ff_effect` of type FF_RUMBLE.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rumble {
    pub strong: u16,
    pub weak: u16,
    pub duration_ms: u16,
}

impl Rumble {
    /// Android amplitude 1..=255 for the stronger motor, or 0 to stop.
    pub fn amplitude(&self) -> i32 {
        let level = self.strong.max(self.weak);
        if level == 0 {
            0
        } else {
            (i32::from(level) * 255 / 0xffff).max(1)
        }
    }
}

/// Size of `struct input_event` on aarch64: timeval, type, code, value.
pub const INPUT_EVENT_SIZE: usize = 24;

/// Records PRoot writes on a pad's pty about rumble effects (see
/// `syscall/evdev.c`), shaped like input events with a type past EV_MAX.
pub const FF_UPLOAD_RECORD: u16 = 0xffff;
pub const FF_ERASE_RECORD: u16 = 0xfffe;
pub const FF_GAIN: u16 = 0x60;
pub const FF_EFFECTS: usize = 16;

/// Serialise events as `struct input_event`s stamped `sec`.`usec`.
pub fn encode_events(events: &[EvdevEvent], sec: i64, usec: i64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(events.len() * INPUT_EVENT_SIZE);
    for event in events {
        bytes.extend_from_slice(&sec.to_le_bytes());
        bytes.extend_from_slice(&usec.to_le_bytes());
        bytes.extend_from_slice(&event.kind.to_le_bytes());
        bytes.extend_from_slice(&event.code.to_le_bytes());
        bytes.extend_from_slice(&event.value.to_le_bytes());
    }
    bytes
}

/// What the guest side of a pad's pty asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PadRequest {
    /// EVIOCSFF stored a rumble effect (relayed by PRoot).
    Upload { id: usize, rumble: Rumble },
    /// EVIOCRMFF removed an effect (relayed by PRoot).
    Erase { id: usize },
    /// The guest wrote EV_FF: start (`count > 0`) or stop an effect.
    Play { id: usize, count: i32 },
    /// The guest wrote EV_FF/FF_GAIN (0..=0xffff).
    Gain(u16),
}

/// Parse one record read from a pad's pty master.
pub fn parse_request(record: &[u8; INPUT_EVENT_SIZE]) -> Option<PadRequest> {
    let usec = u64::from_le_bytes(record[8..16].try_into().ok()?);
    let kind = u16::from_le_bytes([record[16], record[17]]);
    let code = u16::from_le_bytes([record[18], record[19]]);
    let value = i32::from_le_bytes(record[20..24].try_into().ok()?);
    let id = usize::from(code);
    match kind {
        FF_UPLOAD_RECORD if id < FF_EFFECTS => Some(PadRequest::Upload {
            id,
            rumble: Rumble {
                strong: (usec >> 16) as u16,
                weak: usec as u16,
                duration_ms: value.clamp(0, i32::from(u16::MAX)) as u16,
            },
        }),
        FF_ERASE_RECORD if id < FF_EFFECTS => Some(PadRequest::Erase { id }),
        EV_FF if code == FF_GAIN => Some(PadRequest::Gain(value.clamp(0, 0xffff) as u16)),
        EV_FF if id < FF_EFFECTS => Some(PadRequest::Play { id, count: value }),
        _ => None,
    }
}

/// Rumble effects a pad holds, as the guest uploaded them.
#[derive(Clone, Debug)]
pub struct EffectTable {
    effects: [Rumble; FF_EFFECTS],
    gain: u16,
}

impl Default for EffectTable {
    fn default() -> Self {
        Self { effects: [Rumble::default(); FF_EFFECTS], gain: 0xffff }
    }
}

impl EffectTable {
    /// Apply a request; returns the rumble to start (zero strengths stop).
    pub fn apply(&mut self, request: PadRequest) -> Option<Rumble> {
        match request {
            PadRequest::Upload { id, rumble } => {
                self.effects[id] = rumble;
                None
            }
            PadRequest::Erase { id } => {
                self.effects[id] = Rumble::default();
                Some(Rumble::default())
            }
            PadRequest::Gain(gain) => {
                self.gain = gain;
                None
            }
            PadRequest::Play { id, count } => {
                if count <= 0 {
                    return Some(Rumble::default());
                }
                let effect = self.effects[id];
                let scale = |level: u16| (u32::from(level) * u32::from(self.gain) / 0xffff) as u16;
                Some(Rumble {
                    strong: scale(effect.strong),
                    weak: scale(effect.weak),
                    // A zero length means "until stopped" in evdev.
                    duration_ms: if effect.duration_ms == 0 { u16::MAX } else { effect.duration_ms },
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axes(values: [f32; 10]) -> PadState {
        let mut pad = PadState::default();
        pad.apply_axes(&values);
        pad
    }

    #[test]
    fn buttons_map_to_xpad_codes_and_report_once() {
        let mut pad = PadState::default();
        assert!(pad.apply_key(KEYCODE_BUTTON_A, true));
        let events = pad.diff(&PadState::default());
        assert_eq!(
            events,
            vec![
                EvdevEvent::new(EV_KEY, BTN_A, 1),
                EvdevEvent::new(EV_SYN, SYN_REPORT, 0)
            ]
        );
        assert!(pad.diff(&pad).is_empty());
        assert!(pad.apply_key(KEYCODE_BACK, true));
        assert!(pad.buttons[button_index(BTN_SELECT).unwrap()]);
        assert!(!pad.apply_key(66, true)); // Enter is not a pad key
    }

    #[test]
    fn sticks_triggers_and_hat_use_xpad_ranges() {
        let pad = axes([1.0, -1.0, 0.5, 0.0, 1.0, 0.0, 0.0, 0.5, -1.0, 1.0]);
        assert_eq!(pad.axes[0], 32767);
        assert_eq!(pad.axes[1], -32768);
        assert_eq!(pad.axes[3], 16384);
        assert_eq!(pad.axes[2], 255); // LT from LTRIGGER
        assert_eq!(pad.axes[5], 128); // RT from GAS
        assert_eq!((pad.axes[6], pad.axes[7]), (-1, 1));
    }

    #[test]
    fn dpad_keys_and_digital_triggers_survive_axis_samples() {
        let mut pad = PadState::default();
        pad.apply_key(KEYCODE_DPAD_UP, true);
        pad.apply_key(KEYCODE_DPAD_RIGHT, true);
        pad.apply_key(KEYCODE_BUTTON_R2, true);
        pad.apply_axes(&[0.0; 10]);
        assert_eq!((pad.axes[6], pad.axes[7]), (1, -1));
        assert_eq!(pad.axes[5], 255);
        pad.apply_key(KEYCODE_BUTTON_R2, false);
        assert_eq!(pad.axes[5], 0);
    }

    fn record(kind: u16, code: u16, value: i32, usec: u64) -> [u8; INPUT_EVENT_SIZE] {
        let mut bytes = [0u8; INPUT_EVENT_SIZE];
        bytes[8..16].copy_from_slice(&usec.to_le_bytes());
        bytes[16..18].copy_from_slice(&kind.to_le_bytes());
        bytes[18..20].copy_from_slice(&code.to_le_bytes());
        bytes[20..24].copy_from_slice(&value.to_le_bytes());
        bytes
    }

    #[test]
    fn events_encode_as_aarch64_input_events() {
        let bytes = encode_events(&[EvdevEvent::new(EV_KEY, BTN_A, 1)], 7, 9);
        assert_eq!(bytes.len(), INPUT_EVENT_SIZE);
        assert_eq!(&bytes[0..8], &7i64.to_le_bytes());
        assert_eq!(&bytes[16..18], &EV_KEY.to_le_bytes());
        assert_eq!(&bytes[18..20], &BTN_A.to_le_bytes());
        assert_eq!(&bytes[20..24], &1i32.to_le_bytes());
    }

    #[test]
    fn rumble_requests_play_the_uploaded_effect_scaled_by_gain() {
        let mut table = EffectTable::default();
        let upload = parse_request(&record(FF_UPLOAD_RECORD, 2, 500, 0xc000_4000)).unwrap();
        assert_eq!(
            upload,
            PadRequest::Upload { id: 2, rumble: Rumble { strong: 0xc000, weak: 0x4000, duration_ms: 500 } }
        );
        assert_eq!(table.apply(upload), None);
        let play = parse_request(&record(EV_FF, 2, 1, 0)).unwrap();
        assert_eq!(table.apply(play).unwrap().strong, 0xc000);
        table.apply(parse_request(&record(EV_FF, FF_GAIN, 0x8000, 0)).unwrap());
        assert_eq!(table.apply(play).unwrap().strong, 0x6000);
        assert_eq!(table.apply(parse_request(&record(EV_FF, 2, 0, 0)).unwrap()), Some(Rumble::default()));
        assert_eq!(parse_request(&record(EV_KEY, BTN_A, 1, 0)), None);
        assert_eq!(parse_request(&record(FF_UPLOAD_RECORD, 99, 1, 0)), None);
    }

    #[test]
    fn rumble_amplitude_scales_to_android() {
        assert_eq!(Rumble::default().amplitude(), 0);
        assert_eq!(Rumble { strong: 0xffff, weak: 0, duration_ms: 100 }.amplitude(), 255);
        assert_eq!(Rumble { strong: 0, weak: 1, duration_ms: 100 }.amplitude(), 1);
    }
}
