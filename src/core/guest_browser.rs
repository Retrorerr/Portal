//! Browser defaults inside the guest: which browser is the default, and
//! Portal's Firefox preferences for every Firefox a user can install.
//!
//! The image ships Debian's `firefox-esr`, but `sudo apt install` can add
//! Mozilla's own `firefox` (from packages.mozilla.org, into
//! `/usr/lib/firefox`) or its beta, developer-edition and nightly channels
//! (`/usr/lib/firefox-beta`, `-devedition`, `-nightly`). All of them need the
//! same preferences, and the default browser must follow whichever is
//! installed, so nothing here names only `firefox-esr`.
//!
//! The preferences go through Firefox's autoconfig (`defaults/pref/autoconfig.js`
//! plus a `.cfg` next to the executable), which sets any preference; the
//! `policies.json` `Preferences` policy only allows fixed prefixes plus a short
//! list of `security.*` names (checked in Firefox ESR 140's `Policies.sys.mjs`).
//! Firefox's sandbox keeps its defaults: PRoot hands its seccomp traps to it and
//! emulates the namespaces it asks for.
//!
//! dpkg leaves files it does not own alone on upgrade and reinstall, so these
//! normally survive; a browser installed *after* Portal's last launch does
//! not have them yet. `portal-firefox-prefs`, run by dpkg after every
//! install/upgrade/removal, closes that gap without restarting Portal.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Firefox reads `general.config.filename` from here, so this file only
/// points at the real config.
pub const AUTOCONFIG_JS: &str = r#"pref("general.config.filename", "localdesktop.cfg");
pref("general.config.obscure_value", 0);
pref("general.config.sandbox_enabled", false);
"#;

pub const FIREFOX_CFG: &str = r#"// Auto updated by Portal on each startup, do not edit manually
// Project Anland GPU compositing (see src/android/anland/mod.rs): there is
// no DRM render node in PRoot, so Firefox's gfxInfo concludes SOFTWARE_GL
// and blocklists hardware compositing — even though real Adreno contexts
// work (proven: WebGL freedreno, glxtest EGL freedreno). These prefs force
// the GPU path back on; on native Wayland it composites through EGL window
// surfaces and needs no GBM allocation.
defaultPref("gfx.webrender.all", true);
defaultPref("layers.acceleration.force-enabled", true);
// Render at KWin's fractional scale (2.5 on the Pad 3) rather than the next
// integer (3) that KWin then scales down: fewer pixels, and sharp.
defaultPref("widget.wayland.fractional-scale.enabled", true);
// Portal already makes Firefox the default browser (/etc/xdg/mimeapps.list).
// Its own "make default" writes a userapp-*.desktop that the panel cannot
// match to the Firefox window (see repair_user_default_browser), so don't ask.
defaultPref("browser.shell.checkDefaultBrowser", false);
defaultPref("browser.shell.skipDefaultBrowserCheckOnFirstRun", true);

"#;

/// Directories under `usr/lib` prepared even before the browser is installed,
/// so the two browsers Portal knows always have their config.
const ALWAYS_PREPARED: [&str; 2] = ["firefox", "firefox-esr"];

/// Portal's copy of the two files, which the dpkg hook copies into each
/// browser directory it finds.
pub const PREFS_SHARE_DIR: &str = "usr/local/share/portal/firefox";
pub const PREFS_HOOK_SCRIPT_PATH: &str = "usr/local/bin/portal-firefox-prefs";
pub const PREFS_HOOK_SCRIPT: &str = include_str!("../../assets/portal-firefox-prefs.sh");
/// dpkg only reads `dpkg.cfg.d` entries named with letters, digits, `_` and `-`.
pub const DPKG_HOOK_CONF_PATH: &str = "etc/dpkg/dpkg.cfg.d/portal-firefox-prefs";
pub const DPKG_HOOK_CONF: &str =
    "# Managed by Portal: re-apply Portal's Firefox preferences after every dpkg run.\n\
post-invoke=/usr/local/bin/portal-firefox-prefs\n";

/// Every Firefox directory to configure: the two always-prepared ones plus any
/// other `usr/lib/firefox*` that holds a Mozilla browser (`application.ini`,
/// `libxul.so` or `firefox-bin`), which covers `firefox-beta`,
/// `firefox-devedition` and `firefox-nightly` without listing them.
pub fn firefox_install_dirs(fs_root: &Path) -> Vec<PathBuf> {
    let lib = fs_root.join("usr/lib");
    let mut dirs: Vec<PathBuf> = ALWAYS_PREPARED.iter().map(|name| lib.join(name)).collect();
    if let Ok(entries) = fs::read_dir(&lib) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let path = entry.path();
            if (name == "firefox" || name.starts_with("firefox-"))
                && !dirs.contains(&path)
                && is_mozilla_browser_dir(&path)
            {
                dirs.push(path);
            }
        }
    }
    dirs.sort();
    dirs
}

fn is_mozilla_browser_dir(dir: &Path) -> bool {
    dir.is_dir()
        && ["application.ini", "libxul.so", "firefox-bin"]
            .iter()
            .any(|marker| dir.join(marker).exists())
}

/// Whether a Firefox launcher exists in `usr/bin` (`firefox`, `firefox-esr`,
/// `firefox-nightly`, ...). A symlink counts: Mozilla's package links
/// `/usr/bin/firefox` to an absolute guest path, which does not resolve from
/// the host side.
pub fn firefox_installed(fs_root: &Path) -> bool {
    fs::read_dir(fs_root.join("usr/bin"))
        .map(|entries| {
            entries.flatten().any(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name == "firefox" || name.starts_with("firefox-")
            })
        })
        .unwrap_or(false)
}

/// Write `contents` to `path` unless it already holds them, through a
/// temporary file and a rename so a concurrent reader (a Firefox starting, or
/// dpkg) never sees a half-written file. Returns whether it wrote.
fn write_if_changed(path: &Path, contents: &str, mode: Option<u32>) -> io::Result<bool> {
    if fs::read_to_string(path).ok().as_deref() == Some(contents) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    // A leading dot keeps the temporary out of dpkg.cfg.d, which reads only
    // plain names.
    let temporary = path.with_file_name(format!(".{name}.portal-tmp"));
    fs::write(&temporary, contents)?;
    if let Some(mode) = mode {
        set_mode(&temporary, mode)?;
    }
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })?;
    // PRoot's fake-root sidecar outranks the host mode for exec, so a stale
    // record left by an earlier copy would override the mode set above.
    if mode.is_some() {
        if let Some(parent) = path.parent() {
            let _ = fs::remove_file(parent.join(format!(".proot-meta-file.{name}.meta")));
        }
    }
    Ok(true)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) -> io::Result<()> {
    Ok(())
}

/// Install Portal's Firefox autoconfig into every Firefox directory. Runs on
/// every launch, so a browser installed or replaced since is picked up.
pub fn sync_firefox_config(fs_root: &Path) {
    for dir in firefox_install_dirs(fs_root) {
        for (path, contents) in [
            (dir.join("defaults/pref/autoconfig.js"), AUTOCONFIG_JS),
            (dir.join("localdesktop.cfg"), FIREFOX_CFG),
        ] {
            if let Err(error) = write_if_changed(&path, contents, None) {
                log::warn!(
                    "Firefox config {} could not be written: {error}",
                    path.display()
                );
            }
        }
    }
}

/// Install the dpkg hook that re-applies the preferences after package
/// changes, and the copy of the files it distributes.
pub fn sync_firefox_prefs_hook(fs_root: &Path) {
    let share = fs_root.join(PREFS_SHARE_DIR);
    for (path, contents, mode) in [
        (share.join("autoconfig.js"), AUTOCONFIG_JS, None),
        (share.join("localdesktop.cfg"), FIREFOX_CFG, None),
        (
            fs_root.join(PREFS_HOOK_SCRIPT_PATH),
            PREFS_HOOK_SCRIPT,
            Some(0o755),
        ),
        (fs_root.join(DPKG_HOOK_CONF_PATH), DPKG_HOOK_CONF, None),
    ] {
        if let Err(error) = write_if_changed(&path, contents, mode) {
            log::warn!(
                "Firefox preference hook file {} could not be written: {error}",
                path.display()
            );
        }
    }
}

/// Firefox-family desktop entries, most preferred first: Mozilla's own build,
/// then Debian's ESR, then the other channels.
const BROWSER_DESKTOP_IDS: [&str; 5] = [
    "firefox.desktop",
    "firefox-esr.desktop",
    "firefox-beta.desktop",
    "firefox-devedition.desktop",
    "firefox-nightly.desktop",
];

/// `/etc/xdg/mimeapps.list` content. Every value is a list and an entry
/// naming an app that is not installed is skipped, so the browser lines
/// prefer Mozilla's `firefox` when it is installed and otherwise fall back
/// down the list.
pub fn default_applications() -> String {
    let browsers: String = BROWSER_DESKTOP_IDS
        .iter()
        .map(|id| format!("{id};"))
        .collect();
    let mut text = String::from(
        "# Managed by Portal: system-wide default applications.\n\
# Override per user in ~/.config/mimeapps.list (System Settings > Default Applications).\n\
[Default Applications]\n",
    );
    for mime in [
        "x-scheme-handler/http",
        "x-scheme-handler/https",
        "text/html",
        "application/xhtml+xml",
    ] {
        text.push_str(&format!("{mime}={browsers}\n"));
    }
    text.push_str(
        "x-scheme-handler/mailto=thunderbird.desktop;\n\
text/csv=libreoffice-calc.desktop;\n\
text/tab-separated-values=libreoffice-calc.desktop;\n\
application/vnd.ms-excel=libreoffice-calc.desktop;\n\
application/vnd.ms-excel.sheet.macroEnabled.12=libreoffice-calc.desktop;\n\
application/vnd.openxmlformats-officedocument.spreadsheetml.sheet=libreoffice-calc.desktop;\n\
application/vnd.openxmlformats-officedocument.wordprocessingml.document=libreoffice-writer.desktop;\n\
application/vnd.openxmlformats-officedocument.presentationml.presentation=libreoffice-impress.desktop;\n",
    );
    text
}

/// System-wide default applications (`/etc/xdg/mimeapps.list`; users'
/// `~/.config/mimeapps.list` still wins).
///
/// Without explicit defaults, the first desktop entry in `mimeinfo.cache`
/// wins. The optional ChatGPT app claims http/https and Office/CSV types and
/// sorts before Firefox and LibreOffice, so it became the default browser:
/// Plasma's `preferred://browser` panel launcher then showed a second ChatGPT
/// icon instead of Firefox. Entries naming an app that is not installed are
/// ignored by the spec, so the same defaults hold for every selection.
///
/// A file that Portal did not write (no `# Managed by Portal` header) is
/// never touched.
pub fn sync_default_applications(fs_root: &Path) {
    let defaults = default_applications();
    let path = fs_root.join("etc/xdg/mimeapps.list");
    let current = fs::read_to_string(&path).ok();
    if current.as_deref() == Some(defaults.as_str()) {
        return;
    }
    if current.is_some_and(|text| !text.starts_with("# Managed by Portal")) {
        log::warn!(
            "{} is not Portal-managed; leaving it untouched",
            path.display()
        );
        return;
    }
    if let Err(error) = path
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::write(&path, defaults))
    {
        log::warn!("default applications could not be installed: {error}");
    }
}

/// The packaged desktop entry for a Firefox that set itself as the default
/// browser, from the `userapp-*.desktop` entry GIO wrote for it: `Exec=` names
/// the binary under `/usr/lib/firefox*/`, and that directory's name is the
/// package's desktop id (`firefox`, `firefox-esr`, `firefox-beta`, ...).
fn packaged_entry_for_userapp(fs_root: &Path, userapp: &str) -> Option<String> {
    let exec = userapp.lines().find_map(|line| line.strip_prefix("Exec="))?;
    let program = exec.split_whitespace().next()?;
    let dir = program.strip_prefix("/usr/lib/")?.split('/').next()?;
    if !dir.starts_with("firefox") {
        return None;
    }
    let id = format!("{dir}.desktop");
    fs_root
        .join("usr/share/applications")
        .join(&id)
        .is_file()
        .then_some(id)
}

/// Point the desktop user's default browser back at Firefox's packaged entry.
///
/// Firefox's "Make default" (its first-run prompt, the welcome page and
/// Settings) does not use `firefox.desktop`: it has GIO create a
/// `userapp-Firefox-XXXXXX.desktop` with no icon and no `StartupWMClass` and
/// maps the browser types to it in `~/.config/mimeapps.list`. Plasma's
/// `preferred://browser` panel launcher then shows a blank icon, and the
/// Firefox window (app id `firefox`) does not match that launcher, so it gets
/// a second task button as if it were another app. Runs on every launch;
/// other user choices in the file are left alone.
pub fn repair_user_default_browser(fs_root: &Path) {
    let applications = fs_root.join("home/desktop/.local/share/applications");
    let Ok(entries) = fs::read_dir(&applications) else {
        return;
    };
    let replacements: Vec<(String, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if !name.starts_with("userapp-") || !name.ends_with(".desktop") {
                return None;
            }
            let text = fs::read_to_string(entry.path()).ok()?;
            Some((name, packaged_entry_for_userapp(fs_root, &text)?))
        })
        .collect();
    if replacements.is_empty() {
        return;
    }
    let path = fs_root.join("home/desktop/.config/mimeapps.list");
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    let updated = replace_default_entries(&text, &replacements);
    if updated != text {
        if let Err(error) = fs::write(&path, updated) {
            log::warn!("{} could not be updated: {error}", path.display());
        }
    }
}

/// Swap desktop ids in a `mimeapps.list`, dropping duplicates a swap creates
/// within one value list.
pub fn replace_default_entries(text: &str, replacements: &[(String, String)]) -> String {
    text.split_inclusive('\n')
        .map(|line| {
            let body = line.trim_end_matches(['\r', '\n']);
            let eol = &line[body.len()..];
            let Some((key, value)) = body.split_once('=') else {
                return line.to_owned();
            };
            if key.trim_start().starts_with('#') {
                return line.to_owned();
            }
            let mut ids: Vec<&str> = Vec::new();
            let mut changed = false;
            for id in value.split(';').filter(|id| !id.is_empty()) {
                let swapped = replacements
                    .iter()
                    .find(|(from, _)| from == id)
                    .map_or(id, |(_, to)| to.as_str());
                changed |= swapped != id || ids.contains(&swapped);
                if !ids.contains(&swapped) {
                    ids.push(swapped);
                }
            }
            if !changed {
                return line.to_owned();
            }
            format!("{key}={};{eol}", ids.join(";"))
        })
        .collect()
}
