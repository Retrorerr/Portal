//! Classification of Android pointer events by their raw source bits and tool
//! type. Dependency-free so the host tests can include it directly.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// `InputDevice.SOURCE_MOUSE`.
pub const SOURCE_MOUSE: u32 = 0x0000_2002;
/// `InputDevice.SOURCE_TOUCHPAD`.
pub const SOURCE_TOUCHPAD: u32 = 0x0010_0008;

/// `MotionEvent.TOOL_TYPE_FINGER`.
pub const TOOL_TYPE_FINGER: u32 = 1;
/// `MotionEvent.TOOL_TYPE_MOUSE`.
pub const TOOL_TYPE_MOUSE: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerClass {
    /// A mouse: moves the pointer and clicks, from button actions or from a
    /// bare Down/Up.
    Mouse,
    /// A touchpad: moves the pointer, scrolls and pinches; clicks come only
    /// from button actions.
    Touchpad,
    /// Moves the pointer but has no buttons (a stylus hovering while stylus
    /// events are off).
    Hover,
    /// A finger on a touchscreen.
    Touch,
}

impl PointerClass {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mouse => "mouse",
            Self::Touchpad => "touchpad",
            Self::Hover => "hover",
            Self::Touch => "touch",
        }
    }
}

/// A source is a bit set, so a device can report several at once: Samsung DeX
/// sends a desktop mouse as `SOURCE_MOUSE | SOURCE_TOUCHSCREEN` (0x3002). The
/// mouse bits or a mouse tool can therefore identify a mouse even on a
/// combined source.
///
/// A finger is a touchpad only on a touchpad source or on exactly
/// `SOURCE_MOUSE`, which is how Android reports a touchpad's pointer. A finger
/// on a combined source stays a touch.
pub const fn classify(source: u32, tool_type: u32) -> PointerClass {
    let finger = tool_type == TOOL_TYPE_FINGER;
    let mouse_bits = source & SOURCE_MOUSE == SOURCE_MOUSE;
    let touchpad_bits = source & SOURCE_TOUCHPAD == SOURCE_TOUCHPAD;
    if touchpad_bits || (finger && source == SOURCE_MOUSE) {
        PointerClass::Touchpad
    } else if finger {
        PointerClass::Touch
    } else if mouse_bits || tool_type == TOOL_TYPE_MOUSE {
        PointerClass::Mouse
    } else {
        PointerClass::Hover
    }
}

/// The guest receives one seat-wide press and release for each button, even
/// if several devices hold it. Each device can only release its own press.
pub struct PointerButtons<Device, Button> {
    held: HashMap<Button, HashSet<Device>>,
}

impl<Device, Button> Default for PointerButtons<Device, Button> {
    fn default() -> Self {
        Self { held: HashMap::new() }
    }
}

impl<Device: Copy + Eq + Hash, Button: Copy + Eq + Hash> PointerButtons<Device, Button> {
    pub fn has_press(&self, device: Device) -> bool {
        self.held.values().any(|devices| devices.contains(&device))
    }

    pub fn press(&mut self, device: Device, button: Button) -> bool {
        let devices = self.held.entry(button).or_default();
        let first = devices.is_empty();
        devices.insert(device);
        first
    }

    pub fn release(&mut self, device: Device, button: Button) -> bool {
        let Some(devices) = self.held.get_mut(&button) else { return false };
        if !devices.remove(&device) || !devices.is_empty() {
            return false;
        }
        self.held.remove(&button);
        true
    }

    pub fn release_device(&mut self, device: Device) -> Vec<Button> {
        let mut released = Vec::new();
        self.held.retain(|&button, devices| {
            devices.remove(&device);
            if devices.is_empty() {
                released.push(button);
                false
            } else {
                true
            }
        });
        released
    }

    pub fn drain(&mut self) -> impl Iterator<Item = (Device, Button)> + '_ {
        self.held.drain().filter_map(|(button, devices)| {
            devices.into_iter().next().map(|device| (device, button))
        })
    }
}
