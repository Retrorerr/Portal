# Portal architecture

Portal runs a local Linux desktop across two security and runtime domains: an Android-native host and an unprivileged Debian guest. The boundary between them is deliberate. Android owns hardware and application lifecycle; Debian owns the desktop session and Linux applications.

## Process model

```text
Android
└─ Portal package / NativeActivity
   ├─ winit event loop
   ├─ Smithay Wayland compositor
   ├─ setup and diagnostics WebView
   ├─ Android integration bridges
   └─ PRoot supervisor
      └─ Debian 13 guest
         ├─ KDE Plasma 6 / KWin
         ├─ PipeWire and Pulse compatibility
         └─ user applications
```

The Android process is the long-lived owner. Guest processes are supervised children, not independent services, and must tolerate Android pausing, resizing, recreating, or terminating the activity.

## Startup phases

### 1. Host initialization

`android_main` initializes logging, diagnostics, Android handles, the accessibility bridge, and the winit event loop. `PolarBearApp` then selects either the setup backend or the Wayland backend.

### 2. Runtime provisioning

The setup backend prepares a versioned Debian runtime in app-private storage. Stages are idempotent: completed work is recorded, and interrupted setup resumes from the next incomplete boundary.

Runtime images use A/B slots. A candidate slot can be assembled and validated without destroying the last known runtime. Switching the active-slot marker is the commit point.

### 3. Desktop launch

When provisioning is complete, Portal creates the Android/EGL renderer and Smithay state, publishes the Wayland socket to the guest, and starts the Plasma session under PRoot. KWin is a nested Wayland client of Portal's compositor.

Portal does not use X11 or VNC as its normal display transport. Xwayland exists inside the guest only for applications that do not support Wayland.

## Rendering boundary

The compositor translates between Android's surface lifecycle and Wayland's persistent client model.

- Android provides the native window, physical dimensions, rotation, density, and refresh rate.
- Smithay owns Wayland globals, surfaces, configure sequencing, frame callbacks, and presentation feedback.
- EGL/GLES renders into the Android surface.
- Portal exposes `android_wlegl` and can import a compatible Android
  `AHardwareBuffer` directly as an EGL image. The current KWin 6.3 Wayland
  QPainter backend does not produce those buffers: it allocates `wl_shm`
  buffers, which Portal uploads to GLES. Reaching the zero-copy path therefore
  requires an Android-buffer allocator/export path in the guest compositor;
  the server-side import support alone does not make the current desktop
  zero-copy.
- Surface loss suspends the renderer; resume recreates it without launching a second guest session.

Readiness is stricter than process liveness. Portal requires the expected KWin identity, an acknowledged configure, a committed guest buffer, and a host-presented frame from the same startup generation before declaring the desktop ready.

## Input and clipboard

Android input is normalized before it reaches Wayland:

- touch and pointer coordinates share the same display transform used by rendering;
- physical key events map to Linux input codes;
- Android IME commits are queued and delivered through the supported text-input path;
- attached keyboard, mouse, and touchpad state informs Plasma's tablet-mode policy;
- clipboard exchange accepts bounded text MIME types and suppresses echo loops.

The optional accessibility service exists because Android can consume hardware shortcuts before the activity receives them. It forwards key events; it does not inspect the Android window hierarchy.

## Audio and files

The guest exposes normal PipeWire/Pulse interfaces to Linux applications. Portal's native sink bridges the resulting stream to AAudio on Android.

The Debian filesystem remains in app-private storage. Shared Android storage is mounted separately and only when the corresponding Android permission is granted.

## Failure and recovery

Every recovery path should preserve evidence and user data.

- setup failures remain on the local progress UI and can export diagnostics;
- early Plasma failures enter a graphical native-Wayland recovery session;
- host and guest logs rotate within bounded storage;
- diagnostics include Portal-owned state and exclude unrelated personal files;
- uninstalling the Android package is not a recovery step because it removes app-private guest data.

## Compatibility boundaries

The visible daily-use product is Portal. Release builds use package
`app.polarbear.portal` and app-private storage below that package. Portal Debug
retains the historical `app.polarbear` identifier and its existing data so both
variants can be installed, launched and updated independently. Each variant
therefore provisions its own Debian runtime and cannot accidentally launch the
other variant's half-installed state.

The following guest and native identifiers remain stable because Debian setup
and native loading depend on them:

- crate and native-library name `localdesktop`;
- guest paths under `/etc/localdesktop`;
- selected asset and artifact basenames.

Changing one of these requires an explicit migration and upgrade test, not a cosmetic rename.
