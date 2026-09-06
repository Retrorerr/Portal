use super::process::ArchProcess;
use crate::{
    android::{
        app::build::PolarBearBackend,
        backend::{
            wayland::{Compositor, TouchMode, WaylandBackend},
            webview::{ErrorVariant, WebviewBackend},
        },
        diagnostics,
        utils::application_context::get_application_context,
        utils::ndk::{active_refresh_millihz, long_press_timeout_ms, scale_factor, touch_slop_px},
    },
    core::config::{DOCS_HOME_URL, PRODUCTION_FS_ROOT},
};
use pathdiff::diff_paths;
use smithay::utils::Clock;
use std::{
    fs,
    io::ErrorKind,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    process,
    sync::{
        mpsc::{self, Sender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};

use winit::platform::android::activity::AndroidApp;

#[derive(Debug)]
pub enum SetupMessage {
    Progress(String),
    Error(String),
}

pub struct SetupOptions {
    pub android_app: AndroidApp,
    pub mpsc_sender: Sender<SetupMessage>,
}

/// Completion hook used by the lifecycle owner to dismiss the provisioning
/// WebView and send an event through its event-loop proxy. Keeping this hook
/// outside `WebviewBackend` avoids recreating the NativeActivity just to swap
/// to the Wayland backend.
pub type SetupCompletionCallback = Arc<dyn Fn() + Send + Sync + 'static>;

const KWIN_WRAPPER: &str = include_str!("../../../assets/localdesktop-kwin-wrapper-v2.sh");
const PLASMA_LAUNCHER: &str = include_str!("../../../assets/localdesktop-startplasma.sh");
const RECOVERY_LAUNCHER: &str = include_str!("../../../assets/localdesktop-recovery.sh");
const RETRY_PLASMA: &str = include_str!("../../../assets/localdesktop-retry-plasma.sh");
const KONSOLE_CONFIG: &str = include_str!("../../../assets/konsole/konsolerc");
const KONSOLE_PROFILE: &str = include_str!("../../../assets/konsole/LocalDesktop.profile");
const CRASH_HANDLER_BINARY: &[u8] =
    include_bytes!("../../../assets/guest-arm64/localdesktop-crash-handler.so");
const CRASH_HANDLER_SOURCE: &str = include_str!("../../../assets/localdesktop-crash-handler.c");
const PORTAL_IME_BRIDGE: &str = include_str!("../../../assets/portal-ime-bridge.py");
const PORTAL_IME_DESKTOP: &str = include_str!("../../../assets/portal-ime.desktop");
const CLIPBOARD_SYNC: &str = include_str!("../../../assets/localdesktop-clipboard-sync.sh");
const CLIPBOARD_PUSH: &str = include_str!("../../../assets/localdesktop-clipboard-push.sh");
const WL_COPY_BINARY: &[u8] = include_bytes!("../../../assets/guest-arm64/wl-copy");
const WL_PASTE_BINARY: &[u8] = include_bytes!("../../../assets/guest-arm64/wl-paste");

/// Setup is a process that should be done **only once** when the user installed the app.
/// The setup process consists of several stages.
/// Each stage is a function that takes the `SetupOptions` and returns a `StageOutput`.
type SetupStage = Box<dyn Fn(&SetupOptions) -> StageOutput + Send>;
type NamedSetupStage = (&'static str, SetupStage);

/// Each stage should indicate whether the associated task is done previously or not.
/// Thus, it should return a finished status if the task is done, so that the setup process can move on to the next stage.
/// Otherwise, it should return a `JoinHandle`, so that the setup process can wait for the task to finish, but not block the main thread so that the setup progress can be reported to the user.
///
/// For coding agents: READ THIS BEFORE ADDING WORK HERE.
/// - Heavy/long work belongs inside the spawned thread of a returned `Some(JoinHandle)`, so it runs once at install and surfaces as setup progress.
/// - Simple/light tasks or important settings that must be run every launch (e.g. the Firefox config) can be done inline on the `None` path.
type StageOutput = Option<JoinHandle<()>>;

fn setup_debian_runtime(options: &SetupOptions) -> StageOutput {
    let artifact = crate::core::provisioning::RuntimeArtifact::production();
    if artifact.is_ready(Path::new(PRODUCTION_FS_ROOT)) {
        return None;
    }
    let sender = options.mpsc_sender.clone();
    let base = get_application_context().data_dir.clone();
    Some(thread::spawn(move || {
        artifact
            .provision(&base, |message| {
                diagnostics::host_event("runtime-provisioning", &message);
                let _ = sender.send(SetupMessage::Progress(message));
            })
            .unwrap_or_else(|error| panic!("Debian provisioning failed: {error:#}"));
    }))
}

fn simulate_linux_sysdata_stage(options: &SetupOptions) -> StageOutput {
    let fs_root = Path::new(PRODUCTION_FS_ROOT);
    let mpsc_sender = options.mpsc_sender.clone();

    if !fs_root.join("proc/.version").exists() {
        return Some(thread::spawn(move || {
            mpsc_sender
                .send(SetupMessage::Progress(
                    "Simulating Linux system data...".to_string(),
                ))
                .expect(&format!("Failed to send log message"));

            // Create necessary directories - don't fail if they already exist
            let _ = fs::create_dir_all(fs_root.join("proc"));
            let _ = fs::create_dir_all(fs_root.join("sys"));
            let _ = fs::create_dir_all(fs_root.join("sys/.empty"));

            // Set permissions - only try to set permissions if we're on Unix and have the capability
            #[cfg(unix)]
            {
                // Try to set permissions, but don't fail if we can't
                let _ =
                    fs::set_permissions(fs_root.join("proc"), fs::Permissions::from_mode(0o700));
                let _ = fs::set_permissions(fs_root.join("sys"), fs::Permissions::from_mode(0o700));
                let _ = fs::set_permissions(
                    fs_root.join("sys/.empty"),
                    fs::Permissions::from_mode(0o700),
                );
            }

            // Create fake proc files
            let proc_files = [
                ("proc/.sysctl_entry_cap_last_cap", "40\n"),
                ("proc/.sysctl_inotify_max_user_watches", "4096\n"),
            ];

            for (path, content) in proc_files {
                let _ = fs::write(fs_root.join(path), content)
                    .expect(&format!("Permission denied while writing to {}", path));
            }
        }));
    }
    None
}

fn setup_machine_id(_: &SetupOptions) -> StageOutput {
    let fs_root = Path::new(PRODUCTION_FS_ROOT);
    let machine_id = fs_root.join("etc/machine-id");

    let existing = fs::read_to_string(&machine_id).unwrap_or_default();
    if !is_valid_machine_id(&existing) {
        if let Some(parent) = machine_id.parent() {
            fs::create_dir_all(parent).expect("Failed to create /etc for machine-id");
        }

        let _ = fs::set_permissions(&machine_id, fs::Permissions::from_mode(0o644));
        fs::write(&machine_id, format!("{}\n", generate_machine_id()))
            .expect("Failed to write machine-id");
        let _ = fs::set_permissions(&machine_id, fs::Permissions::from_mode(0o444));
        log::info!("Seeded guest /etc/machine-id");
    }

    let dbus_dir = fs_root.join("var/lib/dbus");
    fs::create_dir_all(&dbus_dir).expect("Failed to create /var/lib/dbus");
    let dbus_machine_id = dbus_dir.join("machine-id");
    match fs::symlink_metadata(&dbus_machine_id) {
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {
            symlink("/etc/machine-id", &dbus_machine_id)
                .expect("Failed to symlink /var/lib/dbus/machine-id");
        }
        Err(err) => panic!("Failed to inspect /var/lib/dbus/machine-id: {}", err),
    }

    None
}

fn is_valid_machine_id(value: &str) -> bool {
    let value = value.trim();
    value.len() == 32
        && value.chars().all(|c| c.is_ascii_hexdigit())
        && value.chars().any(|c| c != '0')
}

fn generate_machine_id() -> String {
    if let Ok(uuid) = fs::read_to_string("/proc/sys/kernel/random/uuid") {
        let id = uuid.trim().replace('-', "").to_ascii_lowercase();
        if is_valid_machine_id(&id) {
            return id;
        }
    }

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{:016x}{:016x}", nanos as u64, process::id() as u64)
}

pub fn sync_firefox_config(fs_root: &Path) {
    let candidates = [
        fs_root.join("usr/lib/firefox"),
        fs_root.join("usr/lib/firefox-esr"),
    ];

    let autoconfig_js = r#"pref("general.config.filename", "localdesktop.cfg");
pref("general.config.obscure_value", 0);
pref("general.config.sandbox_enabled", false);
"#;

    let firefox_cfg = r#"// Auto updated by Portal on each startup, do not edit manually
defaultPref("media.cubeb.sandbox", false);
defaultPref("security.sandbox.content.level", 0);
defaultPref("media.allow-audio-non-utility", true);
defaultPref("media.rdd-process.enabled", false);

"#;

    for dir in candidates {
        if dir.exists() || dir.parent().map_or(false, |p| p.exists()) {
            let pref_dir = dir.join("defaults/pref");
            let _ = fs::create_dir_all(&pref_dir);
            let _ = fs::write(pref_dir.join("autoconfig.js"), autoconfig_js);
            let _ = fs::write(dir.join("localdesktop.cfg"), firefox_cfg);
        }
    }
}

fn setup_firefox_config(_: &SetupOptions) -> StageOutput {
    use crate::core::runtime::LinuxRuntime;
    let active_runtime = crate::android::runtime::proot::PRootRuntime::active();
    sync_firefox_config(&active_runtime.rootfs_path());
    None
}

#[derive(Debug)]
enum KvLine {
    Entry {
        key: String,
        value: String,
        prefix: String,
        delimiter: char,
    },
    Other(String),
}

fn parse_kv_lines(content: &str, delimiter: char) -> Vec<KvLine> {
    content
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
                return KvLine::Other(line.to_string());
            }
            if let Some((left, right)) = line.split_once(delimiter) {
                let key = left.trim().to_string();
                if key.is_empty() {
                    return KvLine::Other(line.to_string());
                }
                let prefix_len = line.len() - trimmed.len();
                let prefix = line[..prefix_len].to_string();
                let value = right.trim().to_string();
                KvLine::Entry {
                    key,
                    value,
                    prefix,
                    delimiter,
                }
            } else {
                KvLine::Other(line.to_string())
            }
        })
        .collect()
}

fn set_kv_value(lines: &mut Vec<KvLine>, key: &str, value: &str, delimiter: char) {
    let mut updated = false;
    for line in lines.iter_mut() {
        if let KvLine::Entry {
            key: entry_key,
            value: entry_value,
            ..
        } = line
        {
            if entry_key == key {
                *entry_value = value.to_string();
                updated = true;
            }
        }
    }
    if !updated {
        lines.push(KvLine::Entry {
            key: key.to_string(),
            value: value.to_string(),
            prefix: String::new(),
            delimiter,
        });
    }
}

fn render_kv_lines(lines: &[KvLine]) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        match line {
            KvLine::Entry {
                key,
                value,
                prefix,
                delimiter,
            } => out.push(format!("{}{}{} {}", prefix, key, delimiter, value)),
            KvLine::Other(raw) => out.push(raw.to_string()),
        }
    }
    let mut content = out.join("\n");
    content.push('\n');
    content
}

fn upsert_kv_file(path: &Path, delimiter: char, updates: &[(&str, String)]) {
    let content = fs::read_to_string(path).unwrap_or_default();
    let mut lines = parse_kv_lines(&content, delimiter);
    for (key, value) in updates {
        set_kv_value(&mut lines, key, value, delimiter);
    }
    let content = render_kv_lines(&lines);
    fs::write(path, content).expect("Failed to write key/value file");
}

fn setup_fake_bwrap(_: &SetupOptions) -> StageOutput {
    let fs_root = Path::new(PRODUCTION_FS_ROOT);
    let wrapper_path = fs_root.join("usr/local/bin/bwrap");

    // bwrap (Bubblewrap) requires Linux user namespaces (CLONE_NEWUSER) which are
    // blocked by Android SELinux. We replace it with a shim that strips all
    // namespace/sandbox flags and directly exec's the target binary.
    // This unblocks glycin-svg (used by Onboard) which sandbox-loads SVG files via bwrap.
    let wrapper = r#"#!/bin/sh
# bwrap shim for proot/Android: namespaces are unavailable, exec directly.
# Strips all bwrap sandbox/namespace/bind flags, then exec's the target binary.
while [ $# -gt 0 ]; do
    case "$1" in
        # Three-argument flags (flag + src/key + dest/value)
        --ro-bind|--bind|--dev-bind|--bind-try|--ro-bind-try|--dev-bind-try|\
        --file|--bind-data|--ro-bind-data|--symlink|\
        --setenv|--chmod) shift 3 ;;
        # Two-argument flags (flag + single arg)
        --tmpfs|--proc|--dir|\
        --unsetenv|--perms|--cap-add|--cap-drop|\
        --seccomp|--add-seccomp-fd|--info-fd|--json-status-fd|\
        --block-fd|--userns-block-fd|--userns|--userns2|\
        --pidns|--chdir|--dev|--mqueue) shift 2 ;;
        # Zero-argument flags
        --unshare-all|--unshare-user|--unshare-user-try|--unshare-pid|\
        --unshare-ipc|--unshare-net|--unshare-uts|--unshare-cgroup|\
        --unshare-cgroup-try|--share-net|--remount-ro|\
        --as-pid-1|--die-with-parent|--new-session|--clearenv) shift ;;
        --) shift; break ;;
        *) break ;;
    esac
done
exec "$@"
"#;

    let _ = fs::create_dir_all(
        wrapper_path
            .parent()
            .expect("Failed to read bwrap wrapper parent directory"),
    );
    fs::write(&wrapper_path, wrapper).expect("Failed to write bwrap wrapper");
    fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755))
        .expect("Failed to mark bwrap wrapper executable");

    None
}

fn setup_chromium_no_sandbox(_: &SetupOptions) -> StageOutput {
    let fs_root = Path::new(PRODUCTION_FS_ROOT);

    // Chromium's sandbox needs CLONE_NEWUSER, which Android SELinux blocks, so every
    // Chromium/Electron app has to be started with --no-sandbox. Electron apps pick that up
    // from ELECTRON_DISABLE_SANDBOX (exported by the Plasma launcher), but Chromium itself
    // only takes the flag, and its desktop entry hardcodes an absolute path that a
    // /usr/local/bin wrapper cannot intercept. So shadow the affected application entries in
    // the user's own XDG directory, re-running every session to catch newly installed apps.
    write_executable(
        &fs_root.join("usr/local/bin/localdesktop-no-sandbox-entries"),
        r#"#!/bin/sh
target_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
mkdir -p "$target_dir" || exit 0

for src in /usr/share/applications/*.desktop /usr/local/share/applications/*.desktop; do
    [ -f "$src" ] || continue

    prog=$(sed -n 's/^Exec=//p' "$src" | head -n1 | awk '{print $1}')
    [ -n "$prog" ] || continue
    case "$prog" in
        /*) bin="$prog" ;;
        *) bin=$(command -v "$prog" 2>/dev/null) || continue ;;
    esac
    bin=$(readlink -f "$bin" 2>/dev/null)
    [ -n "$bin" ] || continue

    # Every Chromium/Electron build ships the setuid sandbox helper next to its binary,
    # or one level up when the launcher lives in a bin/ subdirectory.
    dir=$(dirname "$bin")
    [ -e "$dir/chrome-sandbox" ] || [ -e "$dir/../chrome-sandbox" ] || continue

    dst="$target_dir/$(basename "$src")"
    # Leave alone anything the user wrote themselves.
    if [ -e "$dst" ] && ! grep -q '^X-LocalDesktop-NoSandbox=' "$dst"; then
        continue
    fi

    awk '
        /^\[Desktop Entry\]/ && !seen { print; print "X-LocalDesktop-NoSandbox=true"; seen = 1; next }
        /^Exec=/ && !/--no-sandbox/ { sub(/^Exec=[^ ]+/, "& --no-sandbox") }
        { print }
    ' "$src" > "$dst"
done
"#,
    );

    // Same flag for terminal launches, following the /usr/local/bin PATH-priority pattern.
    write_executable(
        &fs_root.join("usr/local/bin/chromium"),
        r#"#!/bin/sh
[ -x /usr/bin/chromium ] || { echo "chromium is not installed" >&2; exit 127; }
exec /usr/bin/chromium --no-sandbox "$@"
"#,
    );

    None
}

fn setup_onboard_signal_fix(_: &SetupOptions) -> StageOutput {
    let fs_root = Path::new(PRODUCTION_FS_ROOT);
    let wrapper_path = fs_root.join("usr/local/bin/onboard");

    // proot intercepts fstat() on socket fds and follows /proc/self/fd/N which points
    // to "socket:[inode]" — not a real path. Python 3.14's signal.set_wakeup_fd()
    // calls fstat(fd) to validate the wakeup socket, which fails with ENOENT under proot.
    // We install a wrapper at /usr/local/bin/onboard (higher PATH priority than /usr/sbin)
    // that monkey-patches signal.set_wakeup_fd to swallow OSError before launching the
    // real Onboard binary.
    let wrapper = r#"#!/usr/bin/python3
# Onboard wrapper for proot/Android: patches signal.set_wakeup_fd to handle
# OSError (ENOENT) caused by proot's fstat translation on socket file descriptors.
import signal as _signal
_orig_swf = _signal.set_wakeup_fd
def _safe_swf(fd, **kwargs):
    try:
        return _orig_swf(fd, **kwargs)
    except OSError:
        return -1
_signal.set_wakeup_fd = _safe_swf

import runpy, sys
sys.argv[0] = '/usr/sbin/onboard'
runpy.run_path('/usr/sbin/onboard', run_name='__main__')
"#;

    let _ = fs::create_dir_all(
        wrapper_path
            .parent()
            .expect("Failed to read onboard wrapper parent directory"),
    );
    fs::write(&wrapper_path, wrapper).expect("Failed to write onboard wrapper");
    fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755))
        .expect("Failed to mark onboard wrapper executable");

    None
}

fn chroot_home_dir(fs_root: &Path, username: &str) -> PathBuf {
    if username == "root" {
        fs_root.join("root")
    } else {
        fs_root.join(format!("home/{username}"))
    }
}

fn normalize_guest_text(contents: &str) -> String {
    // The Android build often runs from a Windows checkout. Keep generated
    // guest text deterministic even when Git has materialized an asset with
    // CRLF line endings.
    contents.replace("\r\n", "\n").replace('\r', "\n")
}

fn write_executable(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // This source tree is checked out on Windows, where Git may materialize
    // text assets with CRLF. A guest kernel interprets the shebang literally,
    // so `#!/bin/bash\r` fails with ENOENT. Normalize at the Android/guest
    // boundary rather than relying on a developer's Git attributes.
    fs::write(path, normalize_guest_text(contents)).expect("Failed to write executable script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("Failed to mark executable script");
}

fn write_guest_binary(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let temporary = path.with_extension("portal-tmp");
    fs::write(&temporary, contents).expect("Failed to write guest binary");
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))
        .expect("Failed to mark guest binary executable");
    fs::rename(&temporary, path).expect("Failed to install guest binary");
}

/// Install a shipped configuration without clobbering a user's later edits.
/// Executable launch/recovery scripts are always refreshed above so upgrades
/// receive fixes, while normal application preferences remain user-owned.
fn write_default_file(path: &Path, contents: &str) {
    if path.exists() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(path, normalize_guest_text(contents))
        .expect("Failed to write default guest configuration");
}

/// Update one KConfig key while preserving unrelated groups, comments and
/// user preferences. Plasma's classic-session switch is an upgrade-sensitive
/// setting, so unlike an initial default it must be repaired on every setup.
fn upsert_kconfig_value(path: &Path, group: &str, key: &str, value: &str) {
    let content = fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    let group_header = format!("[{group}]");
    let group_start = lines
        .iter()
        .position(|line| line.trim().eq_ignore_ascii_case(&group_header));

    let Some(group_start) = group_start else {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(group_header);
        lines.push(format!("{key}={value}"));
        let mut output = lines.join("\n");
        output.push('\n');
        fs::write(path, output).expect("Failed to write KConfig value");
        return;
    };

    let group_end = lines
        .iter()
        .enumerate()
        .skip(group_start + 1)
        .find(|(_, line)| {
            let trimmed = line.trim();
            trimmed.starts_with('[') && trimmed.ends_with(']')
        })
        .map(|(index, _)| index)
        .unwrap_or(lines.len());
    let mut key_index = None;
    for (index, line) in lines
        .iter()
        .enumerate()
        .take(group_end)
        .skip(group_start + 1)
    {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        let Some((existing_key, _)) = trimmed.split_once('=').or_else(|| trimmed.split_once(':'))
        else {
            continue;
        };
        if existing_key.trim().eq_ignore_ascii_case(key) {
            key_index = Some(index);
            break;
        }
    }
    if let Some(index) = key_index {
        let prefix_len = lines[index].len() - lines[index].trim_start().len();
        let prefix = &lines[index][..prefix_len];
        lines[index] = format!("{prefix}{key}={value}");
    } else {
        lines.insert(group_start + 1, format!("{key}={value}"));
    }
    let mut output = lines.join("\n");
    output.push('\n');
    fs::write(path, output).expect("Failed to update KConfig value");
}

/// Map Android density to a whole-number UI scale factor (same baseline as the old LXQt setup).
#[allow(dead_code)]
fn android_ui_scale(density_dpi: i32) -> i32 {
    ((density_dpi as f32) / 160.0 * 1.1).max(1.0).round() as i32
}

/// Ensure guest package-management files are durable across slot switches and clean installs.
fn sync_debian_package_management(fs_root: &Path) {
    let apt_conf_d = fs_root.join("etc/apt/apt.conf.d");
    if fs::create_dir_all(&apt_conf_d).is_ok() {
        let no_sandbox_path = apt_conf_d.join("01no-sandbox");
        if !no_sandbox_path.exists() {
            let _ = fs::write(&no_sandbox_path, "APT::Sandbox::User \"root\";\n");
        }
        let clean_path = apt_conf_d.join("01portal-clean");
        if !clean_path.exists() {
            let _ = fs::write(
                &clean_path,
                "DPkg::Options { \"--force-confdef\"; \"--force-confold\"; };\n",
            );
        }
    }

    let sbin_dir = fs_root.join("usr/sbin");
    if fs::create_dir_all(&sbin_dir).is_ok() {
        let policy_rc_d = sbin_dir.join("policy-rc.d");
        if !policy_rc_d.exists() {
            let _ = fs::write(&policy_rc_d, "#!/bin/sh\nexit 101\n");
            let _ = fs::set_permissions(&policy_rc_d, fs::Permissions::from_mode(0o755));
        }
    }

    let dpkg_dir = fs_root.join("var/lib/dpkg");
    if fs::create_dir_all(&dpkg_dir).is_ok() {
        let arch_path = dpkg_dir.join("arch");
        if !arch_path.exists() {
            let _ = fs::write(&arch_path, "arm64\n");
        }
    }

    let dpkg_info_dir = fs_root.join("var/lib/dpkg/info");
    if fs::create_dir_all(&dpkg_info_dir).is_ok() {
        let format_path = dpkg_info_dir.join("format");
        if !format_path.exists() {
            let _ = fs::write(&format_path, "1\n");
        }
    }

    let sources_list = fs_root.join("etc/apt/sources.list");
    if !sources_list.exists() {
        if let Some(parent) = sources_list.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(
            &sources_list,
            "deb http://deb.debian.org/debian trixie main\n\
             deb http://deb.debian.org/debian trixie-updates main\n\
             deb http://security.debian.org/debian-security trixie-security main\n",
        );
    }
}

pub fn sync_session_runtime_files(fs_root: &Path, ui_scale: i32) {
    let username = get_application_context().local_config.user.username;
    let home_dir = chroot_home_dir(fs_root, &username);
    let xft_dpi = ui_scale * 96;

    // Normally created by systemd-tmpfiles, which does not run in PRoot.
    // KWin refuses to start Xwayland without this socket directory, and
    // Debian's ksmserver still needs that X connection in a Wayland session.
    for relative in ["tmp/.X11-unix", "tmp/.ICE-unix", "var/tmp"] {
        let directory = fs_root.join(relative);
        fs::create_dir_all(&directory).expect("Failed to create guest session directory");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o1777))
            .expect("Failed to set guest session directory permissions");
    }

    sync_android_timezone(fs_root);
    // Small, verified Debian tools needed for triggers skipped by image extraction.
    // Embedded in the APK so the published base image also works after uninstall.
    tar::Archive::new(std::io::Cursor::new(include_bytes!(
        "../../../assets/daily-use-tools.tar"
    )))
    .unpack(fs_root)
    .expect("Failed to install desktop cache tools");
    sync_firefox_config(fs_root);
    // Repair Portal's own launcher to use Debian's installed browser/icon name.
    let docs_entry = home_dir.join("Desktop/localdesktop-online-docs.desktop");
    if fs_root.join("usr/bin/firefox-esr").exists() {
        if let Ok(text) = fs::read_to_string(&docs_entry) {
            let text = text
                .replace("Exec=firefox ", "Exec=firefox-esr ")
                .replace("Icon=firefox\n", "Icon=firefox-esr\n");
            let _ = fs::write(&docs_entry, text);
            let _ = fs::set_permissions(&docs_entry, fs::Permissions::from_mode(0o755));
        }
    }
    sync_konsole_profile(fs_root, &home_dir);
    sync_debian_package_management(fs_root);

    let xresources_path = home_dir.join(".Xresources");
    let _ = fs::create_dir_all(
        xresources_path
            .parent()
            .expect("Failed to read Xresources parent directory"),
    );
    upsert_kv_file(&xresources_path, ':', &[("Xft.dpi", xft_dpi.to_string())]);

    // Disable screen locking completely: Android/OxygenOS owns device security.
    let config_dir = home_dir.join(".config");
    let _ = fs::create_dir_all(&config_dir);
    let kdeglobals = config_dir.join("kdeglobals");
    upsert_kv_file(
        &kdeglobals,
        '=',
        &[("action/lock_screen", "false".to_string())],
    );

    // These KCMs configure Linux-owned hardware/services that do not exist in
    // a nested Android PRoot session. Keep the KWin touchscreen-gestures KCM:
    // it legitimately acts on the wl_touch seat. Hide only the libinput device,
    // drawing-tablet and privileged timedated pages.
    for desktop_file in [
        "kcm_clock.desktop",
        "kcm_tablet.desktop",
        "kcm_mouse.desktop",
        "kcm_touchpad.desktop",
    ] {
        let path = fs_root.join("usr/share/applications").join(desktop_file);
        if path.is_file() {
            upsert_kv_file(
                &path,
                '=',
                &[
                    ("Hidden", "true".to_string()),
                    ("NoDisplay", "true".to_string()),
                ],
            );
        }
    }
    for plugin in [
        "usr/lib/aarch64-linux-gnu/qt6/plugins/plasma/kcms/systemsettings/kcm_touchscreen.so",
        "usr/lib/aarch64-linux-gnu/qt6/plugins/plasma/kcms/systemsettings/kcm_tablet.so",
        "usr/lib/aarch64-linux-gnu/qt6/plugins/plasma/kcms/systemsettings/kcm_mouse.so",
        "usr/lib/aarch64-linux-gnu/qt6/plugins/plasma/kcms/systemsettings/kcm_touchpad.so",
        "usr/lib/aarch64-linux-gnu/qt6/plugins/plasma/kcms/systemsettings_qwidgets/kcm_clock.so",
    ] {
        let source = fs_root.join(plugin);
        if source.is_file() {
            let disabled = source.with_extension("so.portal-disabled");
            fs::rename(&source, &disabled)
                .expect("Failed to disable an unsupported Plasma settings module");
        }
    }

    // Plasma's stock panel pins Discover even when this deliberately minimal
    // image has no package-management backend. Migrate that one dead launcher
    // to Dolphin once, then leave subsequent user panel customisation alone.
    let panel_marker = home_dir.join(".local/state/portal/panel-launchers-v1");
    if !panel_marker.exists() {
        let appletsrc = config_dir.join("plasma-org.kde.plasma.desktop-appletsrc");
        if let Ok(content) = fs::read_to_string(&appletsrc) {
            let discover_available = fs_root.join("usr/bin/plasma-discover").is_file();
            let dolphin_available = fs_root.join("usr/bin/dolphin").is_file();
            let mut changed = false;
            let mut lines = Vec::new();
            for line in content.lines() {
                if let Some(value) = line.strip_prefix("launchers=") {
                    let mut launchers: Vec<&str> = value
                        .split(',')
                        .filter(|entry| {
                            discover_available || *entry != "applications:org.kde.discover.desktop"
                        })
                        .collect();
                    if dolphin_available
                        && !launchers.contains(&"applications:org.kde.dolphin.desktop")
                    {
                        let at = launchers
                            .iter()
                            .position(|entry| entry.starts_with("preferred://"))
                            .unwrap_or(launchers.len());
                        launchers.insert(at, "applications:org.kde.dolphin.desktop");
                    }
                    let replacement = format!("launchers={}", launchers.join(","));
                    changed |= replacement != line;
                    lines.push(replacement);
                } else {
                    lines.push(line.to_string());
                }
            }
            if changed {
                fs::write(&appletsrc, format!("{}\n", lines.join("\n")))
                    .expect("Failed to repair the default panel launchers");
            }
        }
        if let Some(parent) = panel_marker.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(panel_marker, "migrated\n");
    }
    let kscreenlockerrc = config_dir.join("kscreenlockerrc");
    upsert_kv_file(
        &kscreenlockerrc,
        '=',
        &[
            ("Autolock", "false".to_string()),
            ("LockOnResume", "false".to_string()),
            ("Timeout", "0".to_string()),
        ],
    );

    // The guest scripts are versioned assets so the classic startup contract,
    // KWin crash capture and graphical recovery UI cannot drift apart. The
    // launcher substitutes only the device-specific scale factor.
    // Debug APKs opt into the wrapper's debugger path so a device run can
    // collect a real KWin trace when gdb is provisioned. The environment still
    // wins, and release APKs retain the zero-overhead default.
    let gdb_backtrace = if cfg!(debug_assertions) { "1" } else { "0" };
    let launcher = PLASMA_LAUNCHER
        .replace("@UI_SCALE@", &ui_scale.to_string())
        .replace("@GDB_BACKTRACE@", gdb_backtrace);
    write_executable(
        &fs_root.join("usr/local/bin/startplasma-localdesktop"),
        &launcher,
    );
    write_executable(&fs_root.join("usr/local/bin/kwin_wayland"), KWIN_WRAPPER);
    let recovery_launcher = RECOVERY_LAUNCHER.replace("@UI_SCALE@", &ui_scale.to_string());
    write_executable(
        &fs_root.join("usr/local/bin/start-localdesktop-recovery"),
        &recovery_launcher,
    );
    write_executable(
        &fs_root.join("usr/local/bin/localdesktop-retry-plasma"),
        RETRY_PLASMA,
    );
    write_executable(
        &fs_root.join("usr/local/bin/localdesktop-clipboard-sync"),
        CLIPBOARD_SYNC,
    );
    write_executable(
        &fs_root.join("usr/local/bin/localdesktop-clipboard-push"),
        CLIPBOARD_PUSH,
    );
    // Debian Trixie's locked runtime still carries wl-clipboard 2.2.1,
    // which predates KWin's ext-data-control support. Bundle the matching
    // ARM64 2.3 clients into /usr/local/bin so the helper is authoritative
    // across existing and newly provisioned runtime slots.
    write_guest_binary(&fs_root.join("usr/local/bin/wl-copy"), WL_COPY_BINARY);
    write_guest_binary(&fs_root.join("usr/local/bin/wl-paste"), WL_PASTE_BINARY);
    write_executable(
        &fs_root.join("usr/local/bin/ksplashqml"),
        "#!/bin/sh\nexit 0\n",
    );
    write_executable(
        &fs_root.join("usr/local/bin/plasma_waitforname"),
        "#!/bin/sh\nif [ \"$1\" = \"org.kde.KSplash\" ]; then\n    exit 0\nfi\nexec /usr/bin/plasma_waitforname \"$@\"\n",
    );
    write_executable(
        &fs_root.join("usr/local/bin/portal-ime-bridge"),
        PORTAL_IME_BRIDGE,
    );
    let ime_desktop_path = fs_root.join("usr/share/applications/portal-ime.desktop");
    if let Some(parent) = ime_desktop_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(ime_desktop_path, PORTAL_IME_DESKTOP);

    sync_guest_network_config(fs_root);
}

fn sync_konsole_profile(fs_root: &Path, home_dir: &Path) {
    let profile_content = "[General]\nCommand=/bin/bash\nName=Profile 1\nParent=FALLBACK/\n\n[Appearance]\nColorScheme=Breeze\n";
    for dir in [
        fs_root.join("usr/share/konsole"),
        home_dir.join(".local/share/konsole"),
    ] {
        let _ = fs::create_dir_all(&dir);
        let profile_path = dir.join("Profile 1.profile");
        if !profile_path.exists() {
            let _ = fs::write(&profile_path, profile_content);
        }
    }
}

fn sync_android_timezone(fs_root: &Path) {
    let Some(zone_id) = get_application_context().get_timezone_id() else {
        log::warn!("Android timezone was unavailable; retaining the guest timezone");
        return;
    };
    let relative = Path::new(&zone_id);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        log::warn!("Ignoring invalid Android timezone identifier: {zone_id:?}");
        return;
    }
    let zoneinfo = fs_root.join("usr/share/zoneinfo").join(relative);
    if !zoneinfo.is_file() {
        log::warn!("Android timezone is not present in the guest zoneinfo database: {zone_id}");
        return;
    }

    let etc = fs_root.join("etc");
    fs::create_dir_all(&etc).expect("Failed to create guest /etc for timezone sync");
    let localtime = etc.join("localtime");
    match fs::symlink_metadata(&localtime) {
        Ok(metadata) if metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            fs::remove_file(&localtime).expect("Failed to replace guest /etc/localtime");
        }
        Ok(_) => {
            log::warn!("Guest /etc/localtime is not a file; leaving it unchanged");
            return;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            log::warn!("Failed to inspect guest /etc/localtime: {error}");
            return;
        }
    }
    symlink(format!("/usr/share/zoneinfo/{zone_id}"), &localtime)
        .expect("Failed to link guest timezone to Android's zone");
    fs::write(etc.join("timezone"), format!("{zone_id}\n"))
        .expect("Failed to write guest /etc/timezone");
    log::info!("Synchronized guest timezone from Android: {zone_id}");
}

/// Keep guest network configuration (DNS resolver, NSS, hosts, SSL CA certificates)
/// synchronized with Android system state and distro requirements.
pub fn sync_guest_network_config(fs_root: &Path) {
    let context = get_application_context();
    let mut dns_servers = context.get_active_dns_servers();
    if dns_servers.is_empty() {
        dns_servers = vec!["8.8.8.8".to_string(), "1.1.1.1".to_string()];
    }

    let mut resolv_content =
        String::from("# Generated by Portal from Android ConnectivityManager\n");
    for srv in &dns_servers {
        resolv_content.push_str(&format!("nameserver {srv}\n"));
    }

    let etc_dir = fs_root.join("etc");
    let _ = fs::create_dir_all(&etc_dir);

    let resolv_conf = etc_dir.join("resolv.conf");
    let current_content = fs::read_to_string(&resolv_conf).unwrap_or_default();
    if current_content != resolv_content {
        if let Ok(()) = fs::write(&resolv_conf, normalize_guest_text(&resolv_content)) {
            log::info!(
                "Updated guest /etc/resolv.conf with active DNS: {:?}",
                dns_servers
            );
        }
    }

    // Ensure /etc/nsswitch.conf exists with standard host resolution
    let nsswitch = etc_dir.join("nsswitch.conf");
    if !nsswitch.exists() {
        let nsswitch_content = "passwd:         files\ngroup:          files\nshadow:         files\ngshadow:        files\n\nhosts:          files dns\nnetworks:       files\n\nprotocols:      db files\nservices:       db files\nethers:         db files\nrpc:            db files\n\nnetgroup:       nis\n";
        let _ = fs::write(&nsswitch, normalize_guest_text(nsswitch_content));
        log::info!("Seeded guest /etc/nsswitch.conf");
    }

    // Ensure /etc/hosts exists with localhost definitions
    let hosts = etc_dir.join("hosts");
    if !hosts.exists() {
        let hosts_content =
            "127.0.0.1       localhost\n::1             localhost ip6-localhost ip6-loopback\n";
        let _ = fs::write(&hosts, normalize_guest_text(hosts_content));
        log::info!("Seeded guest /etc/hosts");
    }

    // Ensure SSL CA certificates are present
    sync_guest_ssl_certificates(fs_root);
}

/// Ensure OpenSSL and standard Linux tools inside the guest have access to valid CA certificates.
pub fn sync_guest_ssl_certificates(fs_root: &Path) {
    let certs_dir = fs_root.join("etc/ssl/certs");
    let _ = fs::create_dir_all(&certs_dir);
    let ca_bundle = certs_dir.join("ca-certificates.crt");

    if !ca_bundle.exists() || fs::metadata(&ca_bundle).map(|m| m.len()).unwrap_or(0) == 0 {
        let mut bundle_data = Vec::new();
        for dir in [
            "/system/etc/security/cacerts",
            "/apex/com.android.conscrypt/cacerts",
        ] {
            let path = Path::new(dir);
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    if let Ok(content) = fs::read(entry.path()) {
                        bundle_data.extend_from_slice(&content);
                        if !content.ends_with(b"\n") {
                            bundle_data.push(b'\n');
                        }
                    }
                }
            }
        }
        if !bundle_data.is_empty() {
            let _ = fs::write(&ca_bundle, &bundle_data);
            log::info!(
                "Bundled Android system CA certificates into {}",
                ca_bundle.display()
            );
        }
    }

    // Symlink /etc/ssl/cert.pem -> certs/ca-certificates.crt
    let cert_pem = fs_root.join("etc/ssl/cert.pem");
    if !cert_pem.exists() {
        let _ = symlink("certs/ca-certificates.crt", &cert_pem);
    }

    // Symlink /usr/lib/ssl -> /etc/ssl
    let usr_lib = fs_root.join("usr/lib");
    let _ = fs::create_dir_all(&usr_lib);
    let usr_lib_ssl = usr_lib.join("ssl");
    if !usr_lib_ssl.exists() {
        let _ = symlink("/etc/ssl", &usr_lib_ssl);
    }
}

fn setup_plasma_wayland(_options: &SetupOptions) -> StageOutput {
    let fs_root = Path::new(PRODUCTION_FS_ROOT);
    let username = get_application_context().local_config.user.username;
    let home_dir = chroot_home_dir(fs_root, &username);
    // The host Wayland compositor already establishes a logical viewport scaled by
    // guest_scale_factor; the guest Plasma session must run at 1:1 (scale 1) to prevent double scaling.
    let ui_scale = 1;
    sync_session_runtime_files(fs_root, ui_scale);

    // Debian supplies its matching KWin library; the former Arch 6.7 library
    // is ABI-incompatible with the Debian 6.3 executable and is not installed.
    // All builds need the socket fstat fix in this existing library. A
    // nested gdb frequently dies before it can attach under Android's PRoot;
    // this preload still records the fault PC/LR/SP, loader maps and a best-
    // effort glibc backtrace from inside KWin.  Keep the source in the guest
    // so the archive identifies exactly which handler produced the trace.
    let crash_handler_source = fs_root.join("usr/local/lib/localdesktop-crash-handler.c");
    if let Some(parent) = crash_handler_source.parent() {
        fs::create_dir_all(parent).expect("Failed to create crash handler directory");
    }
    fs::write(
        &crash_handler_source,
        normalize_guest_text(CRASH_HANDLER_SOURCE),
    )
    .expect("Failed to write crash handler source");
    fs::set_permissions(&crash_handler_source, fs::Permissions::from_mode(0o644))
        .expect("Failed to set crash handler source permissions");

    let handler = fs_root.join("usr/local/lib/localdesktop-crash-handler.so");
    let temporary = handler.with_extension("so.tmp");
    fs::write(&temporary, CRASH_HANDLER_BINARY)
        .expect("Failed to stage required guest support library");
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))
        .expect("Failed to set guest support library permissions");
    fs::rename(&temporary, &handler).expect("Failed to install required guest support library");
    diagnostics::guest_event(
        "guest-support",
        "installed bundled ARM64 glibc socket-stat shim",
    );

    // Recovery creates its labwc autostart at runtime, after writing the
    // actionable kdialog message. Do not pre-seed an autostart that launches
    // a terminal or bypasses that recovery flow.

    let konsole_profile_dir = home_dir.join(".local/share/konsole");
    write_default_file(&home_dir.join(".config/konsolerc"), KONSOLE_CONFIG);
    let konsole_profile_path = konsole_profile_dir.join("LocalDesktop.profile");
    // Konsole reads this path after PRoot has switched into the guest.  Do
    // not leak the host-side `/data/.../archlinux-*` prefix into the profile;
    // that path is not meaningful inside the guest namespace.
    let guest_home = if username == "root" {
        "/root".to_owned()
    } else {
        format!("/home/{username}")
    };
    let konsole_profile = KONSOLE_PROFILE.replace("@HOME@", &guest_home);
    write_default_file(&konsole_profile_path, &konsole_profile);
    // Existing upgrades may have received the old empty-command profile.
    // Repair only the launch keys, preserving appearance and other user edits.
    upsert_kconfig_value(&konsole_profile_path, "General", "Command", "/bin/bash");
    upsert_kconfig_value(&konsole_profile_path, "General", "Directory", &guest_home);

    let config_dir = home_dir.join(".config");
    let autostart_dir = config_dir.join("autostart");
    let _ = fs::create_dir_all(&autostart_dir);
    // Plasma 6's launcher reads this KConfig gate; the similarly named
    // environment variables are not sufficient on current Plasma releases.
    // Keep the setting in the user's config so startplasma-wayland takes the
    // classic dbus-run-session path and never asks a missing user systemd to
    // own the session bus.
    let startkde_config = config_dir.join("startkderc");
    write_default_file(&startkde_config, "[General]\nsystemdBoot=false\n");
    upsert_kconfig_value(&startkde_config, "General", "systemdBoot", "false");
    fs::write(
        config_dir.join("ksmserverrc"),
        "[General]\nloginMode=emptySession\nconfirmLogout=false\n",
    )
    .expect("Failed to write Plasma session defaults");
    fs::write(
        autostart_dir.join("localdesktop-session-init.desktop"),
        r#"[Desktop Entry]
Type=Application
Name=Portal Session Integration
Exec=/usr/local/bin/localdesktop-no-sandbox-entries
OnlyShowIn=KDE;
X-KDE-autostart-after=panel
"#,
    )
    .expect("Failed to write Plasma session integration autostart entry");
    fs::write(
        autostart_dir.join("powerdevil.desktop"),
        "[Desktop Entry]\nType=Application\nName=Power Management\nHidden=true\nOnlyShowIn=KDE;\n",
    )
    .expect("Failed to disable guest power management autostart");

    let desktop_dir = home_dir.join("Desktop");
    let _ = fs::create_dir_all(&desktop_dir);
    let online_docs = desktop_dir.join("localdesktop-online-docs.desktop");
    if !online_docs.exists() {
        fs::write(
            online_docs,
            format!(
                "[Desktop Entry]\nType=Application\nName=Portal - Online Docs\nExec=firefox {DOCS_HOME_URL}\nIcon=firefox\nTerminal=false\n"
            ),
        )
        .expect("Failed to write documentation desktop entry");
    }

    None
}
fn fix_xkb_symlink(options: &SetupOptions) -> StageOutput {
    let fs_root = Path::new(PRODUCTION_FS_ROOT);
    let xkb_path = fs_root.join("usr/share/X11/xkb");
    let mpsc_sender = options.mpsc_sender.clone();

    if let Ok(meta) = fs::symlink_metadata(&xkb_path) {
        if meta.file_type().is_symlink() {
            if let Ok(target) = fs::read_link(&xkb_path) {
                if target.is_absolute() {
                    log::info!(
                        "Absolute symlink target detected: {} -> {}. This is a problem because libxkbcommon is loaded in NDK, whose / is not Arch FS root!",
                        xkb_path.display(),
                        target.display()
                    );
                    // Compute the relative path from /usr/share/X11/xkb to /usr/share/xkeyboard-config-2
                    // Both are inside the chroot, so strip the fs_root prefix
                    let xkb_inside = Path::new("/usr/share/X11/xkb");
                    let target_inside = Path::new("/usr/share/xkeyboard-config-2");
                    let rel_target = diff_paths(target_inside, xkb_inside.parent().unwrap())
                        .unwrap_or_else(|| target_inside.to_path_buf());
                    log::info!(
                        "Fixing with new relative symlink: {} -> {}",
                        xkb_path.display(),
                        rel_target.display()
                    );
                    // Remove the old symlink
                    let _ = fs::remove_file(&xkb_path);
                    // Create the new relative symlink
                    if let Err(e) = symlink(&rel_target, &xkb_path) {
                        mpsc_sender
                            .send(SetupMessage::Error(format!(
                                "Failed to create relative symlink for xkb: {}",
                                e
                            )))
                            .unwrap_or(());
                    }
                }
            }
        }
    }
    None
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else {
        "unknown panic".to_string()
    }
}

fn send_setup_error(
    index: usize,
    name: &str,
    payload: &(dyn std::any::Any + Send),
    sender: &Sender<SetupMessage>,
) {
    let message = format!(
        "Setup stage {index} ({name}) failed: {}. Reopen Portal to retry, or export diagnostics for support.",
        panic_text(payload)
    );
    log::error!("{message}");
    let _ = sender.send(SetupMessage::Error(message));
}

/// Invoke a stage behind a panic boundary and emit the durable stage events
/// consumed by diagnostics exports. Stage functions historically used
/// `expect` for filesystem/package failures; converting those panics into an
/// explicit WebView error keeps the installer actionable instead of blank.
fn invoke_stage(
    index: usize,
    name: &'static str,
    stage: &SetupStage,
    options: &SetupOptions,
    sender: &Sender<SetupMessage>,
) -> Result<StageOutput, ()> {
    diagnostics::setup_stage(index, name, "start");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| stage(options)));
    match result {
        Ok(output) => Ok(output),
        Err(payload) => {
            diagnostics::setup_stage(index, name, "failed");
            send_setup_error(index, name, payload.as_ref(), sender);
            Err(())
        }
    }
}

fn complete_stage(index: usize, name: &'static str) {
    diagnostics::setup_stage(index, name, "complete");
}

fn update_progress(progress: &Arc<Mutex<u16>>, index: usize, stage_count: usize) {
    let value = ((index * 100) / stage_count.max(1)).min(100) as u16;
    if let Ok(mut current) = progress.lock() {
        *current = value;
    }
}

fn run_remaining_stages(
    first_index: usize,
    first_name: &'static str,
    first_handle: JoinHandle<()>,
    stages: Vec<NamedSetupStage>,
    options: SetupOptions,
    progress: Arc<Mutex<u16>>,
    sender: Sender<SetupMessage>,
    on_complete: Option<SetupCompletionCallback>,
) {
    let stage_count = stages.len();
    if let Err(payload) = first_handle.join() {
        diagnostics::setup_stage(first_index + 1, first_name, "failed");
        send_setup_error(first_index + 1, first_name, payload.as_ref(), &sender);
        return;
    }
    complete_stage(first_index + 1, first_name);
    update_progress(&progress, first_index + 1, stage_count);

    for (index, (name, stage)) in stages.into_iter().enumerate().skip(first_index + 1) {
        update_progress(&progress, index, stage_count);
        let output = match invoke_stage(index + 1, name, &stage, &options, &sender) {
            Ok(output) => output,
            Err(()) => return,
        };
        if let Some(handle) = output {
            if let Err(payload) = handle.join() {
                diagnostics::setup_stage(index + 1, name, "failed");
                send_setup_error(index + 1, name, payload.as_ref(), &sender);
                return;
            }
        }
        complete_stage(index + 1, name);
        update_progress(&progress, index + 1, stage_count);
    }

    if let Ok(mut current) = progress.lock() {
        *current = 100;
    }
    let _ = sender.send(SetupMessage::Progress(
        "Installation finished. Starting Plasma…".to_string(),
    ));
    diagnostics::host_event("setup-complete", "all guest provisioning stages completed");
    if let Some(on_complete) = on_complete {
        on_complete();
    }
}

fn build_wayland_backend(android_app: AndroidApp) -> PolarBearBackend {
    let size = android_app
        .native_window()
        .map(|nw| (nw.width(), nw.height()))
        .unwrap_or((1920, 1080));
    let guest_scale_factor = scale_factor(&android_app);
    let mut compositor =
        Compositor::new(size, guest_scale_factor).expect("Failed to build compositor");
    compositor.enable_android_clipboard(android_app.clone());
    PolarBearBackend::Wayland(WaylandBackend {
        compositor,
        graphic_renderer: None,
        clock: Clock::new(),
        key_counter: 0,
        guest_scale_factor,
        touch_points: std::collections::HashMap::new(),
        scroll_centroid: None,
        touch_mode: TouchMode::Undecided,
        touch_down_position: None,
        touch_down_time: None,
        touch_down_generation: None,
        touch_slop_px: touch_slop_px(&android_app),
        long_press_timeout_ms: long_press_timeout_ms(&android_app),
        pointer_pressed: false,
        presentation_sequence: 0,
        pending_kwin_presentation: None,
        // Nominal output mode is the stable preferred target resolved from
        // `Display.getSupportedModes()` (144 Hz on the OnePlus Pad 3,
        // otherwise the device maximum): never the transient cold-start VRR
        // reading. The live physical rate is tracked separately in
        // `physical_refresh_millihz` for diagnostics/pacing and never
        // rewrites `wl_output`.
        refresh_rate_millihz: crate::android::utils::ndk::refresh_rate_millihz(&android_app),
        physical_refresh_millihz: active_refresh_millihz(&android_app),
        pressed_keys: std::collections::HashSet::new(),
        button_tracker: crate::core::pointer_buttons::PointerButtonTracker::new(),
        suppressed_touch_ids: std::collections::HashSet::new(),
        last_plasma_poll_ms: None,
        last_refresh_poll_ms: None,
        frame_rate_requested: false,
        kwin_commit_gate: crate::core::presentation::KwinCommitGate::new(),
        socket_watcher: None,
        output_dirty: true,
        frame_in_flight: false,
        android_app,
    })
}

/// Backwards-compatible setup entry point. Lifecycle owners that can dismiss
/// the provisioning popup in-process should use `setup_with_completion`.
pub fn setup(android_app: AndroidApp) -> PolarBearBackend {
    setup_with_completion(android_app, None)
}

/// Provision the guest and invoke `on_complete` after the final stage without
/// recreating the NativeActivity. The lifecycle owner can use that callback to
/// send an event through its event-loop proxy and construct the Wayland backend
/// in the current activity.
pub fn setup_with_completion(
    android_app: AndroidApp,
    on_complete: Option<SetupCompletionCallback>,
) -> PolarBearBackend {
    let (sender, receiver) = mpsc::channel();
    let progress = Arc::new(Mutex::new(0));

    if ArchProcess::is_supported(&android_app) {
        sender
            .send(SetupMessage::Progress(
                "✅ Your device is supported!".to_string(),
            ))
            .unwrap_or(());
    } else {
        log::info!("PRoot support check failed, showing Device Unsupported page");
        diagnostics::host_event("setup-unsupported", "PRoot support probe failed");
        return PolarBearBackend::WebView(WebviewBackend {
            socket_port: 0,
            progress,
            error: ErrorVariant::Unsupported,
        });
    }

    let options = SetupOptions {
        android_app: android_app.clone(),
        mpsc_sender: sender.clone(),
    };

    let stages: Vec<NamedSetupStage> = vec![
        ("debian-runtime", Box::new(setup_debian_runtime)),
        ("linux-sysdata", Box::new(simulate_linux_sysdata_stage)),
        ("machine-id", Box::new(setup_machine_id)),
        ("firefox-config", Box::new(setup_firefox_config)),
        ("bwrap-shim", Box::new(setup_fake_bwrap)),
        ("chromium-no-sandbox", Box::new(setup_chromium_no_sandbox)),
        ("onboard-signal-fix", Box::new(setup_onboard_signal_fix)),
        ("plasma-wayland", Box::new(setup_plasma_wayland)),
        ("xkb-symlink", Box::new(fix_xkb_symlink)),
    ];

    for (index, (name, stage)) in stages.iter().enumerate() {
        let stage_name = *name;
        update_progress(&progress, index, stages.len());
        let output = match invoke_stage(index + 1, stage_name, stage, &options, &sender) {
            Ok(output) => output,
            Err(()) => return PolarBearBackend::WebView(WebviewBackend::build(receiver, progress)),
        };
        let Some(handle) = output else {
            complete_stage(index + 1, stage_name);
            update_progress(&progress, index + 1, stages.len());
            continue;
        };

        let progress_clone = progress.clone();
        let sender_clone = sender.clone();
        let options_clone = SetupOptions {
            android_app: options.android_app.clone(),
            mpsc_sender: options.mpsc_sender.clone(),
        };
        let on_complete = on_complete.clone();
        thread::spawn(move || {
            run_remaining_stages(
                index,
                stage_name,
                handle,
                stages,
                options_clone,
                progress_clone,
                sender_clone,
                on_complete,
            );
        });
        return PolarBearBackend::WebView(WebviewBackend::build(receiver, progress));
    }

    build_wayland_backend(android_app)
}

#[cfg(test)]
mod tests {
    use super::normalize_guest_text;

    #[test]
    fn guest_scripts_are_written_with_unix_line_endings() {
        assert_eq!(
            normalize_guest_text("#!/bin/bash\r\nready\r\n"),
            "#!/bin/bash\nready\n"
        );
        assert_eq!(normalize_guest_text("line\rnext\n"), "line\next\n");
    }
}
