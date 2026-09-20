# Portal patch: android-activity 0.6.1 with 54 motion axes

Upstream base: `android-activity 0.6.1` (crates.io), otherwise unmodified.

## Why this patch exists

- Stock GameActivity 4.4.0 defines
  `GAME_ACTIVITY_POINTER_INFO_AXIS_COUNT = 48`, so
  `GameActivityPointerAxes.axisValues` only carries axis ids 0..47.
- Android 14 defines the touchpad gesture axes 48..53:
  `AXIS_GESTURE_X_OFFSET`, `AXIS_GESTURE_Y_OFFSET`,
  `AXIS_GESTURE_SCROLL_X_DISTANCE`, `AXIS_GESTURE_SCROLL_Y_DISTANCE`,
  `AXIS_GESTURE_PINCH_SCALE_FACTOR`, and
  `AXIS_GESTURE_SWIPE_FINGER_COUNT`.
- The OnePlus Pad 3 touchpad reports two-finger scroll exclusively through
  axes 50/51, so stock GameActivity drops Portal's real scroll signal
  (reading them panics: `axisValues[50]` with len 48).
- This patch extends the representation to **54**, the minimum count that
  includes every currently defined gesture axis (48..53). Only explicitly
  enabled axes are copied (X/Y by default); Winit enables exactly 50/51 in
  normal builds and enables 48..53 only for the opt-in diagnostic build.

## What was changed

- `android-games-sdk/.../GameActivityEvents.h`: axis count 48 -> 54
  (the native event-copy path derives all sizes/offsets from this define).
- `android-games-sdk/.../GameActivityEvents.cpp`: `static_assert` guards
  (count == 54, `sizeof(GameActivityPointerAxes)` == 232).
- `src/game_activity/ffi_{aarch64,arm,i686,x86_64}.rs`: matching FFI mirror
  update on all four architectures (const, `[f32; 54]`, layout asserts).
- `src/game_activity/mod.rs`: Rust-side `const` layout guard plus
  `portal_axis_patch_tests` range coverage for axis 53.

## Rollback baseline

Tag `spike/game-activity-baseline` (= commit `7e36577`) is the validated
GameActivity-migration baseline: single-window host, 54-axis ABI live, and
physical 50/51 two-finger scroll observed on the Pad 3. Roll back to it if
later transition work regresses input or windowing.
