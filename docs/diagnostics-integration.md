# Diagnostics and startup contract

The Android host and PRoot guest must treat desktop readiness as a protocol,
not as a process-name heuristic.

## Required wiring

1. Expose `android::diagnostics` from `src/lib.rs` and call
   `diagnostics::initialize()` immediately after `ApplicationContext::build`.
   Wrap the Sentry/Android logger in `diagnostics::HostLogTee` so
   `diagnostics/host.log` persists even when logcat is unavailable.
2. Route every `ArchProcess` stdout/stderr line through
   `diagnostics::guest_process_line`. The guest mirror is
   `/var/lib/localdesktop/guest.log` and is included in the ZIP.
3. Emit `diagnostics::setup_stage(index, name, "start|complete|failed")`
   around each setup stage. Setup errors must send an explicit error message
   and keep the WebView on an actionable error screen.
4. Use `assets/localdesktop-startplasma.sh` (with `@UI_SCALE@` substituted)
   for the classic `dbus-run-session -- startplasma-wayland` path. It waits
   only for `/var/lib/localdesktop/plasma-ready`, which the Android host writes
   after a KWin client has connected, a toplevel has committed a buffer, and
   the host successfully swaps/presents a frame. Do not use `pgrep plasmashell`
   as readiness.
5. Install `assets/localdesktop-kwin-wrapper-v2.sh` as
   `/usr/local/bin/kwin_wayland`. Keep `LOCALDESKTOP_GDB_BACKTRACE=0` for
   normal release startup; diagnostic builds may set it to `1`. Ptrace/gdb
   denial falls back to the real KWin exactly once and SIGSEGV/SIGABRT text is
   parsed independently of gdb's exit code.
   Diagnostic provisioning also installs `gdb`/`gcc` when requested and
   builds `assets/localdesktop-crash-handler.c` as an in-process preload. The
   handler records attempt/PID, fault registers, loader maps and a best-effort
   glibc backtrace even when nested gdb cannot attach; its absence is logged
   as a non-fatal capability limitation.
6. Use `assets/localdesktop-recovery.sh` and its generated labwc autostart.
   The UI is kdialog-only (retry or view captured logs); it never launches
   Konsole automatically. Install the supplied Konsole profile files for
   normal user launches.
7. Add a WebSocket action handler for `{ "action": "export_diagnostics" }`
   in `WebviewBackend`. Call `diagnostics::export_and_share` and reply with a
   success/error message. `assets/runtime-error.html` already provides Export
   and Retry controls.
8. Prefer an in-process setup handoff: after the final stage, dismiss the
   WebView popup and send an event through the event-loop proxy. Construct the
   Wayland backend in the current activity. If an activity recreation fallback
   is unavoidable on a platform build, it must be event-triggered after this
   handoff and must not use a fixed sleep.

`src/core/startup_v2.rs` is the host-testable generation-safe readiness state
machine. A new KWin connection clears surface/buffer/frame evidence, and
out-of-order stale events are ignored. `tests/startup_readiness.rs` contains
the integration regression checks.
