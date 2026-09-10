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
