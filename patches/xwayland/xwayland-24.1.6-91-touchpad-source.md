# Phase B: XWayland touchpad-source diagnosis (Firefox momentum)

Status: **diagnosis complete, hypothesis confirmed end-to-end; native ARM64
build pipeline live; Portal 0005 candidate patch authored in-repo.**
**No fake inertia was added anywhere. Stock XWayland remains canonical.**

## Provenance of the pinned binary (exact)

- Portal package: `xwayland_24.1.6-91_arm64.deb`
  (`scripts/build_debian_rootfs.py`: `LFDEVS_XWAYLAND_DEB`, size 825848,
  SHA-256 `59f9c7486d6a10ad50a13622bf1d1bbf5accd015d630e4b2b0152a80577dcc64`),
  version `2:24.1.6-91` (`scripts/publish_runtime_release.py`).
- Upstream/base: xwayland 24.1.6 as released in xorg-server, identical to
  Debian `2:24.1.6-1` (trixie). Verified by downloading
  `xwayland_24.1.6.orig.tar.xz` from a Debian mirror and checking
  MD5 `78067c218323fe2a496ca5f2145fe7ab` (matches the published Ubuntu
  checksum for the same file).
- lfdevs source, pinned EXACTLY (never the branch tip):
  `github.com/lfdevs/xwayland` commit
  `461772ae63c8985fd5671ea85d38d5102590d760`
  (merge PR #1 "Support KGSL surfaceless glamor and DRI3 rendering",
  2026-07-09; changelog head at this commit is `xwayland (2:24.1.6-91)`).
  Packaging delta over Debian `-1` is exactly 4 patches
  (`debian/patches/series` at the pinned commit), all graphics-only:
  - `0001-...-kgsl-dmabuf-v3-fallback-paths` (rev -90)
  - `0002-...-glamor-...-KGSL-surfaceless-backend-path` (rev -91)
  - `0003-...-dri3-...-KGSL-surfaceless-client-render` (rev -91)
  - `0004-...-KGSL-frame-callback-recovery-per-window` (rev -91)
  The Portal patch is `0005` (see below); `-92` stays reserved for lfdevs.
- Input-path identity: `hw/xwayland/xwayland-input.c` at the pinned commit
  is **byte-identical** to upstream 24.1.6
  (3621 lines both; `git diff --no-index` empty; also identical to the
  `debian-unstable` tip file). No lfdevs patch touches
  input, axis handling, XI2 devices, or valuators.
- Stock binary in the canonical runtime:
  `/usr/bin/Xwayland` SHA-256
  `3a25266671b7615740a7da602bd6a645bc8966be04d1a69c3536f09e67df2f87`
  (recorded 2026-09-11; keep as the immediate recovery reference).

## Native ARM64 build pipeline (reproduces -91)

- Workflow: `.github/workflows/xwayland-arm64.yml`, `runs-on:
  ubuntu-24.04-arm`, container `debian:trixie@sha256:f324c7ff54321e8d9c588493a20244965938ce0aa50bbd1022d38010e9ffc4b1`
  (manifest-list digest; arm64 image `sha256:0aa09084…`). Build-only,
  `contents: read`, branch-scoped trigger; uploads .deb/.buildinfo/.changes
  plus a canonical-vs-reproduced comparison. Pushes the pinned SHA checkout
  and fails closed on any pin mismatch.
- Baseline validation (unmodified pinned source, CI run 2026-09-11):
  version `2:24.1.6-91`, arch arm64, **identical .deb file layout**,
  **identical control metadata incl. Depends**, AArch64 ELF64 with the
  expected NEEDED set, `/usr/bin/Xwayland` 2455888 bytes both.
  Binary SHA differs (`3a25…` vs `97e4…`); byte-level analysis proves
  metadata-only delta: 20-byte `.note.gnu.build-id`, `.dynsym`/`.dynstr`
  entry *ordering* (same symbols, link-order nondeterminism), and the
  `.gnu_debuglink` filename (embeds the build-id). Section headers
  identical. Source/package configuration conclusively equivalent, so per
  the task this does not block the candidate.

## Diagnosis: how FINGER source was handled before (now)

`hw/xwayland/xwayland-input.c` (upstream 24.1.6, identical in -91):

- `pointer_handle_axis_source()` is an **empty function**: the
  `WL_POINTER_AXIS_SOURCE_FINGER` / `WHEEL` / `CONTINUOUS` / `WHEEL_TILT`
  distinction is received and discarded. All sources behave identically.
- `pointer_handle_axis()` accumulates smooth deltas; `dispatch_scroll_motion()`
  emits them via `QueuePointerEvents(get_pointer_device(), MotionNotify, 0,
  POINTER_RELATIVE, &mask)` on the **single** emulated seat pointer, with
  smooth valuators `SCROLL_AXIS_VERT/HORIZ`.
- `pointer_handle_axis_stop()` emits a zero-delta scroll MotionNotify
  (clean physical-gesture termination, no momentum by itself).
- The seat pointer (`xwl_pointer_proc`) declares buttons 4-7 as
  `BTN_WHEEL_UP/DOWN + BTN_HWHEEL_LEFT/RIGHT` and scroll valuators
  `SCROLL_TYPE_VERTICAL/HORIZONTAL` with wheel axis labels. No XI touch
  class, no extra XI properties, no gesture protocol support.

## What Firefox/GTK classified the device as before (now)

GTK 3.24 (`gdk/x11/gdkdevicemanager-xi2.c`, `create_device()`), which is
Firefox ESR's toolkit path under X11:

- `is_touchpad_device()` is true **only** if the XI2 device exposes the
  `libinput Tapping Enabled` property (8-bit INTEGER, 1 item). XWayland's
  virtual pointer does not set it.
- The device has no `XITouchClass`, so the touch-class rule cannot fire.
- The name heuristic sees "xwayland-pointer" (contains "pointer") and falls
  through to **`GDK_SOURCE_MOUSE`**.

So every scroll Firefox receives is mouse-classified. Firefox's GTK backend
turns **touchpad**-source smooth scrolls into `PanGestureInput` (which ends
in an APZ fling when `apz.gtk.kinetic_scroll.enabled`, default true;
Mozilla bugs 1213601, 1564238, 1781209) and **mouse**-source smooth scrolls
into `ScrollWheelInput` (no fling). XI2 valuator events demonstrably flow
(pixel-smooth scrolling works), so classification — not transport — is the
single broken link. The original hypothesis is **confirmed from both ends**.

## Candidate design (implemented as 0005, validation pending)

Smallest patch consistent with the GTK rule above, against the exact
`lfdevs/xwayland@debian-unstable` source:

1. Retain the Wayland axis source in `struct xwl_seat`'s
   `pending_pointer_event` (set it in `pointer_handle_axis_source()` instead
   of discarding it; clear it per pointer frame like the other
   `has_*` flags).
2. Create one additional XI2 slave pointer per seat at seat init
   (e.g. "xwayland-touchpad"), modelled on `xwl_pointer_proc` but carrying
   **only** the two scroll valuators (same `SetScrollValuator` flags as the
   main pointer) and additionally setting the `libinput Tapping Enabled`
   XI property (8-bit INTEGER, one item, value 0 — tapping is not
   implemented) at `DEVICE_INIT` so GTK's
   `is_touchpad_device()` passes. No touch class, no other libinput
   properties (do not fake capabilities).
3. In `dispatch_scroll_motion()`, route finger-source frames through the
   touchpad device and wheel/continuous/tilt-source frames through the
   existing pointer device. Axis-stop emits the zero-delta frame on
   whichever device owns the in-progress gesture (track the active source
   per frame; default to the main pointer when unknown).
   The touchpad device sends **accumulated** surface-px positions, not
   per-event deltas: XI2 scroll valuators are running positions (GTK and
   the server's own button emulation difference consecutive values), so
   per-event deltas would read back as ~zero/jitter. Stop frames repeat
   the position. The stock pointer keeps today's exact per-event values.
4. Never touch glamor/GBM/DRI3/present paths, device init of the existing
   pointer/keyboard, or the `relative_pointer` confinement path.

Expected result: finger scroll arrives on a GTK-TOUCHPAD device (existing
`apz.gtk.kinetic_scroll.enabled=true` then yields the PanGesture fling with
zero Firefox configuration change); wheel scroll stays mouse-classified on
the original device with identical behavior.

## Deterministic build recipe (live)

- Implemented as `.github/workflows/xwayland-arm64.yml` (see the pipeline
  section above): checks out the pinned SHA, registers
  `patches/xwayland/0005-xwayland-preserve-finger-axis-source.patch` as
  `debian/patches/0005-...` with a `debian/changelog` entry for
  `2:24.1.6-91portal1` (never `-92`), asserts the patch scope (only
  `hw/xwayland/xwayland-input.{c,h}` + series/changelog; graphics files
  untouched), then `dpkg-buildpackage -b -us -uc`.
- Provenance gate (enforced in CI): input-file diff against upstream 24.1.6
  shows ONLY the candidate hunk; `debian/patches/series` lists exactly
  0001-0005.
- Record the resulting candidate package/binary SHA-256 values in the final
  report next to the stock SHAs above.

## A/B staging design (implement when a candidate exists)

- Ship the candidate as `/usr/bin/Xwayland.portal-touchpad-candidate`
  (never overwrite `/usr/bin/Xwayland` until validation succeeds).
- Select via an explicit session flag (e.g. `files/xwayland-variant`
  `stock|candidate`, default `stock`), consumed by the session launcher
  with a deterministic fallback to stock on any staging error.
- Keep the stock SHA above as the immediate recovery reference; no runtime
  republication until the full 15-point matrix below passes.

## Test matrix status

Baseline (stock, recorded 2026-09-11, confirmed physically on the OnePlus
Pad 3 keyboard-case touchpad): Firefox V+H two-finger scroll works, stops
dead on finger lift (no momentum); fast flick likewise stops dead the
instant the fingers lift; Ctrl+two-finger-scroll zooms correctly;
Plasma/Settings scroll works; speed/natural/persist work; XWayland KGSL
surfaceless glamor active; Anland READY gen-1 fenced, `fallbacks=0`.
A real-mouse wheel baseline is unavailable (no real mouse on hand). The
candidate matrix (criteria 1-15 in the task) has NOT been executed — there
is no candidate binary.
