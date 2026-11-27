# termux-va (vendored)

Source: https://github.com/lfdevs/termux-va at `ae14813`, with these Portal
changes:

- `common/tva_protocol.h`: one comment no longer names the upstream guidance
  file, and it defines the dma-buf frame extension below.
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
- dma-buf frames: a client that offers `TVA_CLIENT_CAP_DMABUF` on the SHM
  handoff socket gets each decoded frame as the decoder's own buffer (the fd
  in SCM_RIGHTS, offset and size in an 8-word control message) instead of a
  copy in a slot. The image stays acquired until the client resets the
  slot's state word. The fd comes from `AHardwareBuffer_getNativeHandle`
  (resolved at run time), and each buffer's layout is proven once: the
  gralloc lock address is looked up in `/proc/self/maps`, and its inode must
  be one of the handle's fds. Buffers that don't check out, kernels whose
  dma-bufs share one inode, old clients and `TERMUX_VA_DMABUF=0` keep the
  SHM copy. The reader holds 6 images in these sessions.

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
