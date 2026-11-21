# Qt Wayland compatibility overlay

Debian ARM64 Qt Wayland 6.8.2 client library with the source patches and build
recipe in `patches/qtwayland/6.8.2/`. It estimates the fractional output scale
before a window gets its Wayland surface and batches continuous pointer and
touch events within a single Wayland dispatch. Compression is on by default
as it is on xcb. Qt's public compression opt-out, discrete events, touch press
and release, wheel distance, scroll phases and native tablet samples are preserved.

The startup estimate uses a scale in 120ths when exactly one value fits both
rounded dimensions. At 275% KWin reports 3392×2400 physical and 1233×873
logical pixels. The earlier estimate rejected this pair and LibreOffice laid
out dialogs at 300%. Windows use the compositor's preferred scale once it arrives.

Source archive:
https://download.qt.io/archive/qt/6.8/6.8.2/submodules/qtwayland-everywhere-src-6.8.2.tar.xz
SHA-256: `5e46157908295f2bf924462d8c0855b0508ba338ced9e810891fefa295dc9647`.

The library is dynamically linked. Qt Wayland is available under LGPL-3.0-only,
GPL-2.0-only or GPL-3.0-only (or a commercial Qt license); this build uses the
LGPL option. The original copyright and license information is in the source
archive and Debian's `/usr/share/doc/libqt6waylandclient6/copyright` and
`/usr/share/common-licenses/LGPL-3`. The build recipe includes all source changes.

The runtime stages this file outside the normal loader path and mounts it only
when the stock Qt Wayland and Qt Core fingerprints match the builds it was
compiled against. A Debian upgrade or user rebuild disables the overlay.
Bundled and Flatpak Qt libraries retain their own loader paths.

Build on Debian 13 ARM64 with the dependencies listed in `build.sh`:

```sh
bash patches/qtwayland/6.8.2/build.sh /tmp/qtwayland-build /tmp/qtwayland-output
```

`tests/compat/qt_pointer_batch.cpp` exercises the actual QPA boundary, including
mixed press/drag/release, multi-touch frames, wheel-direction changes, phase
boundaries and the compression opt-out used by drawing applications.
`tests/compat/qt_widgets_probe.cpp` reports the scale seen by the first resize.
`tests/compat/qt_fractional_output_scale.cpp` exercises every 120th between
100% and 500% on the Pad 3 output in landscape and portrait.
