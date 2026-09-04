---
title: The folder `patches`
---

The `patches` folder at the repository root contains source code for libraries and tools that Portal customizes.

Portal is built on several niche, fast-moving technologies. When a dependency needs an update, its source is patched locally so development can continue while a change is prepared for upstream.

What have we patched so far?

## xbuild
- [Support `use_cleartext_traffic` in `AndroidManifest.xml`](https://github.com/localdesktop/localdesktop.github.io/commit/a24091b1bb90ac16e680dcdb142db89a5ab86d88#diff-b65b875cc3637c77b2ef2e894f105babcf46a2fda6c065ba1a306fecd143cefc). During setup, Portal shows documentation in a WebView and reports progress via a loopback WebSocket. Without this option, the WebSocket will not work.
- [Support `extract_native_libs` in `AndroidManifest.xml`](https://github.com/localdesktop/localdesktop.github.io/commit/5f1400b2d4feff70ca47ebc9259113ecb71d6d57). Portal invokes the PRoot binary from the native libraries folder, so those libraries must be extracted to the filesystem rather than loaded only from the APK.
- [Patch `gradle` build process](https://github.com/localdesktop/localdesktop.github.io/blob/main/patches/xbuild/xbuild/src/gradle/mod.rs):
  + Pick up `assets` when building with gradle.
  + Support signing if a `release-key.jks` is provided.

## smithay

- Load `libEGL.so` instead of `libEGL.so.1`.
- Create a dummy 1x1 pbuffer surface and use it as both the draw and read surface when calling `eglMakeCurrent` to avoid the `EGL_BAD_MATCH` error. More details [here](/docs/developer/bug-cheat-sheet/egl-context#egl_bad_match).

The content of the patched code can be found [here](https://github.com/localdesktop/localdesktop.github.io/commit/58ffc6fc37da2d799db0d68b8549abe57fa2e636).

## build-proot-android & build-libxkbcommon

The patched code and configuration enable cross-compilation where the upstream repositories were designed only for host builds. These target-specific changes are maintained in Portal when they do not fit upstream.
