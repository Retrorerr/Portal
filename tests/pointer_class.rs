//! How Android pointer events are sorted into mouse, touchpad and touch.

#[path = "../patches/winit/src/platform_impl/android/pointer_class.rs"]
#[allow(dead_code)]
mod pointer_class;

use pointer_class::{classify, PointerButtons, PointerClass};

const TOUCHSCREEN: u32 = 0x1002;
const MOUSE: u32 = 0x2002;
const STYLUS: u32 = 0x4002;
const TOUCHPAD: u32 = 0x0010_0008;
/// Samsung DeX reports a desktop mouse as mouse and touchscreen at once.
const DEX_MOUSE: u32 = MOUSE | TOUCHSCREEN;

const UNKNOWN: u32 = 0;
const FINGER: u32 = 1;
const STYLUS_TOOL: u32 = 2;
const MOUSE_TOOL: u32 = 3;

#[test]
fn plain_mouse_is_a_mouse() {
    assert_eq!(classify(MOUSE, MOUSE_TOOL), PointerClass::Mouse);
    assert_eq!(classify(MOUSE, UNKNOWN), PointerClass::Mouse);
}

#[test]
fn combined_mouse_source_is_a_mouse() {
    assert_eq!(classify(DEX_MOUSE, MOUSE_TOOL), PointerClass::Mouse);
    assert_eq!(classify(DEX_MOUSE, UNKNOWN), PointerClass::Mouse);
}

#[test]
fn mouse_tool_is_a_mouse_on_any_source() {
    assert_eq!(classify(TOUCHSCREEN, MOUSE_TOOL), PointerClass::Mouse);
    assert_eq!(classify(0, MOUSE_TOOL), PointerClass::Mouse);
}

#[test]
fn finger_on_a_mouse_source_is_a_touchpad() {
    assert_eq!(classify(MOUSE, FINGER), PointerClass::Touchpad);
}

#[test]
fn touchpad_source_is_a_touchpad_for_any_tool() {
    assert_eq!(classify(TOUCHPAD, FINGER), PointerClass::Touchpad);
    assert_eq!(classify(TOUCHPAD, MOUSE_TOOL), PointerClass::Touchpad);
    assert_eq!(classify(TOUCHPAD | MOUSE, FINGER), PointerClass::Touchpad);
}

#[test]
fn finger_elsewhere_is_a_touch() {
    assert_eq!(classify(TOUCHSCREEN, FINGER), PointerClass::Touch);
    assert_eq!(classify(DEX_MOUSE, FINGER), PointerClass::Touch);
    assert_eq!(classify(0, FINGER), PointerClass::Touch);
}

#[test]
fn stylus_without_mouse_bits_only_hovers() {
    assert_eq!(classify(STYLUS, STYLUS_TOOL), PointerClass::Hover);
    assert_eq!(classify(TOUCHSCREEN, STYLUS_TOOL), PointerClass::Hover);
}

#[test]
fn releasing_one_device_keeps_the_other_devices_buttons_held() {
    let mut buttons = PointerButtons::default();
    assert!(buttons.press(1, "left"));
    assert!(!buttons.press(1, "left"));
    assert!(buttons.press(2, "right"));
    assert!(!buttons.release(2, "left"));
    assert!(buttons.has_press(1));
    assert_eq!(buttons.release_device(2), ["right"]);
    assert!(buttons.release(1, "left"));
    assert!(!buttons.has_press(1));
    assert!(!buttons.release(1, "left"));
}

#[test]
fn shared_button_is_released_only_when_the_last_device_lets_go() {
    let mut buttons = PointerButtons::default();
    assert!(buttons.press(1, "left"));
    assert!(!buttons.press(2, "left"));
    assert!(buttons.release_device(1).is_empty());
    assert!(buttons.has_press(2));
    assert_eq!(buttons.drain().collect::<Vec<_>>(), [(2, "left")]);
    assert!(!buttons.release(2, "left"));
}
