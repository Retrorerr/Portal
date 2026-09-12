# Ambient spatial frost refinement — 2026-09-12

Baseline: `a2aa27052062c37c7258a6528cb6cd02186422d5`, branch
`spike/game-activity-host`. Runtime changes are confined to PortalAmbientFragments
and measuring/passing the central card bounds in PortalSetupScreen.

Rendering:

- Seven cached canonical path segments: five ivory aperture sections, two orange
  threshold sections. Opacities 3.2–6.4%; sizes/orientations vary by depth.
- Independent continuous phases, linear 0–2π, periods 9.5–15 seconds. Asymmetric
  integer-harmonic paths span 21–34% of the screen per axis (plus small harmonics).
  Central anchors deliberately carry multiple fragments through the card region.
  Position and velocity both match at wrap; no reverse/ping-pong leg. Rotation
  varies ±3.5 degrees and scale ±2.2%, subordinate to translation.
- One quarter-width/height scene display list feeds two GPU RenderEffect passes:
  16dp mild blur and 68dp strong blur. No per-fragment blur or CPU bitmap capture.
- An original Portal AGSL rounded-rectangle SDF uses the actual unclipped card
  bounds and SurfaceCorner token, feathering across 130dp outside the card.
  Strong blur fully replaces mild blur beneath the card, with fragment contrast
  reduced 18%. Both passes include the current background color, so their blend
  does not accumulate duplicate silhouettes or require an opaque installer card.
- Shader/blur objects and path sections are cached. Animation is read only during
  background drawing; mask uniforms/effect snapshots change only with layout,
  density or palette. Existing upper-right orange light is unchanged.

Validation:

- Kotlin/Compose compilation and repository xbuild debug/arm64 APK build passed.
- Numerical trajectory wrap check: maximum position/velocity difference 5.2e-15.
- adb install -r succeeded on the Pad 3. One force-stop/cold launch: status OK,
  550ms, process remained alive/resumed. Splash removal at 13:44:05.033 preceded
  aperture start at 13:44:05.043; CONFIGURE interactive at 13:44:06.268. No immediate fatal.
- Installed APK SHA-256 matched build:
  `29171da908f4e3c7ecdba0d5ff3534657c2df4160dad196fc506f1a30f3eb5bd`.
- No injected input, screenshots, visual judging, device suites or benchmarking.
  CONFIGURE left visible for manual assessment of movement and diffusion.
- No changes to information architecture, controls, app selections/morph, storage,
  button/glow, entry/splash, native host/input, runtime or provisioning.
