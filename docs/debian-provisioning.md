# Production Debian provisioning

Portal provisions Debian 13 ARM64 into `files/runtime-B` on every fresh install.
The name is retained for compatibility with the established Android integration;
it no longer denotes an optional developer slot. Existing `arch` data and old
`active-slot` selections cannot select the production guest.

## Release inputs

`assets/debian-runtime.json` pins the public Portal GitHub release URL, image
version, compressed size and SHA-256. The current image is
`debian13-arm64-2026.09.05.2`, published in
[the runtime release](https://github.com/Retrorerr/Portal/releases/tag/runtime-debian13-arm64-2026.09.05.2).
It contains 986 locked Debian packages. The compressed archive is 823,512,456
bytes; its final regular-file payload is 2,575,636,513 bytes, before filesystem
allocation overhead. Bundling that archive in each APK would be impractical.

Build the image from the existing Debian builder, not a copied guest directory:

```text
python scripts/package_debian_runtime.py
```

`assets/debian-runtime-packages.json` pins package filenames, versions and hashes.
Only use `--refresh-lock` when deliberately selecting new Debian package versions.
Package download and extraction failures abort the build. Payloads travel directly
from Debian archives into the release tar, preserving Linux modes and links on
Windows as well as Linux. Canonical links cannot replace directories containing
real package payloads. The v2 image corrects that conflict for `usr/lib/ssl`.

For a new image, change the image version in the packager, build, publish its
archive under the corresponding `runtime-<version>` GitHub release, and commit
the generated manifest. Do not replace assets under an existing version. Run
`python scripts/verify_runtime_release.py` before building the APK; the APK CI
also checks the public URL and GitHub asset digest. No developer rootfs is used
to produce the image or install the app.

## First launch and restart

1. Select Debian deterministically and check its exact completion identity.
2. Download the pinned archive with three bounded attempts and explicit errors.
3. Verify compressed size and SHA-256 before unpacking anything.
4. Extract into `runtime-B.staging`, reporting actual extracted entry counts.
5. Check Debian identity, image version and required programs. Write the
   completion identity only after successful extraction, then rename staging
   to `runtime-B`. A partial extraction is never booted. A relaunch retries it.
6. Synchronize device/session settings and launch native nested KWin/Plasma.

No package resolution, desktop installation, `apt upgrade`, or `pacman` command
runs during provisioning. A valid completed image is reused on subsequent
launches. A replaced runtime is retained as `runtime-B.previous`; the next
replacement rotates that single backup. Old Arch data is not deleted by setup.

The Android UI uses an indeterminate progress bar with actual byte/entry counts,
then reports configuration and startup. It no longer promises a duration or
uses setup-stage count as an extraction percentage.

## Required Android integration

The APK remains authoritative for timezone, DNS, certificates, machine ID,
Firefox defaults, Konsole settings, session configuration, Android audio/IME
bridges and session directories. In particular:

- The host `XKB_CONFIG_ROOT` and `XLOCALEDIR` point to Debian before keyboard
  initialization. The older bundled host library has an Arch build-time default.
- `tmp/.X11-unix` and `tmp/.ICE-unix` are created by Portal because systemd-tmpfiles
  is absent. Debian's session manager needs nested Xwayland even in a Wayland session.
- `assets/guest-arm64/localdesktop-crash-handler.so` contains the existing PRoot
  socket `fstat` workaround. It is bundled and installed atomically by every APK;
  guest `gcc` is no longer needed. KWin otherwise fails to register its inherited
  Wayland socket and the session remains black.

Rebuild that small glibc ARM64 support library from its existing C source using:

```text
python scripts/build_guest_support.py --clang /path/to/clang
```

NDK clang works on Windows. The helper verifies pinned Debian header packages
from `assets/guest-support-headers.json`; it does not link Android bionic into
the guest library. Keep the source, header lock and generated library together.

## Focused validation

```text
cargo test --test debian_provisioning --test diagnostics_assets --test startup_portal --test startup_readiness
python scripts/verify_runtime_release.py
python scripts/verify_debian_device.py --serial <authorized-adb-serial>
```

The device verifier discovers the single authorized device if no serial is
supplied, and fails on zero/multiple devices. It checks identity, dpkg queries,
absence of Arch/pacman, the completion marker and desktop processes. It does not
install packages or repair guest configuration. A presented KWin frame alone can
be black; physical acceptance additionally requires a visible desktop/panel and
one application launch.

Remaining Arch names are limited to compatibility/diagnostic slot descriptions,
the legacy `ArchProcess` wrapper (which executes the production runtime), older
developer tools and historical build documentation. None selects, downloads or
installs Arch during production setup.

## September 5 clean-install evidence

The OnePlus Pad 3 acceptance run installed only the APK after a full uninstall,
with absence of old runtime directories checked before launch. Portal downloaded
release `debian13-arm64-2026.09.05.2` itself. Preserved host logs record setup
completion after 113.158 seconds and the first Android-presented Plasma frame
after 126.982 seconds. No rootfs was pushed and no guest repair was performed.
The same APK had already displayed the desktop and panel before this clean run;
Debian 13, dpkg 1.22.22, the curated packages and absence of pacman were checked.
The focused suite passed 34 tests. No additional keyboard or broad Plasma QA was
performed. Later device activity created an Arch directory, so the later device
snapshot is not evidence of pristine app data; it was left untouched.
