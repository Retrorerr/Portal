//! Host-testable Android input device classification and KWin tablet mode policy.
//!
//! Under Portal's nested Wayland architecture:
//! Android InputDevice -> Portal / winit / Smithay -> Wayland -> nested KWin
//!
//! KWin's built-in automatic tablet mode detector relies on physical Linux `LibInput::Device`
//! hardware. In nested mode, KWin never receives physical libinput events, so setting
//! `[Input] TabletMode=auto` leaves Plasma permanently in desktop mode even when keyboard
//! and touchpad accessories are disconnected.
//!
//! Portal bridges this gap by listening to Android's authoritative `InputManager` device hotplug
//! events and dynamically writing `[Input] TabletMode = on | off` directly into the guest `kwinrc`.
//! KWin's `KConfigWatcher` detects file modifications, recalculates `effectiveTabletMode()`,
//! and triggers tablet mode adaptions in both KWin and Plasma.

/// Android `InputDevice.SOURCE_KEYBOARD`
pub const SOURCE_KEYBOARD: u32 = 0x00000101;
/// Android `InputDevice.SOURCE_TOUCHSCREEN`
pub const SOURCE_TOUCHSCREEN: u32 = 0x00001002;
/// Android `InputDevice.SOURCE_MOUSE`
pub const SOURCE_MOUSE: u32 = 0x00002002;
/// Android `InputDevice.SOURCE_TOUCHPAD`
pub const SOURCE_TOUCHPAD: u32 = 0x00100008;

/// Android `InputDevice.KEYBOARD_TYPE_NONE`
pub const KEYBOARD_TYPE_NONE: u32 = 0;
/// Android `InputDevice.KEYBOARD_TYPE_NON_ALPHABETIC`
pub const KEYBOARD_TYPE_NON_ALPHABETIC: u32 = 1;
/// Android `InputDevice.KEYBOARD_TYPE_ALPHABETIC`
pub const KEYBOARD_TYPE_ALPHABETIC: u32 = 2;

/// Descriptor of an Android input device's capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDeviceDescriptor {
    pub is_external: bool,
    pub is_virtual: bool,
    pub sources: u32,
    pub keyboard_type: u32,
}

impl InputDeviceDescriptor {
    pub fn new(is_external: bool, is_virtual: bool, sources: u32, keyboard_type: u32) -> Self {
        Self {
            is_external,
            is_virtual,
            sources,
            keyboard_type,
        }
    }
}

/// Check if an Android input device is an external physical alphabetic keyboard.
///
/// Only external alphabetic keyboards suppress the on-screen software keyboard (IME).
/// Built-in hardware keys (power, volume) or virtual devices do not suppress IME.
pub fn is_physical_alphabetic_keyboard(device: &InputDeviceDescriptor) -> bool {
    device.is_external
        && !device.is_virtual
        && (device.sources & SOURCE_KEYBOARD) == SOURCE_KEYBOARD
        && device.keyboard_type == KEYBOARD_TYPE_ALPHABETIC
}

/// Check if an Android input device is an external pointing device (mouse or touchpad).
///
/// An external pointing device keeps Plasma in desktop mode. Internal touchscreens
/// (`SOURCE_TOUCHSCREEN`, `!is_external`) are touch surfaces and must NEVER be treated
/// as desktop pointer input.
pub fn is_desktop_pointer(device: &InputDeviceDescriptor) -> bool {
    device.is_external
        && !device.is_virtual
        && ((device.sources & SOURCE_MOUSE) == SOURCE_MOUSE
            || (device.sources & SOURCE_TOUCHPAD) == SOURCE_TOUCHPAD)
}

/// Evaluated system input state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemInputState {
    /// An external physical alphabetic keyboard is attached.
    /// Controls software keyboard / IME suppression.
    pub physical_keyboard_present: bool,
    /// Either a physical keyboard or an external pointer/touchpad is attached.
    /// Controls KWin tablet mode: `tablet_mode = !desktop_input_present`.
    pub desktop_input_present: bool,
}

impl SystemInputState {
    pub fn evaluate<'a>(devices: impl IntoIterator<Item = &'a InputDeviceDescriptor>) -> Self {
        let mut physical_keyboard_present = false;
        let mut desktop_input_present = false;

        for device in devices {
            if is_physical_alphabetic_keyboard(device) {
                physical_keyboard_present = true;
                desktop_input_present = true;
            } else if is_desktop_pointer(device) {
                desktop_input_present = true;
            }
        }

        Self {
            physical_keyboard_present,
            desktop_input_present,
        }
    }

    /// KWin `TabletMode` value to write to `kwinrc`.
    ///
    /// When desktop input (keyboard case, mouse, or touchpad) is present, tablet mode is "off".
    /// When only touch input is available, tablet mode is "on".
    pub fn kwin_tablet_mode(&self) -> &'static str {
        if self.desktop_input_present {
            "off"
        } else {
            "on"
        }
    }

    /// Whether Android software keyboard (IME) should be suppressed.
    pub fn should_suppress_soft_keyboard(&self) -> bool {
        self.physical_keyboard_present
    }
}

/// Update or insert `TabletMode=<mode>` under `[Input]` in a `kwinrc` configuration string.
///
/// Ensures clean, idempotent updates without clobbering existing configuration entries.
pub fn update_kwinrc_tablet_mode(existing: &str, tablet_mode: &str) -> String {
    let mut lines: Vec<String> = existing.lines().map(|s| s.to_string()).collect();
    let mut in_input_group = false;
    let mut found_tablet_mode = false;
    let mut input_group_line_idx = None;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_input_group {
                // Leaving [Input] group without finding TabletMode
                break;
            }
            if trimmed == "[Input]" {
                in_input_group = true;
                input_group_line_idx = Some(i);
            }
        } else if in_input_group && trimmed.starts_with("TabletMode=") {
            lines[i] = format!("TabletMode={tablet_mode}");
            found_tablet_mode = true;
            break;
        }
    }

    if !found_tablet_mode {
        if let Some(idx) = input_group_line_idx {
            // [Input] group exists, insert right after [Input] header
            lines.insert(idx + 1, format!("TabletMode={tablet_mode}"));
        } else {
            // [Input] group does not exist, append it
            if !lines.is_empty() && !lines.last().map(|s| s.is_empty()).unwrap_or(false) {
                lines.push(String::new());
            }
            lines.push("[Input]".to_string());
            lines.push(format!("TabletMode={tablet_mode}"));
        }
    }

    let mut result = lines.join("\n");
    if existing.ends_with('\n') || result.ends_with('\n') {
        if !result.ends_with('\n') {
            result.push('\n');
        }
    } else {
        result.push('\n');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_keyboard_classified_correctly() {
        let kb = InputDeviceDescriptor::new(true, false, SOURCE_KEYBOARD, KEYBOARD_TYPE_ALPHABETIC);
        assert!(is_physical_alphabetic_keyboard(&kb));
        assert!(!is_desktop_pointer(&kb));

        let state = SystemInputState::evaluate([&kb]);
        assert!(state.physical_keyboard_present);
        assert!(state.desktop_input_present);
        assert_eq!(state.kwin_tablet_mode(), "off");
        assert!(state.should_suppress_soft_keyboard());
    }

    #[test]
    fn external_touchpad_classified_correctly() {
        let touchpad = InputDeviceDescriptor::new(true, false, SOURCE_TOUCHPAD, KEYBOARD_TYPE_NONE);
        assert!(!is_physical_alphabetic_keyboard(&touchpad));
        assert!(is_desktop_pointer(&touchpad));

        let state = SystemInputState::evaluate([&touchpad]);
        assert!(!state.physical_keyboard_present);
        assert!(state.desktop_input_present);
        assert_eq!(state.kwin_tablet_mode(), "off");
        assert!(!state.should_suppress_soft_keyboard());
    }

    #[test]
    fn external_mouse_classified_correctly() {
        let mouse = InputDeviceDescriptor::new(true, false, SOURCE_MOUSE, KEYBOARD_TYPE_NONE);
        assert!(!is_physical_alphabetic_keyboard(&mouse));
        assert!(is_desktop_pointer(&mouse));

        let state = SystemInputState::evaluate([&mouse]);
        assert!(!state.physical_keyboard_present);
        assert!(state.desktop_input_present);
        assert_eq!(state.kwin_tablet_mode(), "off");
        assert!(!state.should_suppress_soft_keyboard());
    }

    #[test]
    fn internal_touchscreen_alone_triggers_tablet_mode() {
        let touchpanel = InputDeviceDescriptor::new(false, false, SOURCE_TOUCHSCREEN, KEYBOARD_TYPE_NONE);
        assert!(!is_physical_alphabetic_keyboard(&touchpanel));
        assert!(!is_desktop_pointer(&touchpanel));

        let state = SystemInputState::evaluate([&touchpanel]);
        assert!(!state.physical_keyboard_present);
        assert!(!state.desktop_input_present);
        assert_eq!(state.kwin_tablet_mode(), "on");
        assert!(!state.should_suppress_soft_keyboard());
    }

    #[test]
    fn internal_power_and_volume_keys_do_not_block_tablet_mode() {
        let gpio_keys = InputDeviceDescriptor::new(false, false, SOURCE_KEYBOARD, KEYBOARD_TYPE_NON_ALPHABETIC);
        let pmic_keys = InputDeviceDescriptor::new(false, false, SOURCE_KEYBOARD, KEYBOARD_TYPE_NON_ALPHABETIC);
        assert!(!is_physical_alphabetic_keyboard(&gpio_keys));
        assert!(!is_desktop_pointer(&gpio_keys));

        let state = SystemInputState::evaluate([&gpio_keys, &pmic_keys]);
        assert!(!state.physical_keyboard_present);
        assert!(!state.desktop_input_present);
        assert_eq!(state.kwin_tablet_mode(), "on");
    }

    #[test]
    fn combined_oneplus_pad_keyboard_and_touchpad_case() {
        let kb = InputDeviceDescriptor::new(true, false, SOURCE_KEYBOARD, KEYBOARD_TYPE_ALPHABETIC);
        let tp = InputDeviceDescriptor::new(true, false, SOURCE_TOUCHPAD, KEYBOARD_TYPE_NONE);
        let touch = InputDeviceDescriptor::new(false, false, SOURCE_TOUCHSCREEN, KEYBOARD_TYPE_NONE);

        let attached_state = SystemInputState::evaluate([&kb, &tp, &touch]);
        assert!(attached_state.physical_keyboard_present);
        assert!(attached_state.desktop_input_present);
        assert_eq!(attached_state.kwin_tablet_mode(), "off");
        assert!(attached_state.should_suppress_soft_keyboard());

        // When detached, only internal touch remains
        let detached_state = SystemInputState::evaluate([&touch]);
        assert!(!detached_state.physical_keyboard_present);
        assert!(!detached_state.desktop_input_present);
        assert_eq!(detached_state.kwin_tablet_mode(), "on");
        assert!(!detached_state.should_suppress_soft_keyboard());
    }

    #[test]
    fn update_kwinrc_replaces_existing_tablet_mode() {
        let original = "[Desktops]\nNumber=1\n\n[Input]\nTabletMode=auto\n\n[Xwayland]\nScale=2\n";
        let updated = update_kwinrc_tablet_mode(original, "off");
        assert!(updated.contains("[Input]\nTabletMode=off"));
        assert!(!updated.contains("TabletMode=auto"));
        assert!(updated.contains("[Desktops]\nNumber=1"));
        assert!(updated.contains("[Xwayland]\nScale=2"));
    }

    #[test]
    fn update_kwinrc_adds_tablet_mode_to_existing_group() {
        let original = "[Desktops]\nNumber=1\n\n[Input]\nCursorTheme=Breeze\n\n[Xwayland]\nScale=2\n";
        let updated = update_kwinrc_tablet_mode(original, "on");
        assert!(updated.contains("[Input]\nTabletMode=on\nCursorTheme=Breeze"));
    }

    #[test]
    fn update_kwinrc_creates_input_group_if_missing() {
        let original = "[Desktops]\nNumber=1\n\n[Xwayland]\nScale=2\n";
        let updated = update_kwinrc_tablet_mode(original, "off");
        assert!(updated.contains("[Input]\nTabletMode=off"));
    }
}
