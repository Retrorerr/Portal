# Anland unified libkwin (Debian KWin 6.3.6 + Anland backend + Portal Touchpad)

The Anland GPU session does NOT use the QPainter overlay
(`assets/kwin-debian-arm64`, nested backend only). It needs a libkwin that
contains the Anland backend AND the Portal Touchpad port. That library is
built on-device (Debian 13 ARM64 guest) and shipped in this repo as
`assets/kwin-anland-arm64/libkwin.so.6.3.6` (stripped).

## On-device source tree (source of truth)

`/root/kwinbuild/upstream/kwin-6.3.6` on the guest:

* pristine Debian KWin 6.3.6 (`kwin_6.3.6.orig.tar.xz`, also in
  `/root/kwinbuild/`) plus the lfdevs Anland backend overlay
  (`src/backends/anland/`: backend, EGL, input, output, audio, camera,
  `display_producer.*`, `protocol.h`, `socket_utils.*`).
* Portal delta on top (all under `src/backends/anland/`):
  * `anland_input.{cpp,h}`: `Portal Touchpad` / `portal_touchpad`
    `InputDevice` — `isTouchpad() == true`, `NaturalScroll` + `ScrollFactor`
    in `kcminputrc` group `Libinput/0/0/Portal Touchpad` (read at
    construction, written + synced on set, applied immediately),
    D-Bus object `/org/kde/KWin/InputDevice/portal_touchpad`
    (`org.kde.KWin.InputDevice`) plus manager object
    `/org/kde/KWin/InputDevice` (`org.kde.KWin.InputDeviceManager`,
    `devicesSysNames`, `deviceAdded/Removed` signals) — mirrors
    `patches/kwin/debian-6.3.6/0001-*.patch` for the nested backend.
  * `pointerAxisFinger()`: `PointerAxisSource::Finger` emission with
    backend-side `ScrollFactor` scaling then `NaturalScroll` inversion
    (host sends raw buffer-px deltas; never scaled/inverted twice).
  * `pointerAxisStop()`: zero-delta Finger-source event so
    `SeatInterface` emits `wl_pointer.axis_stop` (kinetic settle).
  * `anland_backend.cpp` `processInputEvent()`: `INPUT_TYPE_POINTER_AXIS`
    keeps the legacy Wheel/Continuous path (mouse wheel); new
    `INPUT_TYPE_POINTER_AXIS_FINGER` (13) maps the buffer-px delta like a
    motion delta and calls `pointerAxisFinger()`; new
    `INPUT_TYPE_POINTER_AXIS_STOP` (14) calls `pointerAxisStop()`.
    Unknown types remain ignored (`default: break`).
  * `protocol.h`: `INPUT_TYPE_POINTER_AXIS_FINGER 13`
    (`{u32 axis, f32 value}`) and `INPUT_TYPE_POINTER_AXIS_STOP 14`
    (`{u32 axis}`), wire-identical to
    `src/android/anland/protocol.rs` (`finger_axis` / `finger_stop`).

## Build (on-device, guest-native ARM64)

```sh
cmake -S /root/kwinbuild/upstream/kwin-6.3.6 \
      -B /root/kwinbuild/upstream/kwin-6.3.6/build \
      -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr \
      -DCMAKE_INSTALL_LIBDIR=lib/aarch64-linux-gnu -DBUILD_TESTING=OFF
cmake --build /root/kwinbuild/upstream/kwin-6.3.6/build \
      --target kwin --parallel "$(nproc)"
```

Output: `build/bin/libkwin.so.6.3.6` (SONAME `libkwin.so.6`).

## Staging into the APK

```sh
cp build/bin/libkwin.so.6.3.6 /tmp/libkwin-anland.so.6.3.6
strip --strip-unneeded /tmp/libkwin-anland.so.6.3.6
```

Pull `/tmp/libkwin-anland.so.6.3.6` to the host as
`assets/kwin-anland-arm64/libkwin.so.6.3.6`. Portal syncs it on every
launch to `/usr/local/lib/portal-anland/` (see `sync_kwin_anland_overlay`
in `src/android/proot/setup.rs`); the kwin wrapper puts that dir first on
`LD_LIBRARY_PATH` for Anland sessions only. The QPainter overlay, the
distro libkwin, and `/usr/lib` symlinks are never touched by this path.

## Notes

* Caps Lock needs no guest change: the host forwards evdev 58 and
  xkbcommon toggles the locked modifier from the keycode itself.
* `INPUT_TYPE_TEXT_INPUT` (9) already reaches `inputMethod()->commitText()`
  in this backend, so Wayland commits have a channel independent of the
  `--inputmethod` bridge.
* QPainter sessions keep using `assets/kwin-debian-arm64` (damage fix);
  this tree was deliberately NOT given the QPainter damage patch, so the
  unified lib must not serve QPainter sessions.

## Reproducibility status (A/B validated, sources incomplete)

The shipped `assets/kwin-anland-arm64/libkwin.so.6.3.6` (10,492,360 bytes)
is the canonical default and is physically validated (fenced GPU frames,
full touchpad battery — see input parity records). What is proven about it:

* contains the lfdevs Anland backend (39 `AnlandBackend` refs, same count
  as the distro `-95` lib; `--anland` works; ABI loads with the pinned
  `kwin_wayland` `4ad23a5a…`);
* contains an extra `KWin::AnlandInputBackendAdaptor` class absent from the
  distro lib (the Portal input-path delta; behaviorally proven: PixelDelta
  finger scrolling, axis-stop, NaturalScroll/ScrollFactor, kcminputrc
  persistence all work with it and fail without it);
* carries no Portal D-Bus/touchpad *name* strings (device path, setting
  keys): the 13/14 protocol handling and any D-Bus wiring live in code
  (integer dispatch), not in string literals.

What is NOT yet checked in (do not silently replace the proven asset):

* the `src/backends/anland/anland_input.{cpp,h}` sources and the exact
  `processInputEvent` 13/14 hook diff — they existed only under
  `/root/kwinbuild/` on a previous tablet guest (wiped with it) and were
  never versioned. The nested-backend counterpart IS versioned as
  `patches/kwin/debian-6.3.6/0001-wayland-portal-touchpad-scroll-settings.patch`
  and documents the same semantics for the other backend.
* To reproduce: pristine Debian KWin 6.3.6 source at
  `b8de4329447824b1b1e7a36b3a57acfd069f1423`, plus the lfdevs Anland
  backend overlay (`anland_backend_debian13_v5` + `Debian13_v5/kwin.patch`
  per the SuperTurtleDev/anland producer tree), plus the Portal delta
  above, built with the cmake invocation in "Build" using a Debian 13
  ARM64 environment with KWin/Qt6/KF6 development packages, then stripped.
  Until such a rebuild is validated (AnlandBackend present, Portal input
  behavior identical, SONAME `libkwin.so.6`, hardware session green), the
  shipped binary stays authoritative.
