//! Host-testable Android input translation policy.

/// Translate Android's stable `KeyEvent.KEYCODE_*` values to Linux evdev scan codes.
///
/// Winit normally gives the compositor a physical key code, but Android's accessibility bridge
/// can report a zero scan code for some Bluetooth and virtual keyboards. Falling back to the
/// Android key code keeps those keyboards usable without manufacturing the invalid evdev code 0.
pub fn android_keycode_to_scancode(key_code: u32) -> Option<u32> {
    let scan_code = match key_code {
        4 => 1,                     // KEYCODE_BACK -> Escape
        7 => 11,                    // 0
        8 => 2,                     // 1
        9 => 3,                     // 2
        10 => 4,                    // 3
        11 => 5,                    // 4
        12 => 6,                    // 5
        13 => 7,                    // 6
        14 => 8,                    // 7
        15 => 9,                    // 8
        16 => 10,                   // 9
        19 => 103,                  // DPAD_UP
        20 => 108,                  // DPAD_DOWN
        21 => 105,                  // DPAD_LEFT
        22 => 106,                  // DPAD_RIGHT
        23 => 28,                   // DPAD_CENTER -> Enter
        29 => 30,                   // A
        30 => 48,                   // B
        31 => 46,                   // C
        32 => 32,                   // D
        33 => 18,                   // E
        34 => 33,                   // F
        35 => 34,                   // G
        36 => 35,                   // H
        37 => 23,                   // I
        38 => 36,                   // J
        39 => 37,                   // K
        40 => 38,                   // L
        41 => 50,                   // M
        42 => 49,                   // N
        43 => 24,                   // O
        44 => 25,                   // P
        45 => 16,                   // Q
        46 => 19,                   // R
        47 => 31,                   // S
        48 => 20,                   // T
        49 => 22,                   // U
        50 => 47,                   // V
        51 => 17,                   // W
        52 => 45,                   // X
        53 => 21,                   // Y
        54 => 44,                   // Z
        55 => 51,                   // COMMA
        56 => 52,                   // PERIOD
        57 => 56,                   // ALT_LEFT
        58 => 100,                  // ALT_RIGHT
        59 => 42,                   // SHIFT_LEFT
        60 => 54,                   // SHIFT_RIGHT
        61 => 15,                   // TAB
        62 => 57,                   // SPACE
        66 => 28,                   // ENTER
        67 => 14,                   // DEL
        68 => 41,                   // GRAVE
        69 => 12,                   // MINUS
        70 => 13,                   // EQUALS
        71 => 26,                   // LEFT_BRACKET
        72 => 27,                   // RIGHT_BRACKET
        73 => 43,                   // BACKSLASH
        74 => 39,                   // SEMICOLON
        75 => 40,                   // APOSTROPHE
        76 => 53,                   // SLASH
        92 => 104,                  // PAGE_UP
        93 => 109,                  // PAGE_DOWN
        111 => 1,                   // ESCAPE
        112 => 111,                 // FORWARD_DEL
        113 => 29,                  // CTRL_LEFT
        114 => 97,                  // CTRL_RIGHT
        115 => 58,                  // CAPS_LOCK
        117 => 125,                 // META_LEFT
        118 => 126,                 // META_RIGHT
        122 => 102,                 // MOVE_HOME
        123 => 107,                 // MOVE_END
        124 => 110,                 // INSERT
        131..=142 => key_code - 72, // F1..F12 -> 59..70
        _ => return None,
    };
    Some(scan_code)
}

/// Convert the keyboard-friendly subset of committed IME text to evdev key events.
///
/// A Wayland compositor receives key events rather than Unicode strings. This helper handles
/// printable ASCII and leaves non-ASCII text for a text-input-v3/virtual-keyboard path instead
/// of guessing a keyboard layout. Each tuple is `(scan_code, shift_required)`.
pub fn committed_ascii_to_key_events(text: &str) -> Vec<(u32, bool)> {
    text.chars()
        .filter_map(|ch| {
            let (scan_code, shift_required) = match ch {
                'a' => (30, false),
                'b' => (48, false),
                'c' => (46, false),
                'd' => (32, false),
                'e' => (18, false),
                'f' => (33, false),
                'g' => (34, false),
                'h' => (35, false),
                'i' => (23, false),
                'j' => (36, false),
                'k' => (37, false),
                'l' => (38, false),
                'm' => (50, false),
                'n' => (49, false),
                'o' => (24, false),
                'p' => (25, false),
                'q' => (16, false),
                'r' => (19, false),
                's' => (31, false),
                't' => (20, false),
                'u' => (22, false),
                'v' => (47, false),
                'w' => (17, false),
                'x' => (45, false),
                'y' => (21, false),
                'z' => (44, false),
                'A' => (30, true),
                'B' => (48, true),
                'C' => (46, true),
                'D' => (32, true),
                'E' => (18, true),
                'F' => (33, true),
                'G' => (34, true),
                'H' => (35, true),
                'I' => (23, true),
                'J' => (36, true),
                'K' => (37, true),
                'L' => (38, true),
                'M' => (50, true),
                'N' => (49, true),
                'O' => (24, true),
                'P' => (25, true),
                'Q' => (16, true),
                'R' => (19, true),
                'S' => (31, true),
                'T' => (20, true),
                'U' => (22, true),
                'V' => (47, true),
                'W' => (17, true),
                'X' => (45, true),
                'Y' => (21, true),
                'Z' => (44, true),
                '1'..='9' => (2 + (ch as u32 - '1' as u32), false),
                '0' => (11, false),
                '!' => (2, true),
                '@' => (3, true),
                '#' => (4, true),
                '$' => (5, true),
                '%' => (6, true),
                '^' => (7, true),
                '&' => (8, true),
                '*' => (9, true),
                '(' => (10, true),
                ')' => (11, true),
                '-' => (12, false),
                '_' => (12, true),
                '=' => (13, false),
                '+' => (13, true),
                '[' => (26, false),
                '{' => (26, true),
                ']' => (27, false),
                '}' => (27, true),
                '\\' => (43, false),
                '|' => (43, true),
                ';' => (39, false),
                ':' => (39, true),
                '\'' => (40, false),
                '"' => (40, true),
                '`' => (41, false),
                '~' => (41, true),
                ',' => (51, false),
                '<' => (51, true),
                '.' => (52, false),
                '>' => (52, true),
                '/' => (53, false),
                '?' => (53, true),
                ' ' => (57, false),
                '\u{8}' => (14, false),
                '\n' | '\r' => (28, false),
                _ => return None,
            };
            Some((scan_code, shift_required))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_zero_scan_code_android_keys_to_evdev() {
        assert_eq!(android_keycode_to_scancode(29), Some(30));
        assert_eq!(android_keycode_to_scancode(67), Some(14));
        assert_eq!(android_keycode_to_scancode(131), Some(59));
        assert_eq!(android_keycode_to_scancode(999), None);
    }

    #[test]
    fn converts_ascii_ime_commits_with_shift_information() {
        assert_eq!(
            committed_ascii_to_key_events("aA 1!"),
            vec![(30, false), (30, true), (57, false), (2, false), (2, true)]
        );
        assert!(committed_ascii_to_key_events("é").is_empty());
    }
}
