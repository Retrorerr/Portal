const KWIN_WRAPPER: &str = include_str!("../assets/localdesktop-kwin-wrapper-v2.sh");
const PLASMA_LAUNCHER: &str = include_str!("../assets/localdesktop-startplasma.sh");
const RECOVERY: &str = include_str!("../assets/localdesktop-recovery.sh");
const RETRY: &str = include_str!("../assets/localdesktop-retry-plasma.sh");
const KONSOLE_PROFILE: &str = include_str!("../assets/konsole/LocalDesktop.profile");
const PROOT_RUNTIME: &str = include_str!("../src/android/runtime/proot.rs");
const CRASH_HANDLER: &str = include_str!("../assets/localdesktop-crash-handler.c");
const SETUP: &str = include_str!("../src/android/proot/setup.rs");
const DIAGNOSTICS: &str = include_str!("../src/android/diagnostics.rs");
const DIAGNOSTICS_DOC: &str = include_str!("../docs/diagnostics-integration.md");
const GIT_ATTRIBUTES: &str = include_str!("../.gitattributes");
const SETUP_PAGE: &str = include_str!("../assets/setup-progress-v2.html");
const ERROR_PAGE: &str = include_str!("../assets/runtime-error.html");
const ANDROID_MAIN: &str = include_str!("../src/android/main.rs");
const DRMSHIM_SOURCE: &str = include_str!("../assets/guest-arm64/drmshim.c");
const DRMSHIM_BINARY: &[u8] = include_bytes!("../assets/guest-arm64/drmshim.so");

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
    assert!(PLASMA_LAUNCHER.contains("WAYLAND_DEBUG=${WAYLAND_DEBUG:-0}"));
    assert!(PLASMA_LAUNCHER.contains("stage=backend compositor=kwin_wayland"));
    assert!(PLASMA_LAUNCHER.contains("package in kwin-wayland plasma-workspace"));
    assert!(PLASMA_LAUNCHER.contains("signal_tree \"$session_pid\" KILL"));
    assert!(PLASMA_LAUNCHER
        .contains("LOCALDESKTOP_GDB_BACKTRACE=${LOCALDESKTOP_GDB_BACKTRACE:-@GDB_BACKTRACE@}"));
}

#[test]
fn plasma_launcher_leaves_konsole_profile_selection_to_provisioning() {
    assert!(!PLASMA_LAUNCHER.contains("Profile 1.profile"));
    assert!(!PLASMA_LAUNCHER.contains("DefaultProfile"));
}

#[test]
fn proot_starts_guest_processes_with_debian_shell_defaults() {
    assert!(PROOT_RUNTIME.contains(".arg(\"-w\")"));
    assert!(PROOT_RUNTIME.contains("let working_dir"));
    assert!(PROOT_RUNTIME.contains("SHELL=/bin/bash"));
    assert!(PROOT_RUNTIME.contains(
        "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/local/games:/usr/games"
    ));
    assert!(!PROOT_RUNTIME.contains("/system/bin:/system/xbin"));
}

#[test]
fn electron_desktop_entries_receive_proot_safe_startup_flags() {
    assert!(SETUP.contains("resources/app.asar"));
    assert!(SETUP.contains("--no-sandbox"));
    assert!(SETUP.contains("--no-stdio-init"));
    assert!(SETUP.contains("--ozone-platform=wayland"));
    assert!(SETUP.contains("99portal-desktop-integration"));
    assert!(SETUP.contains("desktop-integration.log"));
    assert!(SETUP.contains("$dst.portal-tmp.$$"));
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
    assert!(SETUP.contains("CRASH_HANDLER_BINARY"));
    assert!(!SETUP.contains("command -v gcc"));
    assert!(SETUP.contains("localdesktop-crash-handler.c"));
    assert!(SETUP.contains("localdesktop-crash-handler.so"));
    assert!(SETUP.contains("handler.with_extension(\"so.tmp\")"));
    assert!(SETUP.contains("fs::rename(&temporary, &handler)"));
    assert!(SETUP.contains("join(\"konsolerc\")"));
    assert!(SETUP.contains(".local/share/konsole"));
    assert!(SETUP.contains("migrate_konsole_profile"));
    assert!(SETUP.contains("konsole-profile-v2"));
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
fn drmshim_reports_version_with_correct_drm_version_layout() {
    // struct drm_version is INTERLEAVED (len+pointer per field). A grouped
    // layout keeps the 64-byte size (ioctl number still matches) but
    // misplaces every field after name_len, so Mesa's version query fails
    // and KWin dies at "Failed to create gbm device" (proven on-device).
    assert!(DRMSHIM_SOURCE.contains("size_t name_len;\n    char *name;"));
    assert!(DRMSHIM_SOURCE.contains("size_t date_len;\n    char *date;"));
    assert!(DRMSHIM_SOURCE.contains("size_t desc_len;\n    char *desc;"));
    assert!(DRMSHIM_SOURCE.contains(
        "_Static_assert(sizeof(struct drm_version) == 64"
    ));
    assert!(DRMSHIM_SOURCE.contains("KGSL-backed DRM node (portal-shimmed)"));
    // close() must stay uninterposed: a raw-SVC close replacement breaks
    // processes under PRoot (proven by bisect: EFAULT after successful
    // reads with close-only interposition).
    assert!(!DRMSHIM_SOURCE.contains("int close("));
    assert!(DRMSHIM_SOURCE.contains("close() is deliberately"));
    assert!(DRMSHIM_SOURCE.contains("register long r0 __asm__(\"x0\")"));
    // KGSL-backed render node: the fake fd is a real /dev/kgsl-3d0 open so
    // the kgsl winsys probe succeeds against real hardware; the reported
    // DRM version name steers Mesa away from the MSM winsys (whose GEM
    // ioctls need an unobtainable real render node). Dups of fake fds stay
    // fake (Mesa dups via fcntl64); non-DRM ioctls pass through to the real
    // device instead of failing.
    assert!(DRMSHIM_SOURCE.contains("/dev/kgsl-3d0"));
    assert!(DRMSHIM_SOURCE.contains("version_name[] = \"kgsl\""));
    assert!(DRMSHIM_SOURCE.contains("int fcntl64("));
    assert!(DRMSHIM_SOURCE.contains("int dup3("));
    assert!(DRMSHIM_SOURCE.contains("int openat64("));
    // The staged binary matches the source contract: AArch64 shared object
    // exporting exactly open/open64/openat/openat64/ioctl/dup/fcntl-family
    // (never close).
    assert_eq!(&DRMSHIM_BINARY[..4], b"\x7fELF");
    assert_eq!(u16::from_le_bytes([DRMSHIM_BINARY[18], DRMSHIM_BINARY[19]]), 183);
    for symbol in [
        b"\x00open\x00".as_slice(),
        b"\x00open64\x00".as_slice(),
        b"\x00openat\x00".as_slice(),
        b"\x00openat64\x00".as_slice(),
        b"\x00ioctl\x00".as_slice(),
        b"\x00dup\x00".as_slice(),
        b"\x00dup3\x00".as_slice(),
        b"\x00fcntl\x00".as_slice(),
        b"\x00fcntl64\x00".as_slice(),
    ] {
        assert!(
            DRMSHIM_BINARY.windows(symbol.len()).any(|w| w == symbol),
            "staged drmshim.so must export {}",
            String::from_utf8_lossy(symbol)
        );
    }
    assert!(
        !DRMSHIM_BINARY
            .windows(7)
            .any(|w| w == b"\x00close\x00"),
        "staged drmshim.so must not interpose close()"
    );
    assert!(SETUP.contains("DRMSHIM_BINARY"));
}

#[test]
fn android_logging_is_local_and_warn_in_release() {
    assert!(!ANDROID_MAIN.contains("sentry::init"));
    assert!(!ANDROID_MAIN.contains("SentryLogger"));
    assert!(ANDROID_MAIN.contains("android_logger::AndroidLogger::default()"));
    assert!(ANDROID_MAIN.contains("log::LevelFilter::Warn"));
    assert!(ANDROID_MAIN.contains("log::LevelFilter::Info"));
    assert!(ANDROID_MAIN.contains("cfg!(debug_assertions)"));
    assert!(!ANDROID_MAIN.contains("LevelFilter::Debug"));
    assert!(!ANDROID_MAIN.contains("LevelFilter::Trace"));
}
