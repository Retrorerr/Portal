# Launch to CONFIGURE aperture spike

Base: `7cb19a829bfd9f737abac3e733b8f108e51965f2` on `spike/game-activity-host`.
Its only differences from validated `7e3657717ca1bb971569b594ee344e232fc0424c`
are patch documentation and making capped gesture-axis diagnostic logging opt-in.
No gesture sampling, ABI, or input behavior changed in that cleanup.

## Composition

The unchanged AndroidX system splash waits for real Compose CONFIGURE/header
layout and pre-draw. ComposeOverlay no longer creates a temporary ImageView.
The normal MATCH_PARENT Compose sibling remains in the GameActivity root above
the untouched InputEnabledSurfaceView.

During the intro, the real CONFIGURE composition is recorded into a Compose
GraphicsLayer display list and drawn sharply. A second layer replays it at
quarter width/height, applies a 24dp-equivalent GPU blur, charcoal frost, and an
AGSL aperture mask. This raster target contains 1/16 of the full display pixels.
There are no bitmap readbacks, per-frame CPU pixel copies, new dependencies,
SurfaceView captures, or cross-window blur effects.

The original header Image keeps its layout slot but suppresses drawing during
the intro. One visible canonical vector is drawn above the frost, interpolating
from the system splash's 288dp/1024-unit asset bounds to the actual header
Image's fitted bounds in the same Compose root coordinates. It does not
crossfade. At completion, the overlay vector is removed and the header Image
draws at the identical endpoint. Density, root bounds, and header placement
come from layout; there are no device pixel coordinates. Geometry updates track
re-layout, and interruption resolves instead of stranding the veil.

The 1200ms timeline starts after content pre-draw: 120ms aperture onset, 250ms
travel onset, 920ms landing, 1200ms clearing. Monotonic cubic curves give precise
deceleration without spring or overshoot; the aperture grows faster during
travel. Its angular lobes gently breathe, with one small localized refractive
front and restrained Portal-orange edge light. Optional logo motion blur was
omitted to preserve the crisp vector silhouette.

Transition values are read during drawing rather than by the CONFIGURE
composition. Compiled shader and blur kernel are reused; RenderEffect wrappers
are refreshed because Android snapshots shader uniforms. Capture resources and
effects leave composition at completion. The ordinary setup tree and its state
stay in their original composition slot. The approved Begin Install glow is
untouched, including its existing animation.

Pointer consumption in the initial Compose event pass, key interception, hidden
semantics, and an action guard prevent activation through the intro. They are
removed immediately on completion. No native input routing changed.

An Animatable honors Compose MotionDurationScale. Disabled animation and API
levels below 33 resolve directly. A lifecycle observer resolves on pause/stop,
even when the frame clock stops, and informs the host synchronously. Saveable
completion and host-owned process eligibility prevent replay on overlay
recreation. The full intro remains a first-process-launch validation path;
INSTALLING and desktop launch/reveal policies are not implemented here.

## External source

The MIT-licensed expanding mask/radial-displacement reference is Mejdi Hafiene's
Android-AGSL-Shader-Playground at `cdb866cbc3192dba326871354b98cbf5036227e5`.
See `third_party/android-agsl-shader-playground/README.md` and `LICENSE` for
exact source files and changes. The demo's concentric ripple, tap triggering,
and permanent animation wrappers are not used. Compose GraphicsLayer and
Android RenderEffect provide local backdrop capture/blur without Cloudy.

## Verification — 2026-09-12

- Repository xbuild debug/arm64 APK build passed. Final overlay-only adjustments
  were rebuilt with Gradle `:app:assembleDebug` using that generated project.
- `adb install -r` succeeded on the connected OnePlus Pad 3 (OPD2415, API 36).
- Installed APK SHA-256 matched the local build:
  `e8052a77a26fe0df1f4d75331fd51821cea5d193ce7086e794ea9c316a062762`.
- One `am force-stop` / `am start -W` cold launch: status OK, 569ms launch time.
- System logs confirmed splash charcoal `ff191b1c` and the AndroidX pre-draw hold.
- App log: 12:27:22.561 pre-draw release; 12:27:22.641 intro started;
  12:27:23.856 CONFIGURE interactive/effects released (1215ms).
- Process remained alive and PortalActivity resumed, with no app fatal exception.
- No screenshot/pixel automation, input injection, uiautomator, visual grading,
  or benchmarking was performed.
- Touch/touchpad operation and Begin Install → Anland/Plasma are pending the
  user's physical check. Their implementations and the approved glow are
  unchanged; this pass does not claim renewed physical validation of them.

Visual acceptance, reduced-scale playback, orientation and mid-intro background
behavior have not been physically graded in this pass.
