<p align="center">
  <img src="assets/portal-icon.svg" width="112" alt="Portal" />
</p>

<h1 align="center">Portal</h1>

<p align="center">
  Debian 13 and KDE Plasma 6 on Android, without root or a remote machine.
</p>

<p align="center">
  <a href="https://github.com/Retrorerr/Portal/actions/workflows/build.yml"><img alt="Android build" src="https://github.com/Retrorerr/Portal/actions/workflows/build.yml/badge.svg" /></a>
  <a href="LICENSE"><img alt="GPL-3.0" src="https://img.shields.io/badge/license-GPL--3.0-6f78ff" /></a>
  <img alt="ARM64" src="https://img.shields.io/badge/Android-ARM64-43d7f2" />
</p>

Portal is an Android-native host for a local Debian desktop. A Rust/Smithay compositor presents a Wayland display to KDE Plasma inside PRoot, while Android remains responsible for the physical display, lifecycle, input devices, clipboard, audio, and shared storage.

This is development software. The current hardware target is the **OnePlus Pad 3**; other ARM64 devices should be treated as unverified until they pass the same device checks.

## Current scope

| Area | Current path |
| --- | --- |
| Linux guest | Debian GNU/Linux 13 (Trixie) |
| Desktop | KDE Plasma 6 on native Wayland |
| Host | Rust, Smithay, Android NativeActivity |
| Runtime | Rootless PRoot with recoverable A/B slots |
| Audio | PipeWire/Pulse compatibility over Android AAudio |
| Input | Touch, pointer, physical keyboard, Android IME, clipboard |
| Supported ABI | `arm64-v8a` |

Portal does not stream a desktop from another computer and does not place VNC or X11 between Plasma and the Android surface. Xwayland remains available only for Linux applications that still require X11.

## How it fits together

```text
Android activity and hardware
        │
        ├── lifecycle · display · input · clipboard · audio · files
        │
Portal host (Rust / Smithay)
        │ native Wayland
Debian 13 guest (PRoot)
        │
KDE Plasma 6 and Linux applications
```

The setup pipeline is restartable. Runtime state is versioned in two slots, and an early Plasma failure enters an explicit native-Wayland recovery path rather than changing the normal session behind the user's back.

## Build

The release build uses the vendored Android-aware `xbuild` toolchain:

```bash
git clone https://github.com/Retrorerr/Portal.git
cd Portal
cargo install --path patches/xbuild/xbuild --force
x build --release --platform android --arch arm64 --format apk
```

The APK is written to `target/x/release/android/localdesktop.apk`.

An on-device Termux build is also supported:

```bash
bash scripts/build-termux.sh
```

See [the user guide](docs/user-guide.md) for installation and recovery notes and [the validation guide](docs/arm64-validation.md) for the hardware acceptance process.

## Development checks

Run the host suite before producing an APK:

```bash
cargo fmt --all -- --check
cargo test --tests
```

Shell and Python assets should also receive their language-level syntax checks. A release candidate is not considered validated until its package identity, signature, 16 KiB alignment, install/upgrade behaviour, launch, logs, and real ARM64 Plasma session have been checked.

Useful engineering references:

- [Runtime and compositor architecture](docs/architecture.md)
- [ARM64 validation matrix](docs/arm64-validation.md)
- [Diagnostics integration](docs/diagnostics-integration.md)
- [Startup investigation notes](docs/startup-investigation.md)

## Stable compatibility identifiers

The product name is Portal, but several internal identifiers deliberately retain their original values:

- Android package: `app.polarbear`
- Rust crate and native library: `localdesktop`
- guest configuration root: `/etc/localdesktop`
- build artifact basename: `localdesktop.apk`

Changing them would break Android upgrades or existing guest installations. They are compatibility boundaries, not unfinished branding.

## Contributing and security

Start with [CONTRIBUTING.md](CONTRIBUTING.md). Please use the issue templates and include evidence appropriate to the layer being diagnosed; emulator package tests do not establish that an ARM64 Debian/Plasma session works on physical hardware.

For sensitive reports, follow [SECURITY.md](SECURITY.md) rather than opening a public issue.

## Lineage

Portal is an independent continuation of [Local Desktop](https://github.com/localdesktop/localdesktop.github.io). The original commit history and GPL-3.0 licensing are retained for attribution and traceability.
