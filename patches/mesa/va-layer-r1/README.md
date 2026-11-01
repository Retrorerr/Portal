# VA layer (termuxva driver)

Base: lfdevs/mesa-for-android-container tag `mesa-27.0.0-devel-20261025-r1` (eb3e7fd, branch
dev/adreno-main), which carries the termux-va bridge in the gallium VA
frontend. `portal-va-r1.patch` adds:

- Portal's KGSL patches 0001, 0003 and 0004 from `../kgsl-layer-26.3.0`,
  rebased. 0002 is applied too, though r1 already keeps merged submits alive
  through the ioctl (lfdevs 91f7e8c6); moving the bos to the last submit
  costs nothing.
- H.264: learn the PPS `num_ref_idx_l0/l1_default_active_minus1` from slices
  without `num_ref_idx_active_override_flag`. VA-API passes neither, and the
  bridge guessed l0=2, so MediaCodec misparsed every P/B slice of x264
  streams (default 0) and returned one frame.
- AV1: compute `skipModeAllowed` from tracked reference order hints, so
  `skip_mode_present` is written exactly when the source had it. Without it
  Qualcomm's decoder rejected frames ("Unsupported input buffer").
- AV1: replace stream-specific rules keyed on 1920x1080 or BT.709 (Pixel
  camera streams) with universal ones: `enable_restoration` and selectable
  screen-content tools are always declared in the sequence header and each
  frame writes its own values. Hidden frames keep their own
  `allow_screen_content_tools`.
- AV1: submit a held picture as soon as a client waits on it. The bridge
  holds the newest picture until the next descriptor reveals its refresh
  mask, and Firefox syncs each surface before sending the next one, so every
  shown frame stalled 5 s and the EOS drain then broke the reference chain
  (0 frames played). The picture now goes out with refresh 0, and once the
  next descriptor arrives it is decoded again as a hidden frame with the real
  mask; both decodes see the same references. `DMD_AV1_EAGER_FLUSH=1` forces
  this on every picture for testing.
- AV1: write the real `global_motion_params()`. The bridge wrote IDENTITY
  for every reference, so any frame with global motion had its tile data
  misparsed: YouTube AV1 (ads, then the video itself) froze within seconds,
  dav1d and aomdec reject the same units. The VA `wm[]` parameters are now
  coded against PrevGmParams, the set saved with the primary reference
  frame, which the bridge tracks per surface next to the order hints.
- AV1: omit `allow_high_precision_mv` when the frame sets
  `force_integer_mv`; the spec implies it zero there. The extra bit shifted
  the rest of the header, so MediaCodec dropped screen-content frames
  without an error and the client hung waiting for the surface (YouTube AV1
  stopped after a few minutes).
- Gate the per-call `tva-va:` traces behind `DMD_VA_LOG=1`; they wrote
  several lines per decoded frame to every VA client's stderr.
- `vaDeriveImage` on contiguous NV12 surfaces (`DMD_VA_CONTIGUOUS_DMABUF=1`):
  advertise the contiguous-planes cap, also for the profile-less query
  derive makes, and take each plane's real offset in the shared object (the
  bridge page-aligns the chroma plane). VLC 3's VA interop needs derive;
  without it VLC decodes in software.
- Use the contiguous layout by default on the KGSL backend. Chromium imports
  an NV12 export as one dma-buf and read the Y plane as chroma when the
  planes were separate objects (purple/green video). Upstream enabled it only
  without a DRM render node, which Portal's fake `/dev/dri` defeats.
  `DMD_VA_CONTIGUOUS_DMABUF=0` restores the per-plane layout.
- Fail `vaInitialize` when no daemon listens on the endpoint. The VA frontend
  creates the decoder at the first picture, so a missing daemon used to
  surface mid-decode (2 of 120 frames, then errors) and clients never took
  their software path.
- Meson: link freedreno into the standalone VA target and pull in
  xcb-xfixes when GLX is off.
- `TVA_DUMP_UNITS=<file>`: append every unit sent to the daemon (u32 length +
  bytes) for comparing with the source stream.

`build.sh` builds `termuxva_drv_video.so`.
