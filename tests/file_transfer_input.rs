#[path = "../src/android/anland/protocol.rs"]
#[allow(dead_code)]
mod protocol;

use protocol::*;

#[test]
fn multitouch_cleanup_tracks_each_finger_without_treating_motion_as_a_press() {
    for id in [0, 1, 17, 250] {
        assert_eq!(
            InputEvent::touch(INPUT_ACTION_DOWN, 25.0, 50.0, id).press_edge(),
            Some((id, true))
        );
        assert_eq!(
            InputEvent::touch(INPUT_ACTION_MOVE, 25.0, 100.0, id).press_edge(),
            None
        );
        assert_eq!(
            InputEvent::touch(INPUT_ACTION_UP, 0.0, 0.0, id).press_edge(),
            Some((id, false))
        );
    }
    assert_eq!(InputEvent::touch_frame().press_edge(), None);
    assert_eq!(
        InputEvent::key(INPUT_ACTION_DOWN, 29).press_edge(),
        Some((29, true))
    );
    assert_eq!(
        InputEvent::pointer_button(0x110, false).press_edge(),
        Some((0x110, false))
    );
}
