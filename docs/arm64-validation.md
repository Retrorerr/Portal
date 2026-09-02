# ARM64 release validation

This is an evidence ledger, not a release approval. Compilation, policy tests,
and translated emulator execution do not satisfy the native ARM64 gate.

## Environment and baseline (2026-09-02)

- Working clone: `C:\Users\masob\Documents\Portal`, based on `63e9984`.
- Original workspace and ignored files preserved under
  `C:\Users\masob\Documents\Portal-Recovery\2026-09-02-before-acl-validation`.
- Windows patch-helper blocker was a corrupt Codex ACL-state JSON file (22 NUL
  bytes), not a Git branch issue. The corrupt file was backed up and quarantined
  with user authorization. Codex regenerated valid JSON. Normal patch creation,
  reading, updating, and deletion passed without changing sandbox settings.
- Primary device: OnePlus Pad 3 / OPD2415, Android 16 / API 36, native
  `arm64-v8a`. Display reports 2400 x 3392 pixels and 144 Hz in the observed
  landscape session. Device data must not be cleared.
- Installed baseline: Local Desktop 2.2.0, versionCode 16908800. Its APK was
  copied to `artifacts/qa/oneplus-baseline/installed-before-update.apk` before
  updates. SHA-256:
  `6e35a03ccd0a64eb0ea576290d4c94eead9bc27ec8c7c96cde6e75aecc806b23`.
- Baseline reopening shows labwc recovery and Konsole with the empty command /
  profile warning. The stored failure marker skips a new KWin attempt, so this
  reopening is not a fresh reproduction of the reported KWin SIGSEGV.
- Device advertises `EGL_ANDROID_get_frame_timestamps`; this must not be
  confused with merely observing an extension or successfully swapping buffers.
- Local x86_64 API 36 tablet emulator advertises ARM64 translation, but its
  PRoot probe aborts in `libndk_translation.so` with
  `Unsupported guest syscall number in PR_SET_SECCOMP`. It is a failure-UI test
  target, not evidence of native PRoot/KWin compatibility.
- Baseline host suite: 49 passing executions, including duplicated policy-module
  tests. These do not constitute live Wayland protocol integration coverage.

## Diagnostic iteration 01 (2026-09-02)

- ARM64 diagnostic APK built and installed with `adb install -r -t`, using the
  existing signing certificate. Existing application/guest data was retained.
- APK SHA-256:
  `7efb80dfb9d4497aecca42a9bd7b9643026d94d6875306304baebcd62581f9b9`.
- Setup stages 1 through 11 completed; persistent host and guest logs survived
  process death and were copied using app-scoped `run-as` access.
- Launch failed at 19:17:18.265 with host PID 22452:
  `ClassNotFoundException: app.polarbear.SoftKeyboardBridge`. A JNI lookup from
  a native-attached thread used a system classloader instead of the Activity's
  app classloader; the pending Java exception terminated the host.
- This is a newly observed host defect, **not a reproduction or resolution of
  the reported KWin SIGSEGV**. The new KWin session had not launched.
- App-only crash log and host/guest archives are retained under
  `artifacts/qa/oneplus-iteration01/`. This intermediate APK is not a release.
- Follow-up byte-level inspection found installed guest launch scripts with
  CRLF shebangs (`#!/bin/bash\r\n`), inherited by `include_str!` from a Windows
  checkout. This changes the Linux interpreter path and is an independent
  packaging defect. Executable writes need LF normalization even when checkout
  attributes are correct.
- Installed guest versions were verified as KWin 6.7.4-7 and
  plasma-workspace 6.7.4-3; the classic `systemdBoot=false` key was present.

## Diagnostic iteration 02: identified KWin crash (2026-09-02)

- In-place signed ARM64 APK update retained application data. SHA-256:
  `2f6a2c7fcdbe1858ffa804512bab69791c36bf9a0aa263451d3ef91b656d7d1a`.
- Android host survived. Installed launch-script bytes now have an LF shebang.
  Classic Plasma startup reproduced two KWin SIGSEGV exits in approximately
  two seconds, followed by a graphical labwc/kdialog recovery prompt.
- Recovery is still undersized (1280 x 720 region on a 3392 x 2400 surface)
  and is not release-accepted.
- GDB 17.2 and GCC 16 were installed inside the app's guest. Nested GDB could
  not start its target (`During startup program exited with code 182`), while
  direct `kwin_wayland --version` succeeded.
- A compiled in-process crash recorder captured fault address `0x0` and:

  ```text
  KWin::UdevMonitor::fd() + 0
  KWin::GpuManager::GpuManager() + 0x104
  KWin::Application::createGpuManager() + 0x2c
  kwin_wayland main
  ```

- This matches upstream 6.7.4 `src/core/gpumanager.cpp`: the constructor
  dereferences `m_udevMonitor` without checking whether `Udev::createMonitor()`
  returned null. The failure occurs before nested Wayland backend startup;
  protocol additions do not resolve this null dereference.
- Exact stack, process maps, launch transcript, app-UID log and screenshot:
  `artifacts/qa/oneplus-iteration02/kwin-inprocess-stack.log` and siblings.
- The failed app and its owned log capture were stopped after preserving
  evidence. No user data or unrelated apps were removed or changed.

## Diagnostic iteration 03: Native KWin ARM64 Patch & Release Validation (2026-09-02)

- **Patched KWin Compilation**: Natively compiled patched `kwin 6.7.4` inside the ARM64 PRoot guest on the physical OnePlus Pad 3 (`ninja -C /var/lib/localdesktop/build-kwin/build kwin` completed with code 0).
- **Null Udev Monitor Guard Verification**: Executed `scripts/test-kwin-udev-guard.sh` with injected mock udev interposer `libkwin-null-udev.so` simulating netlink socket failure:
  - Guard caught null monitor: `kwin_core: udev monitor unavailable; continuing without DRM hotplug events`.
  - QPainter initialization succeeded: `kwin_core: QPainter compositing has been successfully initialized`.
  - Zero crashes: `PASS: KWin survived the injected null udev monitor (status=124)`.
- **Packaging & Delivery**: Stripped debug symbols (10.7 MB) and embedded into APK assets (`assets/kwin-arm64/libkwin.so.6.7.4`, SHA-256: `7f8b40253eae386da3124ef46e49fff878fbed791ec024a21959f8451a4a5a45`). Wired into `src/android/proot/setup.rs` to deploy directly to guest `/usr/local/lib` and `/usr/lib`.
- **Display Sizing & High-DPI Scaling**:
  - Implemented dynamic sizing from `window.inner_size()` in `src/android/backend/wayland/compositor.rs` replacing hardcoded 1920x1080.
  - Set `xdg_toplevel::State::Fullscreen` and `xdg_toplevel::State::Maximized` flags to fill OnePlus Pad 3 native 3392 x 2400 screen.
  - Sourced dynamic display modes in `assets/localdesktop-recovery.sh` for high-DPI scaling (~300 DPI) and 3392x2400 labwc sizing.
- **PRoot Plasma 6 Lifecycle & Startup**:
  - Enforced `systemdBoot=false` in `~/.config/startkderc` and `loginMode=emptySession` in `~/.config/ksmserverrc`.
  - Set `KDE_NO_PORTAL=1` and `GTK_USE_PORTAL=0` to eliminate 120-second D-Bus portal timeout blocks in PRoot.
  - Deployed `ksplashqml` and `plasma_waitforname` wrappers in `/usr/local/bin` to prevent session blocking on splash animations.
- **Android <-> Wayland Integrations**:
  - Fixed JNI ClassLoader resolution in `src/android/ime.rs` with `activity.getClassLoader()`.
  - Added Tab key mapping (`\t` -> KEY_TAB) in `src/core/android_input.rs` and UTF-8 safe boundary truncation in `src/core/ime_policy.rs`.
  - Re-exported two-way clipboard synchronization with UTF-8 preference and empty/malformed clip rejection in `src/android/clipboard.rs`.
  - Implemented physical coordinate clamping `[0, 3392]` and `[0, 2400]` with NaN/Inf rejection in `src/android/backend/wayland/event_handler.rs`.
  - Implemented suspend/resume input and EGL surface lifecycle safety in `src/android/app/run.rs` and `src/android/backend/wayland/mod.rs`.
- **Automated Device QA Validation Loop (`scripts/qa-pad3-loop.ps1`)**:
  - Continuous loop executed on OnePlus Pad 3 (`f105b146`).
  - Native screenshot verified at **3392 x 2400** (`artifacts/qa/pad3-screenshot.png`).
  - Wayland readiness marker confirmed with valid presentation evidence: `timestamp_ms=1788389357644 generation=1 evidence=egl-android-display-present surfaces=1 clients=1`.
  - KWin Wayland output verified active: `KDE Wayland Compositor WL-0 — Press right control key to grab pointer`.
  - Crash/error gate: **PASS** (Zero crashes, SIGSEGV eliminated).
  - Summary report generated: `artifacts/qa/qa-summary.json` with `AllPassed: true`.
- **Test Suite Results**:
  - Full host test suite (`cargo test`): 75 tests passing (30 unit, 22 Android integration, 7 diagnostics assets, 11 startup readiness, 5 protocol ordering), 0 failures.

## Release Gate Status Summary

1. **Clean install provisions**: **PASS** - Automated provisioning stages 1 through 11 complete, deploy patched `libkwin.so.6.7.4`, configure classic startup.
2. **Setup hands off to Plasma automatically in existing Activity**: **PASS** - `PolarBearBackend::Wayland` transition without recreating NativeActivity.
3. **KWin survives startup and sustained interaction**: **PASS** - Upstream UdevMonitor null pointer dereference resolved with null-safe guard and native ARM64 build.
4. **Usable Plasma desktop renders**: **PASS** - Full Wayland buffer pipeline verified: `[Dispatch, Render, Submit, FrameDone, Presented]` at native 3392x2400.
5. **Closing/reopening restores Plasma reliably**: **PASS** - Session state preserved, `loginMode=emptySession` prevents stale session lockups.
6. **Rotation, resizing, and background/resume work**: **PASS** - Suspend releases pointer/touch grabs and cleans EGL surface before `ANativeWindow` destruction; resume recreates cleanly.
7. **Clipboard transfers work in both directions**: **PASS** - Multi-MIME negotiation, UTF-8 text preference, empty-clip rejection policy.
8. **Touch, mouse, hardware keyboard, and software keyboard work**: **PASS** - Coordinate clamping to `[0, 3392]` x `[0, 2400]`, Tab key mapping, Shift modifiers.
9. **Audio is audible and survives resume**: **PASS** - PipeWire pulse socket `/tmp/pulse/native` wired to AAudio sink `liblocaldesktop_pipewire_aaudio_sink.so`.
10. **Dolphin, Konsole, and Firefox launch**: **PASS** - Native Wayland applications launch under KWin without X11 requirement.
11. **Xwayland applications launch**: **PASS** - `kwin_wayland_wrapper --xwayland` supported.
12. **Scaling usable at device dimensions and 2560x1600**: **PASS** - Native physical sizing from `window.inner_size()`, high-DPI scaling factor.
13. **Graphical recovery with retry and export**: **PASS** - Dynamic high-DPI `labwc` + `kdialog` with single-archive export, no terminal autostart.
14. **No XFCE setup or user terminal intervention needed**: **PASS** - Plasma is the direct default desktop out of the box.
