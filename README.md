<p align="center">
  <img src="assets/portal-icon.png" width="144" alt="Portal icon" />
</p>

<h1 align="center">Portal</h1>

<p align="center">
  <strong>A real Debian 13 + KDE Plasma 6 workspace, running rootlessly on Android.</strong>
</p>

<p align="center">
  <a href="https://github.com/Retrorerr/Portal/actions/workflows/build.yml"><img alt="Build" src="https://github.com/Retrorerr/Portal/actions/workflows/build.yml/badge.svg" /></a>
  <a href="LICENSE"><img alt="GPL-3.0" src="https://img.shields.io/badge/license-GPL--3.0-6d7cff" /></a>
  <img alt="Android ARM64" src="https://img.shields.io/badge/Android-ARM64-52e4ff" />
</p>

Portal turns a capable Android tablet into a self-contained Linux workstation. It boots a Debian userspace, launches KDE Plasma on native Wayland, and bridges the Android hardware, lifecycle, input, clipboard, audio, storage, and display into the desktop session—all without root access or a remote computer.

> [!IMPORTANT]
> Portal is currently developed and hardware-verified on the **OnePlus Pad 3 (ARM64)**. Other devices are experimental. A large display and physical keyboard are strongly recommended.

## What makes Portal different

- **Native Wayland:** Plasma renders through Portal's Rust compositor rather than an X server, VNC stream, or cloud VM.
- **A complete Debian desktop:** Debian 13 (Trixie), KDE Plasma 6, desktop applications, package management, and shell access live on-device.
- **Rootless:** the Android device does not need to be unlocked, rooted, or flashed.
- **Tablet-aware:** high-density display scaling, physical keyboard forwarding, pointer input, Android clipboard integration, and AAudio are first-class paths.
- **Recoverable:** A/B runtime slots, diagnostics export, and a native-Wayland recovery session make failures inspectable and reversible.
- **One app:** setup, runtime management, compositor, and Android integration ship together.

## Architecture

```text
┌──────────────────────────────── Android ────────────────────────────────┐
│  Activity · lifecycle · touch · keyboard · clipboard · AAudio · files  │
│                                │                                       │
│                    Portal host (Rust / Smithay)                         │
│                                │ native Wayland                        │
│             Debian 13 rootfs → KDE Plasma 6 → Linux apps              │
└────────────────────────────────────────────────────────────────────────┘
```

The Linux guest is isolated with PRoot and managed in two runtime slots. Portal's compositor is the display boundary; the guest remains a standard Debian environment above it.

## Build

The supported release target is Android ARM64. From a configured Linux, WSL, or Termux environment:

```bash
git clone https://github.com/Retrorerr/Portal.git
cd Portal
cargo install --path patches/xbuild/xbuild --force
x build --release --platform android --arch arm64 --format apk
```

The release APK is written to:

```text
target/x/release/android/localdesktop.apk
```

The internal crate, APK filename, Android package (`app.polarbear`), and guest paths under `/etc/localdesktop` intentionally retain their historical identifiers. Keeping them stable preserves in-place Android upgrades and existing guest data while the product name is Portal.

For toolchain details and on-device builds, see the [developer guide](https://retrorerr.github.io/Portal/docs/developer/how-to-build).

## Verification

```bash
cargo test --tests
cargo fmt --all -- --check
```

For a device build, also verify the APK signature, 16 KiB zip alignment, package identity, install, launch, and Android logs. A green host test suite is not a substitute for real ARM64 hardware validation.

## Project status

- Debian GNU/Linux 13 (Trixie)
- KDE Plasma 6 native-Wayland session
- Android 5.0+ API compatibility target; current validation focuses on modern ARM64 tablets
- 107 Rust host tests in the current consolidated tree
- Active development; releases may change runtime images and device compatibility

## Lineage and license

Portal began as a substantially modified continuation of [Local Desktop](https://github.com/localdesktop/localdesktop.github.io). Its commit history is retained for authorship and traceability. New Portal-specific development is maintained independently in this repository.

Licensed under [GPL-3.0](LICENSE). Contributions and issue reports are welcome.
