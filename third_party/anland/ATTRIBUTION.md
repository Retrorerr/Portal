# Project Anland — upstream source attribution & license boundary

Portal branch `gpu-anland` integrates the *concepts and wire protocol* of
upstream Anland. This directory vendors the minimal stable contract needed to
keep Portal's clean-room Rust implementation byte-compatible with the
reference C implementation and the prebuilt guest binaries.

## Origins (pinned)

| Component | Origin | Ref | SHA / tag |
|---|---|---|---|
| `common/protocol.h`, `common/socket_utils.*` | `SuperTurtleDev/anland` (`legacy` branch) | wire spec V3→V5 | `legacy` HEAD `aab290e015ca9e28fa64ad6c4ad127250fc4da1b` (2026-09-08) |
| `consumer/display_consumer.h` (API reference) | `SuperTurtleDev/anland` (`legacy`) | consumer state machine | same as above |
| `producer/display_producer.h` (API reference) | `SuperTurtleDev/anland` (`legacy`) | producer state machine | same as above |
| `consumer/native_consumer.c` (design reference, NOT vendored) | `lfdevs/anland-termux` (`main`) | window dequeue/collect/queue flow | `main` HEAD `23c77435032e7d9df00f40b90ecee04f1fc4ad9e` (2026-09-02) |
| `app/src/main/jni/anw_hidden.h` | `lfdevs/anland-termux` (`main`) | hidden ANativeWindow ABI | same as above |
| KWin `anland_backend_debian13_v5` (design reference, NOT vendored) | `SuperTurtleDev/anland` (`legacy`) | EGL import + fence flow | same as anland row |
| `producers/kde/Debian13_v5/{build.sh,kwin.patch,startup.sh}` (reference) | `SuperTurtleDev/anland` (`legacy`) | guest launch env | same as anland row |

Full C implementations (`display_consumer.c`, `display_producer.c`,
`daemon/`, `native_consumer.c`, backend `.cpp`) are **not** copied into
Portal; Portal reimplements the consumer/broker in Rust
(`src/android/anland/`). Fetch them from the URLs above for audit.

## Guest binaries (prebuilt, device-side for the milestone)

| Binary | Origin | URL |
|---|---|---|
| `kwin_anland-5.13-debian-4_6.3.6-95.zip` (KWin 6.3.6 + Anland backend) | `lfdevs/anland-termux` release `5.13.3` | `https://github.com/lfdevs/anland-termux/releases/download/5.13.3/kwin_anland-5.13-debian-4_6.3.6-95.zip` |
| `xwayland_24.1.6-91_arm64.deb` (KGSL surfaceless XWayland) | `lfdevs/anland-termux` release `5.13.3` | `https://github.com/lfdevs/anland-termux/releases/download/5.13.3/xwayland_24.1.6-91_arm64.deb` |
| `mesa-for-android-container_26.2.0-devel-20260709_debian_trixie_arm64.tar.gz` (freedreno/KGSL Mesa 26.2) | `lfdevs/mesa-for-android-container` | `https://github.com/lfdevs/mesa-for-android-container/releases/download/mesa-26.2.0-devel-20260709/mesa-for-android-container_26.2.0-devel-20260709_debian_trixie_arm64.tar.gz` |

## Licenses

- Upstream `SuperTurtleDev/anland`: root `LICENSE` file is **GPL-3.0** (GitHub
  tags the repo GPL-3.0). Its README additionally grants MIT for
  `common/`, `daemon/`, `libdisplay_*`, `consumers/anland_v3/`, with the
  rule that a vendored copy embedded in a GPL host follows the host license.
  Portal is GPL-3.0: compatible under either reading. Preserved verbatim
  below with origin headers; modifications (none yet) will be marked.
- `lfdevs/anland-termux` (incl. `anw_hidden.h`, `native_consumer.c` design):
  **GPL-3.0**. Concepts reimplemented, not copied; `anw_hidden.h` vendored
  verbatim as the ABI reference with its origin header.
- KWin backend reference: **GPL-2.0-or-later** (KDE). Guest uses lfdevs
  prebuilt binaries; no KWin source vendored.
- `lfdevs/mesa-for-android-container`: Mesa terms (**MIT** plus per-file
  Apache-2.0/BSL/SGI-B-2.0/GPL bits, see Mesa `licenses/`). Guest uses the
  prebuilt tarball; no Mesa source vendored.

## Boundary rules

- `third_party/anland/` holds verbatim upstream text + this file. Do not add
  Portal-native code here.
- Portal-native implementation lives in `src/android/anland/` (Rust) and is
  Portal GPL-3.0 code that *implements* the protocol documented here.
- Never copy the `anland-termux` Android APK/app code into Portal; only the
  documented buffer/fence/socket concepts are reused.
