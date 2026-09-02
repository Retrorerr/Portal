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

## Remaining release gates

All items below remain unverified for the completed implementation until their
device evidence is recorded. Do not publish a fixed release or promote to main
on the strength of this ledger alone.

1. Clean install provisions (use isolated test storage/device; never wipe the
   user's existing tablet data without separate approval).
2. Setup hands off to Plasma automatically in the existing Activity.
3. KWin survives startup and sustained interaction; any crash has an attempt-
   correlated trace and identified code path.
4. A usable Plasma desktop renders, with connection, surface, committed buffer,
   and host presentation evidence tied to the same launch.
5. Closing/reopening restores Plasma reliably.
6. Rotation, resizing, and background/resume work.
7. Clipboard transfers work in both directions.
8. Touch, mouse, hardware keyboard, and software keyboard work.
9. Audio is audible and survives resume (a running audio process is insufficient).
10. Dolphin, Konsole, and Firefox launch.
11. An individual Xwayland application launches inside native Wayland Plasma.
12. Scaling is usable at the device's actual dimensions and at 2560 x 1600.
13. Genuine failure leads to understandable graphical recovery, with working
    retry and a receiver-readable single-archive diagnostic export.
14. No XFCE setup or user terminal intervention is needed.

The old APK, app-UID-only log captures, and baseline screenshot are local ignored
artifacts under `artifacts/qa/`; they are not release assets.
