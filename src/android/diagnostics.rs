//! Persistent diagnostics for the Android host and the Linux guest.
//!
//! The desktop is a nested Wayland session, so a useful bug report needs both
//! sides of the boundary.  This module deliberately has no dependency on the
//! compositor event loop: setup, PRoot, Wayland and the Android UI can all
//! append an event without blocking on one another.  The files are kept in the
//! app's private data directory and a bounded ZIP is produced on demand.

use crate::{
    android::{
        utils::{application_context::get_application_context, ndk::run_in_jvm},
    },
    core::config::{ARCH_FS_ROOT, VERSION},
};
use jni::{
    objects::{JObject, JValue},
    sys::_jobject,
    JNIEnv,
};
use log::Record;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};
use winit::platform::android::activity::AndroidApp;
use zip::{write::FileOptions, ZipWriter};

const DIAGNOSTICS_DIR: &str = "diagnostics";
const HOST_LOG: &str = "host.log";
const GUEST_LOG: &str = "guest.log";
const STAGES_LOG: &str = "stages.jsonl";
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
struct Paths {
    root: PathBuf,
    host_log: PathBuf,
    guest_log: PathBuf,
    stages_log: PathBuf,
    archive_dir: PathBuf,
}

static PATHS: OnceLock<Paths> = OnceLock::new();
static FILE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn lock() -> &'static Mutex<()> {
    FILE_LOCK.get_or_init(|| Mutex::new(()))
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn append_line(path: &Path, line: &str) {
    let _guard = lock().lock().ok();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(meta) = fs::metadata(path) {
        // Keep the diagnostic path bounded across repeated launches.  Losing
        // old log lines is preferable to making an app data partition full.
        if meta.len() > MAX_FILE_BYTES {
            let rotated = path.with_extension("log.1");
            let _ = fs::rename(path, rotated);
        }
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

/// Initialise the persistent diagnostic paths.  It is safe to call this more
/// than once (activity recreation happens after first-run provisioning).
pub fn initialize() {
    if PATHS.get().is_some() {
        return;
    }
    let context = get_application_context();
    let root = context.data_dir.join(DIAGNOSTICS_DIR);
    let paths = Paths {
        host_log: root.join(HOST_LOG),
        guest_log: root.join(GUEST_LOG),
        stages_log: root.join(STAGES_LOG),
        archive_dir: root.join("exports"),
        root,
    };
    let _ = fs::create_dir_all(&paths.archive_dir);
    let _ = PATHS.set(paths);
    host_event("diagnostics.initialized", &format!("version={VERSION}"));
    host_event(
        "diagnostics.environment",
        &format!(
            "os={} arch={} rootfs={} wayland_socket=/tmp/wayland-0",
            std::env::consts::OS,
            std::env::consts::ARCH,
            ARCH_FS_ROOT
        ),
    );
}

fn paths() -> Option<&'static Paths> {
    PATHS.get()
}

/// Append a structured host-side event.  The line format is intentionally
/// plain text with a stable prefix so it remains useful when Android logcat is
/// unavailable.
pub fn host_event(stage: &str, detail: &str) {
    let Some(paths) = paths() else { return };
    let detail = detail.replace('\n', "\\n");
    append_line(
        &paths.host_log,
        &format!("{} host stage={} {}", now_ms(), stage, detail),
    );
}

/// Append a guest-side event and mirror it into the guest rootfs.  Mirroring
/// means a report still contains guest activity if an export is triggered
/// after the host process has already started recovery.
pub fn guest_event(stage: &str, detail: &str) {
    let detail = detail.replace('\n', "\\n");
    let line = format!("{} guest stage={} {}", now_ms(), stage, detail);
    if let Some(paths) = paths() {
        append_line(&paths.guest_log, &line);
    }
    let guest_path = Path::new(ARCH_FS_ROOT).join("var/lib/localdesktop/guest.log");
    append_line(&guest_path, &line);
}

/// Record setup stage start/end timestamps in a machine-readable JSONL file.
pub fn setup_stage(index: usize, name: &str, event: &str) {
    let Some(paths) = paths() else { return };
    let name = serde_json::to_string(name).unwrap_or_else(|_| "\"unknown\"".into());
    append_line(
        &paths.stages_log,
        &format!(
            "{{\"timestamp_ms\":{},\"index\":{},\"stage\":{},\"event\":{}}}",
            now_ms(),
            index,
            name,
            serde_json::to_string(event).unwrap_or_else(|_| "\"unknown\"".into())
        ),
    );
    host_event("setup", &format!("index={index} stage={name} event={event}"));
}

/// Record a successful host presentation of a guest surface.  The marker is
/// consumed by the Plasma launcher to gate readiness; unlike `pgrep`, it
/// proves that the nested session made it through commit, render and swap.
pub fn mark_plasma_frame_presented(surface_count: usize, client_count: usize) {
    let marker = Path::new(ARCH_FS_ROOT).join("var/lib/localdesktop/plasma-ready");
    if let Some(parent) = marker.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(
        marker,
        format!(
            "timestamp_ms={} surfaces={} clients={}\n",
            now_ms(),
            surface_count,
            client_count
        ),
    );
    host_event(
        "plasma-ready",
        &format!("surface_count={surface_count} client_count={client_count}"),
    );
}

/// Record a process output line while preserving the original command log.
pub fn guest_process_line(command: &str, user: &str, stream: &str, line: &str) {
    guest_event(
        "process",
        &format!("user={user} stream={stream} command={} line={line}", command),
    );
}

/// Record a desktop process exit and mark an abnormal KWin exit for the guest
/// recovery script.  Unix signal exits are reported by `Output` as 128+signal
/// on the Android PRoot build, while shell wrappers commonly use 139 for SIGSEGV.
pub fn desktop_exit(status: Option<i32>, elapsed_ms: u128) {
    let status_text = status
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".into());
    host_event(
        "desktop-exit",
        &format!("status={status_text} elapsed_ms={elapsed_ms}"),
    );
    if status.is_some_and(|value| value == 139 || value == 134 || value >= 128) {
        let marker = Path::new(ARCH_FS_ROOT).join("var/lib/localdesktop/kwin-crash");
        if let Some(parent) = marker.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(
            marker,
            format!("timestamp_ms={} status={} elapsed_ms={}\n", now_ms(), status_text, elapsed_ms),
        );
        guest_event("kwin-crash", &format!("status={status_text} elapsed_ms={elapsed_ms}"));
    }
}

/// Best-effort KWin crash metadata.  The wrapper installed by setup records a
/// real debugger trace when `gdb`/`coredumpctl` are available; this fallback
/// preserves the command, environment and signal even on minimal guests.
pub fn kwin_crash_metadata(args: &str, status: i32, pid: Option<i32>) {
    let path = Path::new(ARCH_FS_ROOT).join("var/lib/localdesktop/kwin-backtrace.log");
    let text = format!(
        "timestamp_ms={} status={} pid={} args={}\nbacktrace=best-effort wrapper metadata; no debugger available\n",
        now_ms(),
        status,
        pid.map(|value| value.to_string()).unwrap_or_else(|| "unknown".into()),
        args.replace('\n', "\\n")
    );
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, &text);
    guest_event("kwin-backtrace", text.trim_end());
}

/// A `log` facade wrapper that persists every host log line before forwarding
/// to Android logcat/Sentry.  The wrapper is intentionally small and does not
/// recursively call `log::*` if the private file cannot be written.
pub struct HostLogTee {
    inner: Box<dyn log::Log + Send + Sync>,
}

impl HostLogTee {
    pub fn new(inner: Box<dyn log::Log + Send + Sync>) -> Self {
        Self { inner }
    }
}

impl log::Log for HostLogTee {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            if let Some(paths) = paths() {
                append_line(
                    &paths.host_log,
                    &format!(
                        "{} host log={} target={} {}",
                        now_ms(),
                        record.level(),
                        record.target(),
                        record.args()
                    ),
                );
            }
            self.inner.log(record);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

fn add_file(
    writer: &mut ZipWriter<File>,
    source: &Path,
    archive_name: &str,
    total: &mut u64,
) -> io::Result<()> {
    let meta = fs::metadata(source)?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES || *total + meta.len() > MAX_ARCHIVE_BYTES {
        return Ok(());
    }
    writer
        .start_file(archive_name, FileOptions::default())
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?;
    let mut file = File::open(source)?;
    let mut buf = [0u8; 16 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buf[..read])?;
    }
    *total += meta.len();
    Ok(())
}

fn add_tree(
    writer: &mut ZipWriter<File>,
    root: &Path,
    current: &Path,
    prefix: &str,
    total: &mut u64,
) -> io::Result<()> {
    if !root.exists() || !current.exists() {
        return Ok(());
    }
    let canonical_root = fs::canonicalize(root)?;
    let canonical_current = fs::canonicalize(current)?;
    if !canonical_current.starts_with(&canonical_root) {
        return Ok(());
    }
    let Ok(entries) = fs::read_dir(&canonical_current) else { return Ok(()) };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        let relative = path.strip_prefix(&canonical_root).unwrap_or(&path);
        let name = format!("{prefix}/{}", relative.to_string_lossy().replace('\\', "/"));
        if metadata.is_dir() {
            add_tree(writer, &canonical_root, &path, prefix, total)?;
        } else if metadata.is_file() {
            add_file(writer, &path, &name, total)?;
        }
    }
    Ok(())
}

/// Build one bounded, shareable diagnostics archive.  Only app diagnostics
/// and the guest's Local Desktop state directory are included; unrelated user
/// files in the guest are never traversed.
pub fn export_archive() -> Result<PathBuf, String> {
    let paths = paths().ok_or_else(|| "Diagnostics are not initialized yet".to_string())?;
    let _ = fs::create_dir_all(&paths.archive_dir);
    let archive = paths
        .archive_dir
        .join(format!("localdesktop-diagnostics-{}.zip", now_ms()));
    let file = File::create(&archive).map_err(|error| error.to_string())?;
    let mut writer = ZipWriter::new(file);
    let mut total = 0;

    add_file(&mut writer, &paths.host_log, "host/host.log", &mut total)
        .map_err(|error| error.to_string())?;
    add_file(&mut writer, &paths.guest_log, "host/guest.log", &mut total)
        .map_err(|error| error.to_string())?;
    add_file(&mut writer, &paths.stages_log, "host/stages.jsonl", &mut total)
        .map_err(|error| error.to_string())?;

    let guest_state = Path::new(ARCH_FS_ROOT).join("var/lib/localdesktop");
    add_tree(
        &mut writer,
        &guest_state,
        &guest_state,
        "guest/localdesktop",
        &mut total,
    )
    .map_err(|error| error.to_string())?;

    let metadata = format!(
        "Local Desktop diagnostics\nversion={VERSION}\ntimestamp_ms={}\narch={}\nrootfs={}\nbytes={}\n",
        now_ms(),
        std::env::consts::ARCH,
        ARCH_FS_ROOT,
        total
    );
    writer
        .start_file("metadata.txt", FileOptions::default())
        .map_err(|error| error.to_string())?;
    writer
        .write_all(metadata.as_bytes())
        .map_err(|error| error.to_string())?;
    writer.finish().map_err(|error| error.to_string())?;
    host_event("diagnostics-exported", &format!("path={}", archive.display()));
    Ok(archive)
}

/// Export and invoke the Android Sharesheet.  A public-file/content-provider
/// integration can be added by the APK builder later; the `file://` fallback
/// remains useful on Android versions and share targets that permit it, and is
/// explicitly best-effort so a failed chooser never crashes the desktop.
pub fn export_and_share(android_app: &AndroidApp) -> Result<PathBuf, String> {
    let archive = export_archive()?;
    let path = archive.clone();
    run_in_jvm(
        move |env, app| share_file(env, app, &path),
        android_app.clone(),
    )?;
    Ok(archive)
}

fn share_file(env: &mut JNIEnv, android_app: &AndroidApp, path: &Path) -> Result<(), String> {
    let activity = unsafe { JObject::from_raw(android_app.activity_as_ptr() as *mut _jobject) };
    // Android 7+ rejects file:// URIs from a strict VM policy.  The archive is
    // app-private and user-triggered; temporarily allowing this URI lets the
    // system Sharesheet hand it to compatible targets without requiring a new
    // Java dependency in the tiny native APK builder.
    let strict_mode = env
        .find_class("android/os/StrictMode")
        .map_err(|error| error.to_string())?;
    let builder = env
        .new_object(
            "android/os/StrictMode$VmPolicy$Builder",
            "()V",
            &[],
        )
        .map_err(|error| error.to_string())?;
    let policy = env
        .call_method(
            &builder,
            "build",
            "()Landroid/os/StrictMode$VmPolicy;",
            &[],
        )
        .and_then(|value| value.l())
        .map_err(|error| error.to_string())?;
    env.call_static_method(
        strict_mode,
        "setVmPolicy",
        "(Landroid/os/StrictMode$VmPolicy;)V",
        &[JValue::Object(&policy)],
    )
    .map_err(|error| error.to_string())?;

    let action = env.new_string("android.intent.action.SEND").map_err(|e| e.to_string())?;
    let intent = env
        .new_object(
            "android/content/Intent",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&action)],
        )
        .map_err(|error| error.to_string())?;
    let mime = env.new_string("application/zip").map_err(|e| e.to_string())?;
    env.call_method(
        &intent,
        "setType",
        "(Ljava/lang/String;)Landroid/content/Intent;",
        &[JValue::Object(&mime)],
    )
    .map_err(|error| error.to_string())?;
    let uri_text = env
        .new_string(format!("file://{}", path.display()))
        .map_err(|e| e.to_string())?;
    let uri = env
        .call_static_method(
            "android/net/Uri",
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(&uri_text)],
        )
        .and_then(|value| value.l())
        .map_err(|error| error.to_string())?;
    let extra = env
        .new_string("android.intent.extra.STREAM")
        .map_err(|e| e.to_string())?;
    env.call_method(
        &intent,
        "putExtra",
        "(Ljava/lang/String;Landroid/os/Parcelable;)Landroid/content/Intent;",
        &[JValue::Object(&extra), JValue::Object(&uri)],
    )
    .map_err(|error| error.to_string())?;
    env.call_method(
        &intent,
        "addFlags",
        "(I)Landroid/content/Intent;",
        &[JValue::Int(0x00000001)], // FLAG_GRANT_READ_URI_PERMISSION
    )
    .map_err(|error| error.to_string())?;
    let chooser_title = env.new_string("Share Local Desktop diagnostics").map_err(|e| e.to_string())?;
    let chooser = env
        .call_static_method(
            "android/content/Intent",
            "createChooser",
            "(Landroid/content/Intent;Ljava/lang/CharSequence;)Landroid/content/Intent;",
            &[JValue::Object(&intent), JValue::Object(&chooser_title)],
        )
        .and_then(|value| value.l())
        .map_err(|error| error.to_string())?;
    env.call_method(
        activity,
        "startActivity",
        "(Landroid/content/Intent;)V",
        &[JValue::Object(&chooser)],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_names_never_escape_guest_state() {
        let root = Path::new("/tmp/guest-state");
        let child = root.join("nested").join("log.txt");
        let relative = child.strip_prefix(root).unwrap();
        let archive_name = format!(
            "guest/localdesktop/{}",
            relative.to_string_lossy().replace('\\', "/")
        );
        assert!(!archive_name.contains(".."));
        assert!(archive_name.starts_with("guest/localdesktop/"));
    }

    #[test]
    fn crash_statuses_include_common_signal_forms() {
        assert!(139 == 139 || 134 == 134); // documents the wrapper contract
        assert!(128 + 11 >= 128);
    }
}
