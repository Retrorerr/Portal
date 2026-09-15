# Portal Anland v3 KWin overlay

This directory is the complete source overlay for Portal's active KWin
6.7.4 Anland backend. It contains the backend sources plus the full contents
of every KWin file that is intentionally changed by the Android/PRoot
integration. The overlay is applied after numbered patches `0001` through
`0005` with `scripts/apply_kwin_forky_anland.sh`.

The overlay pins the producer/consumer contract at protocol version 3:

* `PRODUCER_HELLO` negotiates the version and required frame-work and
  no-damage features before any screen or buffer message is accepted.
* `FRAME_WANTED(sequence,generation)` is explicit producer work. A display
  tick cannot invent a frame.
* `NO_DAMAGE(sequence,generation)` carries no fence and causes the selected
  buffer to be cancelled rather than queued as an identical frame.

The active presenter remains Anland dmabuf transport plus KWin's accelerated
surfaceless EGL context. This overlay does not restore the retired DRM shim,
does not enable a software fallback, and does not change output scale policy.
