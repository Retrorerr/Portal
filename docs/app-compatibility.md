# Application compatibility

Portal uses Debian's normal desktop runtimes and preserves application-specific
library, plugin, scaling, rendering and input-method settings. Qt 5 and Qt 6
Wayland plugins are included in fresh installations and added once to existing
installations. Xwayland remains available. Bundled and older Qt installations
keep their own plugins; Portal does not set a global `QT_PLUGIN_PATH`.
[Qt documents why these plugin paths must remain separate](https://doc.qt.io/qt-6/deployment-plugins.html).
Portal also leaves `QT_QPA_PLATFORM` to the application and user. Modern Qt
selects Wayland from the session and retains its XCB fallback. A global
`wayland;xcb` override would break older Qt versions that interpret it as a
single plugin name, including [Qt 5.9](https://github.com/qt/qtbase/blob/v5.9.9/src/gui/kernel/qguiapplication.cpp#L1035).

The Qt 6 fractional-scale fix is built from the pinned Debian-compatible
QtWayland source in `patches/qtwayland/6.8.2`. Its overlay is used only when
the installed QtCore and original QtWayland library match the tested hashes.
An upgraded or independently bundled Qt is left intact. The former managed
LibreOffice preload launcher is retired without removing custom launchers.
The animation preload intercepts only explicitly versioned Qt 5/6 entry
points. Unversioned legacy and bundled builds use their native event loops.
Its remaining private timer adjustment is limited to tested Qt 5.15/6.8;
other minors retain native timing.
Already-loaded Qt components with renamed bundle SONAMEs are resolved within
their matching Qt family, including `RTLD_LOCAL` plugins. Host loader tests
cover these cases; Qt 4 GUI applications remain unverified.

## Verified coverage and limits

Device coverage currently focuses on Debian 13, Anland and tawcroot on the
OnePlus Pad 3. A small probe verifies its named flow, not every feature of an
application built with the same toolkit.

| Path | Coverage |
| --- | --- |
| Bundled Qt 5.9.5 Widgets | Isolated ARM64 runtime, native Xwayland animation and resize; the former global platform string reproduces a launch failure |
| PyQt 5.15.15 Widgets and Quick | Native Wayland windows and animation; Widgets also tested through Xwayland |
| PyQt 6.8.2 Widgets and Quick | Native Wayland, initial fractional scale and consistent widget/window scale; Widgets also tested through Xwayland |
| PySide 6.8.2 Widgets and Quick | Native Wayland animation, fractional scaling and unclipped Widgets dialog geometry |
| LibreOffice Writer, Calc and Impress | Disposable native KDE/Qt documents, save/reopen, repeated viewport updates and shutdown |
| GTK 2/3/4, Tk and wxWidgets | Native runtime windows and Unicode text; GTK 2 through Xwayland |
| SDL 2 | Animated Wayland OpenGL window on Adreno 830 |
| VLC | Dynamically loaded Qt 5.15.15 interface window on Xwayland; launch-only coverage |
| Chromium 156 | Disposable profile, owned WebGL 2 animation on Wayland, Adreno ANGLE renderer and correct pixel readback |
| Java 21 Swing | Window creation and animation tested; default XRender presentation batching remains a performance limitation |
| OpenGL and Vulkan | Wayland EGL, Xwayland GLX, and Vulkan enumeration plus cube swapchain rendering on both Wayland and Xwayland, using Adreno 830 |
| VA hardware decoding | H.264, HEVC, VP9 profile 0 and AV1 profile 0; owned 8-bit 320×180 clips, 60 frames each |
| x86-64 Linux | Box64 console fixture, including spaced and Unicode arguments |
| Windows x86-64 | ARM64 Proton/FEX console fixture with a file receipt from inside the Windows process; separate disposable prefix |
| Native type 2 AppImage | FUSE-free extraction, repeated cached launch, AppImage environment and Unicode arguments |
| Flatpak freedesktop 25.08 | Installed user/system fixture, native Wayland, Adreno GL, correct pixel readback, Vulkan hardware enumeration and denied access to an external file canary |

Three warmed LibreOffice runs completed for each component at the Pad's
fractional scale. Writer and Impress had no active-flow presentation gaps over
100 ms. Calc occasionally had gaps around 120 ms. The default Java 21 XRender
probe had repeated gaps around 500 ms despite frequent paint callbacks; an
explicit application `Toolkit.sync()` after painting removed those gaps in
three warmed comparison runs. This points to deferred X11 flushing in this flow;
Portal does not force a different Java renderer or inject application flushes.
The isolated Qt 5.9.5 window launched and animated after removing the global
platform override, but resizing coincided with a 220–280 ms presentation gap
in each warmed run. This older runtime is not verified as free of resize jank.
Modern Qt/PySide Wayland and Qt 5/6 Xwayland runs separately cover animation,
resize and dialog geometry. The Vulkan Wayland cube completed three warmed
runs without a presentation gap over 100 ms. Its Xwayland test verifies
swapchain execution; startup/teardown gaps are not classified as animation jank.
The installed Chromium likewise completed three warmed WebGL runs without a
presentation gap over 100 ms, using its normal renderer selection. That does
not establish compatibility for individual Electron applications.

Flatpak GL extensions are provided for freedesktop branches 24.08 and 25.08.
Their presence alone does not verify an installed application's graphics or
its portal services. Other runtime branches need a matching extension build.
Application sandbox permissions still apply.
Portal adds missing KGSL defaults to the system installation's Flatpak graphics
environment once, including when a launcher omits the desktop's GPU variables.
Existing values, explicit unsets, per-app/user overrides and subsequent
removals are preserved. It likewise adds the legacy session-variable cleanup
only when no explicit global choice exists.
In that controlled missing-environment case, three warmed five-second runs
moved the fixture from softpipe to Adreno. Perfetto measured median client CPU
time of 1.36 seconds before and 0.25 seconds after; both had no presentation
gaps over 100 ms. This small clear-animation fixture is not a general app
performance estimate. The file canary checks one access rule; namespace
emulation retains Android kernel limits.

Qt 4 and other untested Qt versions, 32-bit FEX applications, `.NET`/Mono, JavaFX, individual
Electron applications, Windows installers and complete Windows GUI applications
need their own functional tests. PRoot needs separate disposable runtime data;
results from tawcroot cannot be transferred to it. Qt windows were also tested
at integer scale 2 and fractional scale 2.2333, with portrait rotation, resize
and Android background/foreground resume. The original scale and rotation
policy were restored. Power suspend, physical keyboard/IME, touchpad, stylus,
clipboard, drag-and-drop and capture still require dedicated interaction
tests. Do not treat a launch-only result as verified
support for those features.

## Repeating the tests

`scripts/app_compat.py` operates only on Portal Debug (`app.polarbear`) and
uses its active engine, recorded mounts, desktop account and live environment.
GUI probes activate a temporary, one-use service on the live session bus.
This gives their processes the desktop's actual Android SELinux domain;
ADB's `run-as` domain cannot delegate a private Wayland listener to KWin.
The temporary activation service is removed after the command. Root
maintenance retains direct engine execution; `guest_exec.py --adb-context`
explicitly selects that context for non-root diagnostics.
It checks desktop visibility before the suite and after each flow, swipes up
the veil on entry when necessary and saves a screenshot before measuring.
A failed visibility check invalidates the run.
Only one runner per device can measure at a time.

```bash
python3 scripts/app_compat.py --serial DEVICE --suite qt --repeat 3 \
  --seconds 5 --perfetto --output target/qa/qt
python3 scripts/app_compat.py --serial DEVICE --suite office --repeat 3 \
  --seconds 5 --output target/qa/office
```

Each case has one warmup followed by the requested number of measured runs.
`--case` limits a suite to specific probes. `--install-probes` installs optional
test dependencies through APT. `--env KEY=VALUE` provides an explicit per-test
comparison, such as `--env QT_QPA_PLATFORM=xcb`; it never rewrites the session.
`--library-path` similarly stages an isolated source-fix comparison.
`--require-frame-pacing` additionally fails a measured active flow with gaps
over 100 ms or insufficient presentation samples; warmup timing is excluded
from that gate. Functional success and pacing observations are recorded
separately.

Reports include runtime versions, selected engine, inherited graphics/toolkit
environment, launch failures, probe observations, SurfaceFlinger presentations,
Android PSS and CPU ticks. GNU `time`, when installed, adds command CPU time
and maximum individual-process RSS. These are not total guest memory. Optional
Perfetto captures contain scheduler/graphics evidence; frame-timeline or
function-level CPU conclusions require the corresponding data and analysis.

Animated probes identify their active interval. Static-window idle gaps and
startup/shutdown gaps are not animation jank. Guest paint counts and Wayland
commits are separate from actual Android presentations. On the Pad 3,
OxygenOS may present at 50 Hz even when Qt advertises 144 Hz; use the recorded
presentation timestamps when comparing pacing. Compare three warmed runs with
the same scale, orientation, refresh policy and visible desktop.

Build disposable codec and translation fixtures on an x86-64 host:

```bash
python3 scripts/build_compat_fixtures.py --output target/compat-fixtures \
  --kind linux --kind windows --kind media
python3 scripts/app_compat.py --serial DEVICE --suite media --repeat 3 \
  --fixtures target/compat-fixtures --output target/qa/media
python3 scripts/app_compat.py --serial DEVICE --suite portable --repeat 3 \
  --fixtures target/compat-fixtures --output target/qa/portable
```

The builder uses a C compiler, LLVM's Windows linker/import tools and FFmpeg's
test encoders. AppImage fixtures additionally need `mksquashfs`, an official
type 2 runtime for the target architecture and its expected SHA256, supplied
with `--kind appimage --appimage-runtime PATH --runtime-sha256 SHA256`.
The runner verifies the fixture manifest before uploading it. Missing fixtures
or runtimes are reported explicitly. Windows probes require a receipt written
inside the Windows process; a successful Proton launcher exit is insufficient.
Hardware decode tests require VA surfaces and compare every downloaded pixel
with a separate software decode, accepting at least 40 dB PSNR or exact output.

`tests/compat/qt_widgets_probe.cpp` can also be compiled against an older
bundle's own Qt 5/6 headers and libraries, without replacing system Qt. Run
that binary through `guest_exec.py` in the live desktop context, with library
and plugin paths scoped to that process. It reports the actual Qt version,
backend, animation count and resize result; Qt 4 needs a separate fixture.

Portable tests use isolated caches and Proton prefixes. LibreOffice uses
disposable profiles and documents; VLC uses a disposable configuration.
Diagnostic logs and fixtures remain in the report's named guest `/tmp` folder.
