# CONFIGURE refinement — 2026-09-12

Based on `46b20f036df35e3a239953270d3d8300b46ed504`, branch
`spike/game-activity-host`. Only setup UI/model code changed.

- Header: Install Portal Desktop / Powered by Debian 13 · KDE Plasma.
  Minimal install names bundled functional tools once, without repeating the OS.
- Three blurred canonical-path fragments move in independent 13/17/23-second
  reverse cycles. Only outer graphics-layer transforms animate; warm upper-right
  light is unchanged. No new animation dependency or shader.
- Add apps starts empty and expands in place via SharedTransitionLayout,
  sharedBounds, a shared header, AnimatedContent spring size transformation,
  and updateTransition-driven glass/corners/padding/chevron. The real surrounding
  layout reflows. Outside presses contract it and consume the closing gesture;
  all eight selections remain in local setup state after collapse.
- StatFs queries filesDir's actual filesystem on an IO dispatcher and refreshes
  on resume. Used means total minus app-available bytes (including reservations).
  Decimal installed-footprint estimates are local: 5400 MB baseline plus separate
  per-app values. No download sizes are substituted for installed sizes.
- The capacity bar and numeric line share one animated footprint; free-after is
  derived from the same value. No minimum orange width falsifies proportions.
  A saturated fill and thin top glint improve visibility. Insufficient space is
  clamped to available capacity and explicitly labeled; query failures are shown.
- Footer spacing no longer treats the button's 56dp glow margins as a separate
  tall layout island. The complete BeginInstallButton implementation and glow
  sources match the baseline exactly. Secondary contrast is slightly raised.

Validation:

- Kotlin/JVM model checks passed for every optional-app combination (256) across
  four capacities, covering conservation, exhaustion, app deltas, invalid capacity
  and collapsed summaries. Source: tests/kotlin/SetupModelsTest.kt. With kotlinc:
  `kotlinc src/android/kotlin/app/polarbear/setup/SetupModels.kt tests/kotlin/SetupModelsTest.kt -include-runtime -d target/setup-model-tests.jar`
  then `java -jar target/setup-model-tests.jar`.
- Repository xbuild debug/arm64 APK build passed; adb install -r succeeded on Pad 3.
- One force-stop / cold launch: status OK, 597ms; process remained alive/resumed.
- Actual app-data capacity read: total 495483752448, available 262435172352 bytes.
- Logs: splash removal confirmed 13:21:25.965; aperture started 13:21:25.980;
  CONFIGURE interactive and intro effects released 13:21:27.199. No immediate fatal.
- Installed APK hash matched the built APK:
  `155ac180fae6b53369add41064713362cc5f1b8236ef53da13227c85c074b12f`.
- Host, input patches, splash/entry implementation, native runtime and provisioning
  files have no diff from the requested baseline. No package/backend integration.
- No injected input, automated gestures, screenshots, pixel judging, broad device
  suites or benchmarks. CONFIGURE left visible; morph/visual acceptance is manual.
