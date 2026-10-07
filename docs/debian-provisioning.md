# Production Debian provisioning

Portal provisions Debian 13 ARM64 into `files/runtime-B` on every fresh install.
The name is retained for compatibility with the established Android integration;
it no longer denotes an optional developer slot. Existing `arch` data and old
`active-slot` selections cannot select the production guest.

## Release inputs

`assets/debian-runtime.json` pins the public Portal GitHub release URL, image
version, compressed size and SHA-256. The current image is
`debian13-arm64-2026.10.01.1`, published in
[the runtime release](https://github.com/Retrorerr/Portal/releases/tag/runtime-debian13-arm64-2026.10.01.1).
It contains 1,165 locked Debian packages plus the pinned lfdevs Anland
KWin/XWayland stack (kwin -95 bundle + XWayland 24.1.6-91, replacing the stock
Debian KWin/XWayland payloads, which have no Anland backend). The compressed
archive is 965,676,616 bytes (921 MiB). Bundling that archive in each APK would
be impractical. Only the pinned runtime is kept published; older images are
removed once no released APK pins them, and a published asset is never
replaced in place.

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
`python scripts/publish_runtime_release.py` to validate (including the
fail-closed Anland-capability gate for every non-legacy version), publish, or
restore the release idempotently with size and SHA-256 safety guards. The
publisher writes `assets/debian-runtime.json` only after the bytes are uploaded
and the public URL verifies; a local build never retargets the manifest. Run
`python scripts/verify_runtime_release.py` before building the APK; the APK CI
also checks the public URL and GitHub asset digest. No developer rootfs is used
to produce the image or install the app.

## First launch and restart

1. Select Debian deterministically and check its exact completion identity.
2. Preflight free space (five times the compressed size plus 512 MiB, less
   reusable archive bytes). Download with three bounded attempts and explicit
   errors, resuming partial bytes when the server accepts the matching range.
3. Verify compressed size and SHA-256 before unpacking anything.
4. Extract into `runtime-B.staging`, reporting actual extracted entry counts.
5. Check Debian identity, image version and required programs, then write the
   image-ready identity inside staging. Rename only that validated staging
   tree to `runtime-B`; a partial extraction is never booted. A relaunch
   resumes or repairs it without redownloading completed work.
6. Synchronize required device/session settings, verify Mesa when Anland is
   selected, and write the final three-line installation marker last. Only
   then may native nested KWin/Plasma start.

A complete verified staging tree also recovers an interrupted promotion without
redownloading. Setup explains the required Android Developer Options setting,
“Disable child process restrictions”, and continues into Plasma automatically.
Download errors retain partial bytes for Retry. Extraction errors never make a
partial runtime launchable.

No package resolution, desktop installation, `apt upgrade`, or `pacman` command
runs during provisioning. A valid completed image is reused on subsequent
launches. A replaced runtime is retained as `runtime-B.previous`; the next
replacement rotates that single backup. Old Arch data is not deleted by setup.

The Android UI uses the native provisioning snapshot: download bytes, archive
verification, extraction, promotion, configuration and finalisation. It may
interpolate between real updates for visual continuity, but it never reports
100% or leaves the installer until the final marker has been written and
revalidated.

## Updates after installation

An installed runtime is never replaced by a newer image: `is_bootable` accepts
any completed runtime, so a new image only reaches fresh installs. Existing
installs are updated in two layers.

- **Portal's platform layer** (the lfdevs Anland KWin/XWayland packages, the
  Mesa KGSL layer, the libkwin overlay and session files) ships with the APK
  and is re-synchronised on launch. Every launch rewrites
  `/etc/apt/preferences.d/portal-platform`, which pins `kwin-common`,
  `kwin-data`, `kwin-wayland`, `kwin-x11`, `libkwin6` and `xwayland` to
  priority -1 for every Debian archive (`o=Debian`). Debian's builds have no
  Anland backend; without the pin, a Debian KWin 6.3.7 would outrank the
  installed `4:6.3.6-95` and replace it.
- **Everything else is ordinary Debian**, updated in place from trixie,
  trixie-updates and trixie-security. Portal only runs
  `apt-get upgrade --with-new-pkgs`, which never removes packages. When
  Debian moves something the pinned KWin depends on exactly (it requires
  `qt6-base-private-abi (= 6.8.2)`), apt keeps that package back instead of
  removing KWin.

Ninety seconds after the first desktop frame, at most every 12 hours and never
on a metered network, Portal runs `apt-get update` and records what an
upgrade would install in `/var/lib/localdesktop/system-updates.json`. The next
Return-to-Plasma screen offers those updates in a capsule that expands into
the package list. Choosing Update stops Plasma, runs `dpkg --configure -a`,
`apt-get update` and the upgrade with apt's machine-readable progress, checks
that no pinned package changed, and starts a fresh Plasma session on either
outcome. `/var/lib/localdesktop/system-update-in-progress` survives an
interrupted upgrade, so the next launch offers to finish it. The apt
transcript is `/var/log/portal-updates.log`. The parsing and policy are in
`src/core/system_updates.rs` and are covered by `tests/system_updates.rs`.

## Required Android integration

The APK remains authoritative for timezone, DNS, certificates, machine ID,
Firefox defaults, Konsole settings, session configuration, Android audio/IME
bridges and session directories. In particular:

- The APK installs the pinned Debian ARM64 libcanberra PulseAudio backend and
  its copyright notice. `scripts/build_canberra_backend.py` reproduces the
  module from the SHA-256-verified Debian package; the base image is unchanged.
- The session restores the Breeze splash once and waits for Portal's single
  audio bridge before starting Plasma. It does not start another audio server.

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
