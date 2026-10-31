# termux-va (vendored)

Source: https://github.com/lfdevs/termux-va at `ae14813`. Only one comment in
`common/tva_protocol.h` differs: it no longer names the upstream guidance file.
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
