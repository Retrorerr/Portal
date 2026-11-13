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
| Portal patches | `0001-*.patch` to `0012-*.patch` here, appended to the lfdevs quilt series |

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
* `0005` adds touchpad pinch gestures (`INPUT_TYPE_POINTER_PINCH` 17).
* `0006` asks the consumer for Android pointer capture while a client holds
  a zwp_locked_pointer_v1 lock (`OUTPUT_TYPE_SET_CONSUMER_VAR` 3 with
  `CONSUMER_VAR_CAPTURE_MOUSE` 1, as in upstream Anland), so games get raw
  relative mouse motion that does not stop at the screen edges.
* `0007` closes X11 popup menus on presses outside Xwayland. An X client's
  pointer grab only sees input on Xwayland surfaces, so a tap on the panel,
  the desktop or a Wayland app left menus such as Steam's tray menu open.
  While an override-redirect menu is mapped and an X client holds a pointer
  grab, KWin delivers such a press to Xwayland as a click at the same
  position and swallows it, as an X server grab would.
* `0008` composites as soon as the consumer selects a buffer, instead of
  about a vblank later by the RenderLoop's own prediction (two when selects
  came close together, dropping a frame). It drops the full-output catch-up
  repaint that, once anything changed, made every later frame repaint the
  whole output: a frame with nothing to repaint is reported to the consumer
  as skipped (`REFRESH_DONE_SKIPPED`), which keeps presenting its last
  buffer. Input events carry the time Android sampled them
  (`INPUT_TYPE_EVENT_TIME` 18) instead of the time KWin read them, and the
  X11 touchpad fling steps once per output frame instead of on an 8 ms timer.
* `0009` reads all input the consumer has written before handling a select.
  The input socket and the buffer-ready eventfd are separate notifiers, so a
  select dispatched first sent the frame's frame callbacks ahead of that
  vsync's touch events: clients that sample input at their frame callback
  (Qt Quick, Gecko) showed no motion for one frame and double the next.
* `0010` stops KWin from repainting the output when nothing changed. The
  scene layer's repaint request was cleared only together with a repaint
  region it never has, so after the first repaint every frame request
  rendered: each touch made the consumer present about 180 identical frames.
  A frame that only serves a frame callback is now reported as skipped.
* `0011` flushes input to clients as soon as it is applied, so it does not
  leave in the same write as the frame callbacks of the composite that
  follows. Qt dispatches frame callbacks on a separate thread and often drew
  a frame before handling the touch that came with it.
* `0012` lets the consumer choose the output transform. With the display a
  quarter turn from the panel (a tablet in portrait), Portal allocates the
  buffers in the panel's orientation and KWin renders rotated into them, so
  Android's display controller scans them out instead of SurfaceFlinger
  rotating every frame on the GPU. The transform arrives right after a
  buffer set and is applied with the buffer size as one output change.

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
