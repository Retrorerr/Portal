# Release readiness audit — 7 September 2026

Status: release-profile testing build, **not a passed stable release gate**.
The remaining measured blocker is Firefox 1080p60 playback. No release was
pushed or tagged. Changes include the existing local working tree and remain
uncommitted for review.

## Proven problems and changes

- KWin's SHM damage already limited texture uploads, but Portal discarded that
  benefit at the final stage by clearing, drawing and submitting the complete
  3392x2400 Android target for every dirty event. A Smithay
  `OutputDamageTracker` now combines KWin, cursor and overlay damage, restores
  buffer-age history and supplies the same exact rectangles to GLES and EGL
  swap-damage. Resize, scale drift, renderer loss and resume invalidate history
  and force one correctness-first full repaint.
- Portal sent `wl_surface.frame` callbacks only after the vsynced EGL swap
  returned. Because swap can wait on Android's buffer queue, this serialized
  KWin's next CPU-rendered frame behind host presentation. Callbacks now flush
  after the current render and before the blocking submit. Current client
  buffers stay retained until submit returns, and Wayland dispatch remains on
  the same thread, so the change permits one frame of preparation overlap
  without allowing KWin to replace a buffer still being presented.

- Startup, resize and refresh polling could copy Android's instantaneous
  refresh into the nominal Wayland mode. Nominal refresh now comes from
  supported modes (144 Hz on this Pad 3); observed physical refresh is separate.
  Resume recreates the Android request without changing the nominal output.
- Sustained approximately 45–65 compositor commits/sec now selects the highest
  supported 60 Hz multiple, with a two-second observation window, four-second
  minimum between changes and six-second return hysteresis. This requests
  120 Hz on the Pad 3. It is demand evidence, **not detection of source video
  FPS**. The supported NDK DEFAULT/seamless hint cannot force OxygenOS policy.
- The Wayland socket watcher published its wake before arming the acknowledgement
  flag. An immediate dispatch could lose the acknowledgement and park the worker.
  It now arms under the mutex before publishing. Dispatch/flush errors also
  resume polling. No damage or presentation ownership redesign was introduced.
- Audio startup could publish children after a concurrent suspend had cleaned
  their sockets. Startup and teardown now serialize off the Android lifecycle
  thread and honor the latest requested state.
- The libcanberra PulseAudio backend was missing. A pinned Debian ARM64 module
  and copyright notice are bundled and installed atomically by setup. The image
  and Debian package database are unchanged; the module is reproducible with
  `scripts/build_canberra_backend.py`.
- The session waits for the single Portal audio backend before launching Plasma.
  The Breeze splash is restored once, obsolete splash/wait stubs removed, and
  the existing bounded startup/readiness failure flow retained.
- Provisioning gains free-space preflight, range download recovery, verified
  staged-promotion recovery and clearer setup/retry guidance. One previous
  runtime is preserved. A local edit that deleted that backup was corrected.
- Release backtraces and verbose host/Wayland/input tracing are off by default.
  Active guest logs are bounded without replacing an inode still held open by
  children. IME debug output no longer includes typed text. Recovery markers stay.
- The old always-on guest command FIFO is removed. Security documentation now
  states the actual PRoot, browser sandbox, storage and bridge boundaries.
- Unavailable Discover/Kontact references are removed from stock defaults;
  existing panel migration waits until a panel configuration actually exists.

## Local changes integrated

Retained the local modifier/key bookkeeping, duplicate pointer-press handling,
resize touch reanchoring, border-touch suppression, injected Shift preservation,
GL import cleanup, Android buffer validation, machine-ID repair, best-effort
desktop cache setup and incomplete-runtime recovery changes. Reviewed them
against the existing lifecycle and provisioning contracts.

The already-solved SHM ghosting, scaling and configure-unit contracts, pointer
transform, nested clipboard broker, Android IME protocol integration and KWin
touchpad settings/gesture integration were preserved. No VA-API, DRM/GBM,
unsafe partial-damage path, OEM setting manipulation or graphics rewrite added.

## Evidence from the connected OnePlus Pad 3

- The current Portal Debug build boots Plasma with an internal mode of
  `2714x1920@144`, geometry `1357x960`, scale 2. Portal's Android EGL target
  remains the native 3392x2400 surface and performs the final aspect-correct
  upscale. Android's physical mode during the earlier playback measurement was
  approximately 60 Hz. Nominal Wayland mode, render size and physical VRR are
  kept as distinct values.
- Android accepts both the 144 and 120 Hz native-window requests with status 0;
  accepted does not establish a physical 120/144 Hz transition.
- Installed KWin SHA-256 matches the bundled patched library:
  `3b012c15c4f70124801dae1cc63dfd33d233dee059e364013c4c956ea1080e31`.
  Installed libcanberra module SHA-256:
  `17565584fda22200fa5c1011b60f695cbb5f528ae7f807da3b87b2afcb3a891d`.
- Breeze KSplash configuration is enabled, desktop startup reaches Android
  presentation readiness and no splash hang reproduced. The animation itself
  and audible login sound still need physical confirmation.
- There was exactly one PipeWire, WirePlumber, AAudio sink and Pulse bridge.
  libcanberra context creation, pulse-driver selection, open and Ocean login
  sound playback returned success; playback was active after one second.
  This proves software acceptance, not that a human heard it.
- Firefox 140.12 uses software WebRender; its graphics diagnostics show
  llvmpipe and its audio backend is pulse-rust targeting Portal's AAudio output.
- Latest warmed H.264 1920×1080@60 capture, displayed at native pixel size:
  17.529 seconds, 1,050 video frames, 316 dropped, **41.87 rendered fps**.
  Android SurfaceFlinger presents approximately **50.07 fps**, while the
  physical display is **60 Hz**. Buffered content stays 43–67 seconds ahead.
  This fails the 1080p60 gate independently of network starvation.
- An earlier larger displayed video measured 32.36 rendered fps. These are
  bounded samples, not controlled comparative benchmarks. They suggest
  software rendering/scaling cost; they do not isolate decode versus browser
  compositing versus host upload/pacing. A CPU sample showed Firefox consuming
  more CPU than either KWin or Portal, but it was not a sampled call-stack profile.
- A YouTube run reset with a generic playback error. Its counter-delta capture
  is explicitly invalidated, not reported as a performance result.
- No new input-to-photon or queue-to-present latency claim is made. The earlier
  user-provided latency figures are not treated as measurements of this build.
- The output-damage build reached the normal Plasma desktop at 3392x2400 after
  the Breeze startup splash; the initial black splash frame was not a rendering
  failure. No stale-buffer or damage artifact was observed in this bounded
  startup check. Subjective motion testing remains with the user.

Raw local evidence is under `artifacts/release-readiness/` (intentionally not
committed). The buffered H.264 source was the
[Kodi Big Buck Bunny mirror](https://mirrors.mit.edu/kodi/demo-files/BBB/bbb_sunflower_1080p_60fps_normal.mp4).
Android documents the request as a preference in its
[frame-rate API guidance](https://developer.android.com/media/optimize/performance/frame-rate).

## Clean-install contract

The APK pins runtime `debian13-arm64-2026.09.05.3`, 896,188,212 compressed bytes,
SHA-256 `aa75ea96300c26a9cfdffb443aff954a3cbe89146ffe32bba7287415e89e00f3`.
The public asset and manifest were verified. No runtime image contents changed
in this pass and no new runtime publication is required.

First launch explains the 855 MiB download, approximately 4.7 GiB space budget,
and Android Developer Options → Disable child process restrictions. It checks
space, reuses/resumes downloaded bytes, verifies size/hash, safely extracts to
staging, validates completion and promotes atomically. It then installs bundled
guest integrations and automatically starts Plasma. Partial extraction cannot
be launched. Retry retains reusable download bytes; an interrupted complete
promotion can recover staging; replacement preserves one previous runtime.
Subsequent launches reuse the completed image and synchronize APK-owned setup.

All guest fixes are in APK assets/setup/session logic. Diagnostic probes and
temporary Firefox profiles do not provide any product fix. The tablet was
updated with `adb install -r`; **a destructive uninstall/reinstall was not
performed**, so fresh-install device acceptance remains pending. Host tests
cover download resume/server range refusal, corrupt/incomplete state and
promotion/backup behavior.

## Remaining gates and platform limits

1080p60 is still a measured blocker; 1440p60 is not accepted. A/V sync, audible
system/browser audio, rapid physical suspend/resume, fullscreen/popup, scaling,
ghosting, touchpad/tap/scroll settings and gestures, keyboard attach/detach,
tablet IME and clipboard need the user's short physical acceptance pass. Host
tests and source preservation do not establish those physical behaviors.

Stock OxygenOS owns final physical refresh selection. PRoot is not a security
boundary between guest applications. Browser namespace sandboxes cannot work
normally here; shared storage is only as isolated as the granted Android
permission; a compromised guest shares Portal's UID and bridge credentials.
Conventional guest `/dev/dri`/GBM hardware video acceleration is unavailable.
See `SECURITY.md` for the release threat model.

### Performance decisions after source audit

The candidate preserves SHM generation checks while carrying damage through
KWin/Smithay, GLES scissoring and EGL swap damage. It also removes the
callback-after-swap serialization, so KWin can prepare the next frame while the
current Android submit completes.

Portal now requests an 80% linear KWin render target. On the Pad 3 this is
2714x1920, then one GLES draw upscales to the native 3392x2400 Android surface.
The expected initial 25% larger Plasma UI follows from the smaller logical
geometry and can be adjusted with Plasma Display scaling. Coordinate and frame
attribution tests cover native input mapping, scale transitions and the reduced
buffer size. The live `kscreen-doctor` result above proves the intended mode
reached the installed guest.

Dirty Wayland work is now coalesced onto Android's next Choreographer callback
before requesting a winit redraw. The Android API is resolved dynamically, with
the existing immediate event-driven path retained as the compatibility fallback.
This avoids arbitrary sleeps and preserves the 144 Hz nominal policy. Initial
resume remains immediate so startup is not delayed by a callback that may not
arrive while the surface is being created.

The Android display/event/render thread receives a best-effort elevated nice
priority only after guest and audio workers are spawned, avoiding an accidental
priority inheritance across the whole process. The running Pad process reports
the main thread at nice -10. No CPU affinity, privileged scheduler class or OEM
setting is used.

Portal already contains the host half of a GPU-native route through
`android_wlegl` and AHardwareBuffer-to-EGLImage import. Current KWin produces
QPainter `wl_shm` buffers and device logs expose neither guest GBM nor dma-buf
import, so standard Linux dma-buf/VA-API work cannot activate it. The practical
future route is a contained KWin Android-buffer allocator/export backend that
targets Portal's existing protocol. Until such a guest producer exists, the
current path has one shared-memory GLES upload per damaged region and no extra
Portal CPU-side pixel copy.

The stable APK uses optimization level 3 and statically caps Rust `log` and
Android `tracing` at warnings in release builds. Its manifest is not debuggable.
Portal Debug uses a separate manifest, the historical development package,
debug Rust profile, full diagnostics and a visibly badged launcher icon. Static
library inspection confirms per-frame debug strings are absent from stable and
present in debug. Both were signed with the existing local development
certificate for local installation; this is not public-release signing.

## Final build and installation

The current pass ran 232 focused host test cases across the default and debug
feature sets. Android ARM64 stable and debug builds succeeded. The stable APK
verifies with v1/v2/v3 signatures; the Gradle debug APK verifies with v1/v2.
Icon safety checks and `git diff --check` passed.

Both artifacts were installed successfully on the connected OnePlus Pad 3:

- Portal: package `app.polarbear.portal`, not debuggable,
  `target/Portal.apk`, SHA-256
  `94b412240f7be7b1ad179c22fd3b8f672670b889e3f0a6aa9ebb80abb2855fbf`.
- Portal Debug: package `app.polarbear`, debuggable,
  `target/Portal-Debug.apk`, SHA-256
  `9d9d35f7410aed7ef2480052d4befd240692d9345fb664c25e3d1ddc95c6d58d`.

On-device hashes exactly match both local files. Portal Debug was launched for
the objective mode and process checks. Stable Portal was uninstalled and then
freshly reinstalled without launching it; Android reports `stopped=true` and
`notLaunched=true`, so the user's first launch exercises the independent
clean-install flow.
