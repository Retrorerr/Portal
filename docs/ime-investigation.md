# Qt/X11 keyboard activation investigation

Investigated 2026-10-26 for [issue #7](https://github.com/Retrorerr/Portal/issues/7).
This records the diagnosis, session-policy fix, and validation evidence.

## Confirmed failure

Portal's guest session has no Qt input-method selection policy for X11 clients.
`assets/localdesktop-startplasma.sh` sets `QT_QPA_PLATFORM=wayland;xcb`, but
neither the session launcher nor the Android launch environment selects an
IBus input context for Qt/X11. Display-backend fallback does not select an input
method.

Qt 5's XCB integration defaults to the local Compose input context when
`QT_IM_MODULE` is unset. That context does not connect to Portal's IBus engine,
so editable focus never reaches `ACTIVATE:ibus` in `src/android/ime.rs` and the
Android software keyboard is not requested.

VLC 3's X11-enabled Qt interface explicitly passes `-platform xcb`; adding the
missing Qt 5 Wayland plugin alone would not address this application.

## Physical-device evidence

Tested Portal Debug (`app.polarbear`) on the OnePlus Pad 3, leaving Stable and
existing runtime data intact. Installed Debug reports versionName 2.6.0; do not
infer its exact source revision from the current checkout's version (2.7.0).

- Plasma's environment has `QT_QPA_PLATFORM=wayland;xcb` and no `QT_IM_MODULE`.
- Qt 5's IBus and Compose plugins are installed; its Wayland platform plugin
  is absent.
- An unconfigured VLC launch mapped `libqxcb.so` and
  `libcomposeplatforminputcontextplugin.so`. The network URL field gained a
  caret without opening the Android keyboard.
- A launch from the guest's own Konsole with `QT_IM_MODULE=ibus` mapped the
  IBus plugin, without Compose. Focusing the network URL field generated
  `FocusIn (editable focused, client=QIBusInputContext)` followed by
  `ACTIVATE:ibus`; Android reported `mInputShown=true`. Dismissing the keyboard
  and tapping the field summoned it again.

Local screenshots are under `artifacts/qa/ime-investigation/` (ignored).

ADB test caveats: processes launched directly through `run-as` cannot perform
`kill(ibus_pid, 0)` against the app-launched daemon on this device (EACCES).
Qt's IBus plugin performs that check and rejects the connection. Launch probes
from the existing desktop session to avoid misdiagnosing this as a product
failure. Android text injection through the composing keyboard also autocorrected
`ibus` to `ibis`; verify the target process environment rather than trusting the
typed command. The successful comparison used an exact script in guest `/tmp`.

## Implementation

The shared launcher sources `portal-input-method-env.sh` before Plasma starts.
With no explicit Qt input-method setting, it sets `QT_IM_MODULE=ibus` for Qt 5
and `QT_IM_MODULES=wayland;ibus` for Qt 6.7+. Qt 6's Wayland backend handles the
first entry natively; XCB skips it and selects IBus. Qt 5 ignores the list and
uses IBus for either display backend. No app names or launcher overrides are
involved. GTK's display-specific selection remains unchanged. Explicit settings
present when the policy is sourced, including empty values, are preserved.

The helper is embedded in the APK and synced with the session launcher on every
launch. Dependency repair now checks the daemon, CLI, Python binding, typelib,
and Qt client plugins rather than a historical completion marker. It repairs
the selected rootfs, prevents duplicate repair jobs, and retries on later
launches for existing installs. The rootfs seed list now includes the required
packages for the next package-lock refresh/runtime-image build; this APK change
does not rebuild or republish the pinned runtime image. The currently published
image remains supported through dependency repair.
Repair remains asynchronous, so an older incomplete install that is offline
can still lack IBus until repair succeeds; already-running Qt 5 clients may
need reopening afterward.

## Validation of the fixed APK

Built and installed Portal Debug in place; package identity, maintainer debug
signature, and installed APK hash were verified by the build/install scripts.
Stable was left untouched. On the Pad 3's Anland/tawcroot session:

- A normal Plasma launch of VLC loaded IBus without Compose or a command-line
  input-method override. Its network URL field summoned the Android keyboard;
  dismissing and tapping the field reopened it.
- Software-keyboard typing and deletion worked. Ctrl+V inserted
  `https://example.org/é` exactly. No stream was opened. The clipboard text
  saved before the test was restored afterward.
- Switching to Konsole deactivated IBus and activated the KWin input-method
  bridge. Konsole mapped Qt 6 Wayland and no IBus input-context plugin.
- Android compile check, Debug APK build, 494 host test executions, and the
  Python suite (33 tests, one skipped) passed. The policy tests execute a real
  shell and cover defaults, repeated sourcing, explicit overrides, and leaving
  GTK untouched.

Hardware-keyboard hotplug, the QPainter renderer, and a newly downloaded runtime
were not exercised in this device run.

## Upstream source checks

- [Qt 5 XCB input-context selection](https://github.com/qt/qtbase/blob/5.15/src/plugins/platforms/xcb/qxcbintegration.cpp): `QXcbIntegration::initialize`.
- [Qt 5 input-context factory](https://github.com/qt/qtbase/blob/5.15/src/gui/kernel/qplatforminputcontextfactory.cpp): reads `QT_IM_MODULE`.
- [Qt 5 Wayland input-context selection](https://github.com/qt/qtwayland/blob/5.15/src/client/qwaylandintegration.cpp): `reconfigureInputContext`.
- [Qt 6 fallback-list selection](https://github.com/qt/qtbase/blob/6.8/src/gui/kernel/qplatforminputcontextfactory.cpp): `QT_IM_MODULES` takes precedence over the single module.
- [Qt 6 Wayland input-context selection](https://github.com/qt/qtwayland/blob/6.8/src/client/qwaylandintegration.cpp): native `wayland` entry and IBus fallback.
- [Qt 5 IBus connection and focus](https://github.com/qt/qtbase/blob/5.15/src/plugins/platforminputcontexts/ibus/qibusplatforminputcontext.cpp): `createConnection`, `isValid`, `setFocusObject`.
- [VLC 3 Qt platform selection](https://github.com/videolan/vlc/blob/3.0.x/modules/gui/qt/qt.cpp): `Open`, `ThreadXCB`, `ThreadPlatform`.
