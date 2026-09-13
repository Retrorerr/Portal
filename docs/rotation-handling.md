# Anland rotation handoff

The Android surface can resize without recreating Portal's Activity or native
window. Updating Smithay's output and input transform alone does not resize the
Anland buffer set. KWin's Anland EGL importer updates its output mode from the
dimensions in the new `BUFS_READY` set, not from a new broker `SCREEN_INFO` on
each reconnect. The existing packaged KWin supports this path.

The previous rotation commit (`fba0a3d`) rebound the queue on the Android event
thread. Its inflight counter started after dequeue, and its drain timeout could
proceed while a frame still owned a buffer. It also cancelled collected slots
that had already been queued. Teardown closed socket masters while polling
threads retained duplicates, and a fresh deposit became visible before the
handshake waiter subscribed. On the Pad, the old implementation logged a new
deposit after rotation with no subsequent producer attachment, leaving black
buffers on screen.

The corrected handoff:

- The event thread requests the newest agreed size and ends delivered key,
  button and finger-scroll state. It does not manipulate live queue slots.
- The render thread finishes its current frame, preserving its GPU fence, then
  performs the resize between frames. Only the held, dequeued spare is cancelled.
- Socket shutdown explicitly wakes old peers, including through duplicated
  descriptors. Generation replacement is serialized; stale failures cannot
  tear down a replacement connection. Error paths release actual mutex guards.
- The handshake waiter subscribes before publishing the new deposit. Collected
  buffer dimensions must match the requested dimensions before publication.
- Input anchors reset at the geometry boundary. Only the first presented frame
  confirms the surface generation. A rapid return to the old size supersedes an
  unpresented intermediate request instead of incorrectly coalescing it.

No guest restart, KWin binary replacement, Plasma scale change, or runtime
replacement is required.

## Validation (2026-09-13)

Portal Debug (`app.polarbear`) built and installed with `adb install -r` on the
connected OnePlus Pad 3. Android reported `LaunchState: COLD` after force-stop.
The local APK and installed base APK both hashed to:

`eac5ea77e05c1a498d9014935083d930bd36aeb678cf979ee401d10e668a7d57`

The device then reported repeated `3392x2400` / `2400x3392` changes without any
rotation commands from the agent. Generations 2 through 6 each attached and
presented a fenced frame at the requested size. For example, generation 2 began
at 14:20:56.310, attached at 14:20:56.519, and presented at 14:20:56.629. A
portrait screenshot showed the complete Plasma desktop and bottom panel.

57 focused host tests passed: 13 surface-generation tests, 4 Anland wire/input
tests, and 40 existing coordinate-transform, presentation-resize and pointer
button tests. The latter ran against the actual core source modules through a
temporary rustc harness, avoiding unrelated host packaging dependencies.
Subjective cursor alignment and interaction remain part of the user's manual
test. The app was left open after the verified cold launch.
