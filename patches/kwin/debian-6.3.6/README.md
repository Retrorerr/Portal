# Debian 13 KWin 6.3.6

Apply `0001` (input provenance/lifecycle) and `0002` (QPainter buffer damage)
to KWin v6.3.6 (`b8de4329447824b1b1e7a36b3a57acfd069f1423`), in order. Keep Debian's
`relax-interplasma-versioned-deps.patch` from the 6.3.6-1 source package when
building against Debian 13's build dependencies.

## QPainter damage boundary

QPainter paints output-local logical damage through an integer QPainter window
onto its native-sized QImage. That mapping is not necessarily the integer
`wl_surface.buffer_scale`: at KDE scale 1.5, the Wayland backend advertises 2.
Sending that logical region as surface damage shifts/clips the effective upload
region increasingly far from the origin. Treating it as buffer damage without
mapping is also wrong.

`0002` maps the rendered region (including swapchain age repair) with the actual
QPainter window and QImage dimensions, rounds outward, clips to the image and
uses `wl_surface.damage_buffer` through KWayland. The EGL backend retains its
original setter and damage behavior. Portal uses standard Wayland damage
semantics, with no KWin-title-based coordinate reinterpretation.

## Build and regression test

Run in a Debian 13 ARM64 build environment with KWin and Qt development packages:

```sh
sh scripts/build-debian-kwin-damage.sh /path/to/kwin-6.3.6 \
  "$PWD/patches/kwin/debian-6.3.6" "$PWD/tests/kwin_qpainter_damage.cpp" \
  /path/to/build
```

The test uses real Qt QPainter rasterization, checks every changed pixel against
the exported damage, and verifies the former coordinate formulas fail. It covers
native, reduced, rotated and odd dimensions, six scales, and five positions.

Strip a copy of `build/bin/libkwin.so.6.3.6` and stage it in
`assets/kwin-debian-arm64/libkwin.so.6.3.6` before building the APK. Portal's
provisioning installs the packaged library for both existing runtime-B
and clean provisioning; device-only source edits are not a deployment mechanism.

Portal additionally propagates forced full texture synchronization to output
damage, and acquires the current EGL backbuffer before querying its age. Neither
full uploads nor correct source damage alone can repair an incorrectly bounded
final redraw. Native render scale is 1.0; KDE UI scale remains independent.

Android texture imports also preserve an already-current EGL context instead of
allocating/rebinding a temporary pbuffer per upload. Initial temporary pbuffers
are released using EGL's deferred destruction rule (destroy when no longer
current), rather than leaked. See the
[Android EGL contract](https://developer.android.com/reference/androidx/graphics/opengl/egl/EGLSpec#eglDestroySurface(androidx.graphics.opengl.egl.EGLSurface)).

Physical validation remains required: drag Settings across all four corners,
scroll changing content near the bottom right, then test resize and resume.
