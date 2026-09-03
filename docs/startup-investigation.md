# Plasma startup investigation

This note records bounded startup observations on the native Wayland PRoot
session. It is diagnostic evidence, not a release claim.

## Baseline and portal candidate

On OnePlus Pad 3 (`f105b146`), the cold launch reached the host-presented KWin
readiness marker at roughly 11 seconds. The output title changed from disabled
to enabled within about 340 ms. The session then remained black with a cursor;
`plasma_session` and `kcminit_startup` stayed alive while `kded6`, `ksmserver`,
and `plasmashell` never appeared.

The bounded portal candidate added `QT_NO_XDG_DESKTOP_PORTAL=1` to
`assets/localdesktop-startplasma.sh`. Qt 6.11.2 recognizes this variable in
[`qdesktopunixservices.cpp`](https://raw.githubusercontent.com/qt/qtbase/v6.11.2/src/gui/platform/unix/qdesktopunixservices.cpp#L376-L383),
and the installed `libQt6Gui.so.6.11.2` contains the string. The candidate run
recorded the variable and removed the portal activation/timeout messages, but
still stalled through 320.6 seconds. Therefore portal probing is not the
startup blocker and this change must not be treated as the fix.

Evidence archive: `artifacts/qa/20260903-startup-qtportal/run-01`.

## Narrowed wait

Upstream Plasma 6.7.4 starts `kcminit_startup` before `kded6`, `ksmserver`, and
session autostart. The final candidate child was sleeping in `do_sys_poll`,
with the startup pipe still open. Its maps contained QtGui, QtDBus, and dbus,
but no `kcm_*` plugin. The kernel stack was inaccessible to the app UID, so a
user-space stack was still required.

`assets/localdesktop-kcminit-stack-probe.c` and
`assets/localdesktop-kcminit-stack-wrapper.sh` provide that diagnostic path.
`scripts/capture-kcminit-stack.ps1` backs up any existing app-private
`/usr/local/bin/kcminit_startup`, stages the probe temporarily, sends SIGUSR2
only after the probe identifies the target PID, captures PC/LR/SP, maps, and a
best-effort backtrace, then force-stops the app and restores/removes the exact
temporary paths. It does not install an APK or clear data.

The SIGUSR2 handler is intentionally separate from the production crash
handler and does not replace SIGSEGV/SIGABRT handlers.

## Decisive user-space stack

The bounded probe completed in
\`artifacts/qa/20260903-kcminit-stack/run-03\`. It identified the PRoot
\`kcminit_startup\` fork child (PID 15655, parent 15585), verified the probe
mapping and SIGUSR2 disposition, and restored every app-private target with
content/mode verification:
\`wrapper_restored=true\`, \`probe_paths_removed=verified\`, and
\`data_clear=false\`.

The child was in \`do_sys_poll\` both before and after the signal. The captured
backtrace is, in order:

\`\`\`
libc ppoll
libwayland-client wl_display_dispatch_queue_timeout
libwayland-client wl_display_dispatch_queue
libwayland-client wl_display_roundtrip_queue
QtWaylandClient::QWaylandDisplay::initialize
libqwayland
QGuiApplicationPrivate::createPlatformIntegration
QGuiApplicationPrivate::createEventDispatcher
QCoreApplicationPrivate::init
QGuiApplicationPrivate::init
QGuiApplication::QGuiApplication
kcminit_startup
\`\`\`

This narrows the measured wait to the native Wayland QPA initialization
roundtrip, before \`KCMInit\` constructs or runs any KCM plugin and before the
child registers its \`org.kde.kcminit\` D-Bus service. The upstream
[Plasma 6.7.4 kcminit source](https://raw.githubusercontent.com/KDE/plasma-workspace/v6.7.4/startkde/kcminit/main.cpp#L138-L174)
forks first, then constructs \`QGuiApplication\` in the child, matching this
ordering. It rules out a synchronous portal call or an early KCM plugin body
as the observed wait.

The stack proves that the child is waiting for a Wayland display roundtrip; it
does not by itself prove whether KWin is not servicing the socket, the nested
Wayland endpoint is miswired, or the roundtrip request is otherwise
unanswered.

## Root cause: PRoot fake_id0 socket fstat failure

1. In PRoot's `fake_id0` extension (`patches/build-proot-android/build/proot/src/extension/fake_id0/fake_id0.c:796-822`),
   `PR_fstat` and `PR_fstat64` are converted to `readlinkat` on `/proc/<pid>/fd/<fd>`.
   For socket file descriptors, the target is `"socket:[<inode>]"`.
2. While `fake_id0` explicitly replays `fstat` for paths starting with `"pipe"` or ending
   with `" (deleted)"`, it omits sockets (`socket:[...]`). It falls through to
   `fstatat64(AT_FDCWD, "socket:[...]", buf)`. The kernel attempts to stat this relative
   string as a regular file path on disk, failing with `-ENOENT`.
3. Upstream `kwin_wayland_wrapper` binds the nested Wayland socket `/tmp/wayland-1` and
   hands it to KWin via `--wayland-fd 8`. KWin calls `wl_display_add_socket_fd` in
   `libwayland-server`.
4. Disassembly of `libwayland-server.so.0` confirmed that `wl_display_add_socket_fd`
   executes `fstat64@GLIBC_2.33` on the socket descriptor to verify `S_ISSOCK`. Because
   `fstat64` returned `-1` (`errno == ENOENT`), `wl_display_add_socket_fd` returned `-1`,
   logging `kwin_core: Failed to add 8 fd to display`.
5. Consequently, KWin never registered the listening socket with its event loop. When
   `kcminit_startup` connected to `/tmp/wayland-1` and called `wl_display_roundtrip_queue`,
   KWin never called `accept()` or serviced the socket. `kcminit_startup` froze in `ppoll`
   indefinitely, and `plasma_session` never advanced to launch `ksmserver` or `plasmashell`.

## Resolution: surgical fstat/fstat64 fallback interceptor

1. Added `fstat` and `fstat64` interceptors to `assets/localdesktop-crash-handler.c`.
   When `fstat`/`fstat64` fails with `ret < 0 && errno == ENOENT`, it falls back to
   `syscall(SYS_newfstatat, fd, "", buf, AT_EMPTY_PATH)`. PRoot's `fake_id0` does not
   divert `newfstatat` on enter, allowing the Linux kernel to stat the socket descriptor
   directly. On success, `errno` is reset to 0.
2. Verified with unit regression test `crash_handler_intercepts_fstat_and_fstat64_for_proot_sockets`
   in `tests/diagnostics_assets.rs`. Full test suite passes cleanly (92 tests passed).
3. Deployed the signed ARM64 APK to OnePlus Pad 3 (`f105b146`).
4. Live execution results:
   - `kwin_core: Failed to add ... fd to display` is completely eliminated.
   - `kcminit_startup` finishes cleanly.
   - Full Plasma process tree is active (`kwin_wayland`, `plasmashell`, `ksmserver`, `kded6`, `kioworker`).
   - Genuinely rendered, interactive KDE Plasma 6 desktop verified on the 3392x2400 tablet screen:
     - `artifacts/qa/plasma-pad3-fresh.png`: Visible Plasma wallpaper, desktop icons, and bottom panel.
     - `artifacts/qa/screencap-launcher.png`: Interactive input dispatch and Wayland popup menu rendering.
