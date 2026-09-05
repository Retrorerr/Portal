//! Dynamic KWin tablet mode manager.
//!
//! Controls `[Input] TabletMode = on | off` in `kwinrc` to match the presence of external
//! keyboard/pointer hardware on Android.

use std::path::PathBuf;
use crate::core::runtime::LinuxRuntime;
use crate::core::tablet_mode::update_kwinrc_tablet_mode;

pub fn get_kwinrc_path() -> PathBuf {
    let runtime = crate::android::runtime::proot::PRootRuntime::active();
    runtime.rootfs_path().join("root/.config/kwinrc")
}

pub fn apply_kwin_tablet_mode(has_desktop_input: bool) {
    let mode = if has_desktop_input { "off" } else { "on" };
    let runtime = crate::android::runtime::proot::PRootRuntime::active();
    let kwinrc_path = runtime.rootfs_path().join("root/.config/kwinrc");

    log::info!(
        "Updating KWin tablet mode: has_desktop_input={has_desktop_input} -> TabletMode={mode} at {:?}",
        kwinrc_path
    );

    let fifo_path = runtime.rootfs_path().join("tmp/portal-session-cmd.fifo");
    if fifo_path.exists() {
        // In a live Plasma session, kwriteconfig6 writes to kwinrc AND emits the D-Bus
        // notification. Writing directly to disk beforehand would clear the dirty flag,
        // preventing kwriteconfig6 from notifying KWin's KConfigWatcher.
        std::thread::spawn(move || {
            use std::io::Write;
            log::info!("Dispatching TabletMode {mode} to session FIFO at {:?}", fifo_path);
            match std::fs::OpenOptions::new().read(true).write(true).open(&fifo_path) {
                Ok(mut file) => {
                    let cmd = format!("kwriteconfig6 --file kwinrc --group Input --key TabletMode {mode} --notify\n");
                    if let Err(e) = file.write_all(cmd.as_bytes()) {
                        log::warn!("Failed to write command to session FIFO: {e}");
                    } else {
                        let _ = file.flush();
                        log::info!("Successfully dispatched TabletMode {mode} to session FIFO");
                    }
                }
                Err(e) => {
                    log::warn!("Failed to open session FIFO at {:?}: {e}; falling back to direct disk write", fifo_path);
                    let existing = std::fs::read_to_string(&kwinrc_path).unwrap_or_default();
                    let updated = update_kwinrc_tablet_mode(&existing, mode);
                    let _ = std::fs::write(&kwinrc_path, updated);
                }
            }
        });
    } else {
        // Plasma session is not active yet; persist setting directly to disk so KWin reads
        // it on initial startup.
        let existing = match std::fs::read_to_string(&kwinrc_path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("Could not read kwinrc at {:?}: {e}", kwinrc_path);
                String::new()
            }
        };

        let updated = update_kwinrc_tablet_mode(&existing, mode);
        if existing != updated {
            if let Some(parent) = kwinrc_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(&kwinrc_path, &updated) {
                log::error!("Failed to write updated kwinrc at {:?}: {e}", kwinrc_path);
            } else {
                log::info!("Successfully wrote initial TabletMode={mode} to kwinrc");
            }
        } else {
            log::debug!("kwinrc TabletMode already matches '{mode}' on disk");
        }
    }
}
