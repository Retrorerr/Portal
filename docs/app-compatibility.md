# Application compatibility

Portal uses Debian's normal desktop runtimes and preserves application-specific
library, plugin, scaling, rendering and input-method settings. Qt 5 and Qt 6
Wayland plugins are included in fresh installations and added once to existing
installations. Older downloaded runtime images receive missing plugins through
the first session's APT repair; Xwayland remains available while that finishes
or when an upgrade is offline. Bundled and older Qt installations
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
Portal-derived Electron launchers no longer force `--ozone-platform=wayland`
or `--no-stdio-init`. Existing entries lose those options only when their
contents still match the vendor entry plus Portal's known additions. Modified
user launchers and vendor-supplied options are preserved. Android input-method
integration and the existing shortcut-portal integration remain in place;
this is not a removal of every browser compatibility option. Steam likewise
uses its native scale selection and inherits an explicit
`STEAM_FORCE_DESKTOPUI_SCALING` without manufacturing one from a Qt setting.
KWallet's first-login setup uses the base image's D-Bus client. It no longer
waits for Python's `gi` binding, which older runtime images install in the
background for IBus. Existing wallets and chosen passwords retain their
normal behavior.

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
| VLC 3.0.24 | Dynamically loaded Qt 5.15.15 interface on Xwayland; default 1080p playback and displayed-frame counters for H.264, HEVC, VP9 and AV1 |
| Chromium 156 | Disposable profile, owned WebGL 2 animation on Wayland, Adreno ANGLE renderer and correct pixel readback |
| Steam ARM64 | Native sign-in window and matching geometry with/without the former forced scale; account features and games are not covered |
| Electron 45.2.1 | Official independent ARM64 runtime, owned application, default Wayland/Adreno WebGL 2 rendering, pixel readback and debugger-driven Unicode composition; native Claude launch checked separately |
| Java 21 Swing | Window creation and animation tested; default XRender presentation batching remains a performance limitation |
| OpenGL and Vulkan | Wayland EGL, Xwayland GLX, and Vulkan enumeration plus cube swapchain rendering on both Wayland and Xwayland, using Adreno 830 |
| VA hardware decoding | H.264, HEVC, VP9 profile 0 and AV1 profile 0; owned 8-bit 1920×1080 clips, 60 frames per decode, exact software-reference pixels and active Qualcomm Android decoder components |
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
not establish compatibility for individual Electron applications. The Electron
fixture uses its own application and profile; its composition test uses the
debug protocol, rather than the Android keyboard. Native Claude launch tests
use no debugger and do not exercise sign-in or account features.
The Electron fixture records an initial input-focus/window pause of about one
second on this device, both with the old options and with native launch
selection. After two seconds of per-window warmup, three four-second runs in
each mode had no presentation gap over 100 ms. Warmup timing is retained in
the observations; this does not establish pause-free startup. Steam's initial
client update downloaded successfully, but its first CEF handoff timed out;
a subsequent native launch and a repeat of the old launch both opened the
sign-in window. That first-update limitation is not fixed by a scale override.

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

Default VLC playback selects VAAPI/Qualcomm hardware for H.264, HEVC and VP9.
VLC 3.0.24 selects software decoding for AV1; playback success must not be
reported as AV1 hardware acceleration in that player. Direct FFmpeg VAAPI AV1
checks use the physical Qualcomm decoder. The media suite records the actual
Android codec components separately from VA surfaces; Android's public codec
inventory distinguishes hardware and software implementations. These checks
cover the tested 8-bit profiles, not every codec profile, resolution or player.

Qt 4 and other untested Qt versions, 32-bit FEX applications, `.NET`/Mono, JavaFX,
complete Electron application workflows, Windows installers and complete Windows GUI applications
need their own functional tests. PRoot needs separate disposable runtime data;
results from tawcroot cannot be transferred to it. Qt windows were also tested
at integer scale 2 and fractional scales 2.2333 and 2.625, with portrait rotation, resize
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
environment, installed APK fingerprints, launch failures, probe observations, SurfaceFlinger presentations,
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
python3 scripts/build_compat_fixtures.py --output target/compat-hd \
  --kind media --media-size 1920x1080 --frames 240
python3 scripts/app_compat.py --serial DEVICE --suite playback --repeat 3 \
  --fixtures target/compat-hd --output target/qa/playback
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
Playback fixtures must last at least one second longer than the measured
interval. VLC's probe uses its normal decoder/output selection with an isolated
configuration and a local statistics interface; no hardware decoder is forced.
`scripts/read_android_codecs.py --serial DEVICE --output target/qa/codecs.json`
records Android's public decoder flags. Codec log evidence is limited to
Debug's Android UID and the measured interval.

An independently installed Electron runtime can be tested with
`--suite apps --case electron --electron-runtime /path/to/electron`.
The owned fixture covers renderer selection, animation, pixels and composition;
applications which reject debugging require separate native launch checks.

`tests/compat/qt_widgets_probe.cpp` can also be compiled against an older
bundle's own Qt 5/6 headers and libraries, without replacing system Qt. Run
that binary through `guest_exec.py` in the live desktop context, with library
and plugin paths scoped to that process. It reports the actual Qt version,
backend, animation count and resize result; Qt 4 needs a separate fixture.

Portable tests use isolated caches and Proton prefixes. LibreOffice uses
disposable profiles and documents; VLC uses a disposable configuration.
Diagnostic logs and fixtures remain in the report's named guest `/tmp` folder.

Updated-install checks compare an actual in-place APK update with controlled
document and preference fingerprints. Clean first-run checks use
`scripts/debug_data_state.py`: it force-stops Debug and renames its private data
into a journalled snapshot, preserving guest hardlinks and permissions. Both
states remain on the device after restoration; it never clears or uninstalls
an app. Android permission grants are retained, so this tests clean app-data
setup rather than resetting every Android first-install permission flow.
Always restore using the same device and record after testing:

```bash
python3 scripts/debug_data_state.py --serial DEVICE --record target/qa/data.json isolate
# Run first-run setup and the compatibility suites in Portal Debug.
python3 scripts/debug_data_state.py --serial DEVICE --record target/qa/data.json restore
```
