# Portal user guide

Portal runs a Debian 13 desktop directly on a supported ARM64 Android device. It does not require root, a remote server, or a second computer after installation.

## Before installing

- Use an ARM64 Android tablet. Development and hardware validation currently focus on the OnePlus Pad 3.
- Keep several gigabytes of internal storage free for Debian, Plasma, applications, and updates.
- A physical keyboard and pointing device are recommended. Portal can switch between tablet and desktop input behaviour as devices are attached.
- Treat builds from `main` as development software. Back up anything important inside the Linux guest.

## First launch

Keep Portal open while it prepares the Debian runtime. The setup screen reports each stage and can export diagnostics if provisioning fails. Closing Android's activity during extraction or package installation can delay completion, but setup stages are designed to resume safely.

When setup completes, Portal starts KDE Plasma 6 on its native Wayland compositor. If Plasma fails during early startup, Portal records the failure and opens a recovery session instead of silently switching the normal desktop to X11.

## Installing software

Portal uses Debian's standard package manager:

```bash
sudo apt update
sudo apt install firefox-esr
```

Search available packages with `apt search <name>`. Graphical applications appear in Plasma's launcher after installation.

Android cannot provide Linux user namespaces to the guest. Chromium and Electron applications may therefore need `--no-sandbox`; Portal installs compatibility launchers for common packages.

## Display and input

Display size, refresh rate, rotation, and density come from Android. Plasma receives a logical desktop size and scale suitable for the attached screen. You can adjust the result under **System Settings → Display & Monitor**.

Android may intercept desktop shortcuts before they reach Linux. If keys such as `Ctrl+C`, `Ctrl+V`, or `Alt+Tab` do not arrive in Plasma, enable Portal under **Android Settings → Accessibility → Downloaded apps**. The service forwards physical key events and does not inspect window content.

When no external keyboard, mouse, or touchpad is connected, Portal can present Plasma in tablet mode and allow Android's software keyboard. Attaching desktop input switches the session back to desktop behaviour.

## Android storage

Grant **All files access** only if you want the Linux guest to work with shared Android files. Portal's own Debian runtime remains in the app's private storage.

Uninstalling Portal removes that private storage. Export important files before uninstalling, changing signing keys, or moving between incompatible builds.

## Diagnostics and recovery

Use Portal's diagnostics export before reinstalling. Reports are bounded to Portal's host logs, setup state, and guest state directory; unrelated personal files are excluded.

When reporting a problem, include:

- device model and Android version;
- Portal version and build source;
- whether the installation is fresh or upgraded;
- the shortest reliable reproduction sequence;
- the exported diagnostics archive, after reviewing it.

Open reports through the repository's [issue tracker](https://github.com/Retrorerr/Portal/issues).

## Android process limits

Android 12 and newer can stop large child-process trees. If the entire desktop disappears under load, first check whether the device exposes **Disable child process restrictions** in Developer options. System-wide ADB changes should be a last resort and are intentionally not prescribed here because availability and side effects vary by Android build.
