//! Dynamic KWin tablet mode manager.
//!
//! Controls `[Input] TabletMode = on | off` in the desktop user's `kwinrc` to match the presence
//! of external keyboard/pointer hardware on Android.

use crate::core::config::DESKTOP_USER;
use crate::core::runtime::LinuxRuntime;
use crate::core::tablet_mode::update_kwinrc_tablet_mode;

pub fn apply_kwin_tablet_mode(has_desktop_input: bool) {
    let mode = if has_desktop_input { "off" } else { "on" };

    if crate::android::proot::launch::is_running() {
        // A live session: the IME bridge runs inside it as KWin's input method and applies
        // the mode with kwriteconfig6 --notify, which writes kwinrc and tells KWin and Plasma
        // to re-read it. Writing the file here instead would make kwriteconfig6 see no change
        // and skip the notification. A bridge that is still starting reads the queued command.
        if crate::android::ime::send_ime_command(&format!("TABLET_MODE:{mode}\n")) {
            log::info!("Tablet mode {mode} sent to the Plasma session (has_desktop_input={has_desktop_input})");
            return;
        }
        log::warn!("Tablet mode {mode}: session command channel unavailable; updating kwinrc for the next start");
    }

    // No session: KWin reads the file when it starts.
    let runtime = crate::android::runtime::proot::PRootRuntime::active();
    let rootfs = runtime.rootfs_path();
    // Before setup accepts a plan the runtime must stay absent: a stray home/
    // directory makes the next launch treat a clean first run as a partial
    // install and refuse to start setup. Setup writes the initial kwinrc itself.
    if !rootfs.exists() {
        return;
    }
    let kwinrc_path = rootfs
        .join("home")
        .join(DESKTOP_USER)
        .join(".config/kwinrc");
    let existing = std::fs::read_to_string(&kwinrc_path).unwrap_or_default();
    let updated = update_kwinrc_tablet_mode(&existing, mode);
    if existing == updated {
        return;
    }
    if let Some(parent) = kwinrc_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&kwinrc_path, &updated) {
        Ok(()) => log::info!("Wrote TabletMode={mode} to {}", kwinrc_path.display()),
        Err(error) => log::error!("Failed to write {}: {error}", kwinrc_path.display()),
    }
}
