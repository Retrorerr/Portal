//! Hardware video decoding defaults for guest media players.
//!
//! Portal decodes video in hardware through VA-API (see
//! `android::backend::video_decode`). Firefox, Chromium, VLC, Haruna and
//! GStreamer's VA plugin use it by default; mpv does not: its `hwdec`
//! default is `no`, so mpv and the libmpv players that load its config
//! decode 4K in software. A system `mpv.conf` selects `auto-safe`, mpv's
//! list of decoders that fall back to software cleanly (when decoding is
//! turned off, libva finds no driver and mpv decodes in software).
//! Celluloid passes its own option string to libmpv and never reads
//! `mpv.conf` unless asked, so its default comes from a GSettings override,
//! which takes effect the next time the schemas are compiled (dpkg does that
//! when Celluloid is installed).
//!
//! Debian's mpv ships no `/etc/mpv/mpv.conf`; a file there without Portal's
//! first line is the user's and is left alone.

use std::fs;
use std::io;
use std::path::Path;

const MPV_CONF_HEADER: &str =
    "# Portal: hardware video decoding (VA-API). Remove this line to keep your own edits.";

/// `/etc/mpv/mpv.conf` as Portal writes it.
pub fn mpv_conf() -> String {
    format!("{MPV_CONF_HEADER}\nhwdec=auto-safe\n")
}

/// GSettings override giving Celluloid the same default.
pub const CELLULOID_OVERRIDE: &str =
    "[io.github.celluloid-player.Celluloid]\nmpv-options='--hwdec=auto-safe'\n";

const MPV_CONF_PATH: &str = "etc/mpv/mpv.conf";
const CELLULOID_OVERRIDE_PATH: &str = "usr/share/glib-2.0/schemas/90_portal-media.gschema.override";

fn write_if_changed(path: &Path, contents: &str) -> io::Result<()> {
    if fs::read_to_string(path).ok().as_deref() == Some(contents) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("portal-tmp");
    fs::write(&temporary, contents)?;
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

/// Install the player defaults. Runs on every launch; a `mpv.conf` the user
/// wrote (or took over by removing the header) is never touched.
pub fn sync_player_defaults(fs_root: &Path) {
    let mpv = fs_root.join(MPV_CONF_PATH);
    let owned = match fs::read_to_string(&mpv) {
        Ok(text) => text.lines().next() == Some(MPV_CONF_HEADER),
        Err(error) => error.kind() == io::ErrorKind::NotFound,
    };
    let mut writes = vec![(
        fs_root.join(CELLULOID_OVERRIDE_PATH),
        CELLULOID_OVERRIDE.to_string(),
    )];
    if owned {
        writes.push((mpv, mpv_conf()));
    }
    for (path, contents) in writes {
        if let Err(error) = write_if_changed(&path, &contents) {
            log::warn!("player defaults {} not written: {error}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_both_defaults_on_a_fresh_root() {
        let root = tempfile::tempdir().unwrap();
        sync_player_defaults(root.path());
        let conf = fs::read_to_string(root.path().join(MPV_CONF_PATH)).unwrap();
        assert!(conf.lines().any(|line| line == "hwdec=auto-safe"));
        assert_eq!(
            fs::read_to_string(root.path().join(CELLULOID_OVERRIDE_PATH)).unwrap(),
            CELLULOID_OVERRIDE
        );
    }

    #[test]
    fn a_users_mpv_conf_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(MPV_CONF_PATH);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "hwdec=no\n").unwrap();
        sync_player_defaults(root.path());
        assert_eq!(fs::read_to_string(&path).unwrap(), "hwdec=no\n");
    }

    #[test]
    fn portals_own_mpv_conf_is_refreshed() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(MPV_CONF_PATH);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, format!("{MPV_CONF_HEADER}\nhwdec=vaapi\n")).unwrap();
        sync_player_defaults(root.path());
        assert_eq!(fs::read_to_string(&path).unwrap(), mpv_conf());
    }
}
