//! KWin tablet mode: the `[Input] TabletMode` entry Portal writes to the desktop user's `kwinrc`.
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
//! events and setting `[Input] TabletMode = on | off` in the desktop user's `kwinrc`. In a live
//! session the IME bridge applies it with `kwriteconfig6 --notify`: KWin's `KConfigWatcher`
//! acts on that D-Bus notification (not on file changes), recalculates `effectiveTabletMode()`,
//! and triggers tablet mode adaptions in both KWin and Plasma.

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
        let original =
            "[Desktops]\nNumber=1\n\n[Input]\nCursorTheme=Breeze\n\n[Xwayland]\nScale=2\n";
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
