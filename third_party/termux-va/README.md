# termux-va (vendored)

Source: https://github.com/lfdevs/termux-va at `ae14813`, with two Portal
changes:

- `common/tva_protocol.h`: one comment no longer names the upstream guidance
  file.
- `daemon/termux-va.c`: SHM decode sessions render into an `AImageReader`
  instead of using ByteBuffer output. In ByteBuffer mode Codec2 converts
  every graphic block into a linear buffer before the daemon copies it into
  the slot (`CopyRow_NEON`, ~40% of the daemon's CPU at 4K). Now the daemon
  copies each frame once, from the image into the slot. Qualcomm renders UBWC
  unless a color format is requested, so the session asks for
  YUV420SemiPlanar and reads the linear Venus NV12 layout (UV at stride x
  ALIGN(height, 32)). Other vendors' buffers go through the `AImage` planes.
  `TERMUX_VA_OUTPUT=buffer` restores ByteBuffer output, which is also the
  fallback when the reader or configure fails and for inline transport.

GPL-3.0-or-later (see `LICENSE`); the daemon derives from
Re-s/droidspaces-media-decode (Apache-2.0), as its header notes.

- `daemon/termux-va.c`: the Android-side MediaCodec decode daemon (DMD
  protocol v3). It listens on a Unix socket in the guest's `/tmp` and returns
  decoded NV12 frames through a shared-memory slot pool.
- `common/tva_protocol.h`: the wire protocol, shared with the guest's VA
  driver (lfdevs Mesa, `src/gallium/frontends/va/tva_*`).
- `tools/tva-probe.c`: a self-test that decodes a few synthetic frames
  through MediaCodec.

`scripts/build-portal-va-daemon.sh` builds them into
`assets/libs/arm64-v8a/libportal_va_daemon.so` and `libportal_va_probe.so`;
`src/android/backend/video_decode.rs` runs the daemon.
