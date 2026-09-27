# Anland libkwin (KWin 6.3.6 + Anland backend + Portal patches)

`assets/kwin-anland-arm64/libkwin.so.6.3.6` is the KWin library Portal uses
for Anland GPU sessions. It is built from source by
`.github/workflows/kwin-anland-arm64.yml`, which runs `build.sh` in this
directory inside Debian 13 (trixie) on a native ARM64 runner.

## Source

| Input | Pin |
| --- | --- |
| Debian KWin 6.3.6 | `kwin_6.3.6.orig.tar.xz`, SHA-256 `27f2205f06d58f1d1f480d2a94ae24022c2f95b9c1fdc5a549f8e143713fce12` |
| lfdevs Debian packaging with the Anland backend | [lfdevs/kwin](https://github.com/lfdevs/kwin) tag `anland-5.13-debian-4_6.3.6-95`, commit `beea4c3d22f08100b1b3acda1bd502e87bb1a347` |
| Portal patches | `0001-*.patch` to `0004-*.patch` here, appended to the lfdevs quilt series |

The lfdevs tag is the source of the `kwin` 4:6.3.6-95 packages installed in
Portal's guest (lfdevs/anland-termux release 5.13.3, pinned in
`scripts/build_debian_rootfs.py`), so the library differs from the guest's own
`libkwin.so.6.3.6` only by Portal's patches:

* `0001` honours `ANLAND_DISABLE_AUDIO` (backport of upstream Anland 9f3ae3c).
  Portal sets it because audio goes through its own PipeWire/AAudio bridge;
  without it KWin creates an `anland-speaker` sink that nothing plays, which
  WirePlumber can choose as the default sink.
* `0002` adds the Portal Touchpad: finger-source smooth scrolling
  (`INPUT_TYPE_POINTER_AXIS_FINGER` 13, `INPUT_TYPE_POINTER_AXIS_STOP` 14,
  wire-identical to `src/android/anland/protocol.rs`) with NaturalScroll and
  ScrollFactor from `kcminputrc` group `Libinput/0/0/Portal Touchpad`, exported
  over D-Bus so Plasma's touchpad settings can change them.
* `0003` adds the Portal Stylus, a zwp_tablet_v2 tool with pressure, tilt,
  hover distance and the eraser (`INPUT_TYPE_TABLET_TOOL_AXES` 15,
  `INPUT_TYPE_TABLET_TOOL` 16).
* `0004` fixes the touchpad: the cursor landed a motion step ahead of the
  host's, finger scrolling was output-scale times too fast, and X11 windows
  get momentum after the fingers lift (X11 has no axis-stop, so they cannot
  scroll kinetically themselves).

The library is linked without `-Bsymbolic`, as Debian builds it, which
`assets/guest-arm64/anland-damage.c` relies on to interpose KWin's repaint
calls.

## Building

CI builds on every change to this directory and uploads an artifact with the
stripped library, the exact patched source tree
(`kwin-anland-6.3.6-portal-source.tar.xz`, the GPL corresponding source) and
`SHA256SUMS`. Run the workflow manually with `baseline` to build the lfdevs
source without Portal's patches for comparison.

To build elsewhere, run as root in a Debian 13 ARM64 environment:

```sh
patches/kwin/anland-6.3.6/build.sh . out
```

## Updating the APK

Copy the artifact's `libkwin.so.6.3.6` to
`assets/kwin-anland-arm64/libkwin.so.6.3.6`. Portal syncs it on every launch to
`/usr/local/lib/portal-anland/` (`sync_kwin_anland_overlay` in
`src/android/proot/setup.rs`); the KWin wrapper puts that directory first on
`LD_LIBRARY_PATH` for Anland sessions only. QPainter sessions keep using
`assets/kwin-debian-arm64`.

To move to a newer lfdevs release, update the tag and commit in `build.sh`,
rebase the Portal patches onto it, and rebuild.
