const KWIN_WRAPPER: &str = include_str!("../assets/localdesktop-kwin-wrapper-v2.sh");
const PLASMA_LAUNCHER: &str = include_str!("../assets/localdesktop-startplasma.sh");
const RECOVERY: &str = include_str!("../assets/localdesktop-recovery.sh");
const RETRY: &str = include_str!("../assets/localdesktop-retry-plasma.sh");
const KONSOLE_PROFILE: &str = include_str!("../assets/konsole/LocalDesktop.profile");
const CRASH_HANDLER: &str = include_str!("../assets/localdesktop-crash-handler.c");
const SETUP: &str = include_str!("../src/android/proot/setup.rs");
const DIAGNOSTICS: &str = include_str!("../src/android/diagnostics.rs");
const DIAGNOSTICS_DOC: &str = include_str!("../docs/diagnostics-integration.md");
const GIT_ATTRIBUTES: &str = include_str!("../.gitattributes");
const SETUP_PAGE: &str = include_str!("../assets/setup-progress-v2.html");
const ERROR_PAGE: &str = include_str!("../assets/runtime-error.html");
const ANDROID_MAIN: &str = include_str!("../src/android/main.rs");

#[test]
fn kwin_wrapper_does_not_make_gdb_a_release_requirement() {
    assert!(KWIN_WRAPPER.contains("LOCALDESKTOP_GDB_BACKTRACE:-0"));
    assert!(KWIN_WRAPPER.contains("run_normally=1"));
    assert!(KWIN_WRAPPER.contains("ptrace"));
    assert!(KWIN_WRAPPER.contains("status=139"));
    assert!(KWIN_WRAPPER.contains("crash-summary"));
    assert!(KWIN_WRAPPER.contains("Do not remove it here"));
    assert!(KWIN_WRAPPER.contains("libSegFault"));
    assert!(KWIN_WRAPPER.contains("localdesktop-crash-handler.so"));
    assert!(KWIN_WRAPPER.contains("LOCALDESKTOP_CRASH_LOG"));
    assert!(KWIN_WRAPPER.contains("during startup program exited"));
    assert!(KWIN_WRAPPER.contains("tee -a \"$log_file\""));
}

#[test]
fn crash_handler_captures_registers_before_unwinding() {
    assert!(CRASH_HANDLER.contains("sigaction"));
    assert!(CRASH_HANDLER.contains("uc_mcontext"));
    assert!(CRASH_HANDLER.contains("fault_address"));
    assert!(CRASH_HANDLER.contains("write_pointer(fd, \"pc\""));
    assert!(CRASH_HANDLER.contains("write_maps(fd)"));
    assert!(CRASH_HANDLER.contains("backtrace_symbols_fd"));
    assert!(CRASH_HANDLER.contains("LOCALDESKTOP_ATTEMPT_ID"));
    assert!(CRASH_HANDLER.contains("localdesktop-crash-handler-start"));
    assert!(CRASH_HANDLER.contains("readlink(\"/proc/self/exe\""));
    assert!(CRASH_HANDLER.contains("sigprocmask(SIG_UNBLOCK"));
    assert!(CRASH_HANDLER.contains("pid="));
}

#[test]
fn crash_handler_intercepts_fstat_and_fstat64_for_proot_sockets() {
    assert!(CRASH_HANDLER.contains("int fstat(int fd, struct stat *buf)"));
    assert!(CRASH_HANDLER.contains("int fstat64(int fd, struct stat64 *buf)"));
    assert!(CRASH_HANDLER.contains("dlsym(RTLD_NEXT, \"fstat\")"));
    assert!(CRASH_HANDLER.contains("dlsym(RTLD_NEXT, \"fstat64\")"));
    assert!(CRASH_HANDLER.contains("AT_EMPTY_PATH"));
    assert!(CRASH_HANDLER.contains("ret < 0 && errno == ENOENT"));
}

#[test]
fn plasma_launcher_waits_for_host_presented_marker() {
    assert!(PLASMA_LAUNCHER.contains("plasma-ready"));
    assert!(PLASMA_LAUNCHER.contains("dbus-run-session -- /usr/bin/startplasma-wayland"));
    assert!(PLASMA_LAUNCHER.contains("KDE_USE_SYSTEMD=0"));
    assert!(PLASMA_LAUNCHER.contains("systemdBoot false"));
    assert!(PLASMA_LAUNCHER.contains("loginMode emptySession"));
    assert!(!PLASMA_LAUNCHER.contains("pgrep plasmashell"));
    assert!(
        PLASMA_LAUNCHER.contains("rm -f \"$ready_marker\" \"$failure_marker\" \"$crash_marker\"")
    );
    assert!(PLASMA_LAUNCHER.contains("attempt=$attempt_id"));
    assert!(PLASMA_LAUNCHER.contains("WAYLAND_DEBUG=${WAYLAND_DEBUG:-1}"));
    assert!(PLASMA_LAUNCHER.contains("stage=backend compositor=kwin_wayland"));
    assert!(PLASMA_LAUNCHER.contains("package in kwin plasma-workspace"));
    assert!(PLASMA_LAUNCHER.contains("signal_tree \"$session_pid\" KILL"));
    assert!(PLASMA_LAUNCHER
        .contains("LOCALDESKTOP_GDB_BACKTRACE=${LOCALDESKTOP_GDB_BACKTRACE:-@GDB_BACKTRACE@}"));
}

#[test]
fn kwin_wrapper_disables_guest_screenlocker() {
    assert!(KWIN_WRAPPER.contains("/usr/bin/kwin_wayland --no-lockscreen"));
}

#[test]
fn plasma_launcher_and_setup_disable_guest_screenlocker() {
    assert!(PLASMA_LAUNCHER.contains("action/lock_screen"));
    assert!(PLASMA_LAUNCHER.contains("Autolock false"));
    assert!(SETUP.contains("action/lock_screen"));
    assert!(SETUP.contains("kscreenlockerrc"));
}

#[test]
fn plasma_launcher_and_setup_do_not_force_scale_1() {
    assert!(!PLASMA_LAUNCHER.contains("\"scale\": 1"));
    assert!(!PLASMA_LAUNCHER.contains("QT_SCALE_FACTOR"));
}

#[test]
fn recovery_is_graphical_and_never_autostarts_a_terminal() {
    assert!(RECOVERY.contains("kdialog"));
    assert!(!RECOVERY.contains("konsole"));
    assert!(!RECOVERY.contains("pkill"));
    assert!(RETRY.contains("labwc.pid"));
    assert!(!RETRY.contains("pkill"));
    assert!(RECOVERY.contains("while true; do"));
    assert!(RECOVERY.contains("output_mode"));
    assert!(RECOVERY.contains("output_scale"));
    assert!(RECOVERY.contains("QT_SCALE_FACTOR"));
    assert!(RECOVERY.contains("<mode>${output_mode}</mode>"));
    assert!(RECOVERY.contains("<scale>${output_scale}</scale>"));
}

#[test]
fn setup_installs_versioned_classic_startup_assets_and_profile() {
    assert!(SETUP.contains("startplasma-localdesktop"));
    assert!(SETUP.contains("kwin_wayland"));
    assert!(SETUP.contains("start-localdesktop-recovery"));
    assert!(SETUP.contains("localdesktop-retry-plasma"));
    assert!(SETUP.contains("setup_with_completion"));
    assert!(SETUP.contains("systemdBoot"));
    assert!(SETUP.contains("@GDB_BACKTRACE@"));
    assert!(SETUP.contains("require_gdb"));
    assert!(SETUP.contains("command -v gcc"));
    assert!(SETUP.contains("localdesktop-crash-handler.c"));
    assert!(SETUP.contains("localdesktop-crash-handler.so"));
    assert!(SETUP.contains("localdesktop-crash-handler.so.tmp"));
    assert!(SETUP.contains("command -v gdb"));
    assert!(SETUP.contains(".config/konsolerc"));
    assert!(SETUP.contains(".local/share/konsole"));
    assert!(KONSOLE_PROFILE.contains("Command=/bin/bash"));
    assert!(KONSOLE_PROFILE.contains("Directory=@HOME@"));
    assert!(SETUP.contains("fn normalize_guest_text"));
    assert!(SETUP.contains("replace(\"\\r\\n\", \"\\n\")"));
    assert!(SETUP.contains("replace('\\r', \"\\n\")"));
    assert!(GIT_ATTRIBUTES.contains("*.sh text eol=lf"));
}

#[test]
fn diagnostics_export_keeps_rotated_logs_and_guest_absence_metadata() {
    assert!(DIAGNOSTICS.contains("guest_state_status"));
    assert!(DIAGNOSTICS.contains("guest_state={guest_state_status}"));
    assert!(DIAGNOSTICS.contains("rotated_log_path"));
    assert!(DIAGNOSTICS.contains("host/host.log.1"));
    assert!(DIAGNOSTICS.contains("host/guest.log.1"));
    assert!(DIAGNOSTICS.contains("mark_plasma_frame_presented_for_generation"));
}

#[test]
fn diagnostics_export_uses_scoped_content_grants_and_cleans_up_failures() {
    let share_file = DIAGNOSTICS
        .split_once("fn share_file")
        .map(|(_, body)| body)
        .expect("share_file implementation is present");
    assert!(share_file.contains("Build$VERSION"));
    assert!(share_file.contains("MediaStore$Downloads"));
    assert!(share_file.contains("copy_archive_to_content_uri"));
    assert!(share_file.contains("is_pending"));
    assert!(share_file.contains("setClipData"));
    assert!(share_file.contains("FLAG_GRANT_READ_URI_PERMISSION"));
    assert!(share_file.contains("delete_content_uri"));
    assert!(!DIAGNOSTICS.contains("StrictMode"));
    assert!(!DIAGNOSTICS.contains("file://"));
    assert!(!DIAGNOSTICS_DOC.contains("Sentry"));
    assert!(DIAGNOSTICS_DOC.contains("MediaStore"));
    assert!(DIAGNOSTICS_DOC.contains("API 29"));
}

#[test]
fn setup_and_error_pages_offer_one_tap_export() {
    assert!(SETUP_PAGE.contains("Export diagnostics"));
    assert!(SETUP_PAGE.contains("export_diagnostics"));
    assert!(ERROR_PAGE.contains("Export diagnostics"));
    assert!(ERROR_PAGE.contains("export_diagnostics"));
}

#[test]
fn android_logging_is_local_and_info_by_default() {
    assert!(!ANDROID_MAIN.contains("sentry::init"));
    assert!(!ANDROID_MAIN.contains("SentryLogger"));
    assert!(ANDROID_MAIN.contains("android_logger::AndroidLogger::default()"));
    assert!(ANDROID_MAIN.contains("let log_level = log::LevelFilter::Info;"));
    assert!(!ANDROID_MAIN.contains("LevelFilter::Debug"));
    assert!(!ANDROID_MAIN.contains("LevelFilter::Trace"));
}
