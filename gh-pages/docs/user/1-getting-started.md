---
title: Getting Started
---

Welcome to Portal.

Portal provisions a Debian 13 (Trixie) userspace and starts KDE Plasma 6 as a native Wayland session on your Android device. The first launch extracts and configures the Linux runtime, so keep Portal in the foreground until setup completes.

{/* hide-in-pdf — shown only inside the in-app installer iframe; dropped from PDF manuals */}

During setup you can tap the progress area to show or hide its log. A fresh installation can take several minutes depending on storage and network speed. You can revisit this guide at [retrorerr.github.io/Portal](/).

:::info Recommended setup
Use an ARM64 tablet with a physical keyboard and, ideally, a mouse or trackpad. Portal is currently hardware-verified on the OnePlus Pad 3; other devices are experimental.
:::

{/* /hide-in-pdf */}

## How to install applications

Portal ships Debian, so software is managed with `apt`:

```bash
sudo apt update
sudo apt install firefox-esr
```

Search the Debian repositories with:

```bash
apt search <package-name>
```

Graphical applications appear in Plasma's application launcher after installation. Some Chromium/Electron applications require `--no-sandbox` under Android's userspace restrictions; Portal applies compatibility launchers for common packages.

## Display scaling

Portal detects the device display and prepares Plasma scaling automatically. To change it, open **System Settings → Display & Monitor → Display Configuration**, choose the Portal output, adjust **Global scale**, and apply the change.

If text is still too small, sign out and back into the Plasma session after changing scale.

## Physical keyboard shortcuts

Android may intercept combinations such as `Ctrl+C`, `Ctrl+V`, or `Alt+Tab`. If that happens, enable Portal's optional accessibility service under **Android Settings → Accessibility → Downloaded apps → Portal**. It forwards physical key events and does not inspect screen content.

## Disable phantom process killer

:::warning Android 12 and newer
Android may stop child processes used by a Linux desktop. If Portal shuts down unexpectedly, disable the phantom-process restriction for your device.
:::

Some devices expose **Disable child process restrictions** in Developer options. Otherwise, with USB debugging enabled, run:

```bash
adb shell "/system/bin/device_config set_sync_disabled_for_tests persistent"
adb shell "/system/bin/device_config put activity_manager max_phantom_processes 2147483647"
adb shell settings put global settings_enable_monitor_phantom_procs false
```

These are Android system settings; review and reverse them if you no longer need a Linux userspace workload.

## Getting help

If setup or Plasma fails, use Portal's diagnostics export before reinstalling—the report captures the relevant Android and guest state without including unrelated personal files. Open a report at [github.com/Retrorerr/Portal/issues](https://github.com/Retrorerr/Portal/issues) and include the device model, Android version, and reproduction steps.
