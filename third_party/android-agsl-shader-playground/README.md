# Android AGSL Shader Playground reference

Upstream: https://github.com/mejdi14/Android-AGSL-Shader-Playground

Pinned revision: `cdb866cbc3192dba326871354b98cbf5036227e5`

MIT, Copyright (c) 2025 Mejdi Hafiene. Full license in `LICENSE`.

`src/android/kotlin/app/polarbear/setup/PortalLaunchTransition.kt` adapts
`RevealContentTransition` in `shaderRippleEffect/src/main/java/com/example/shaderrippleeffect/MixedAnimation.kt`
and the radial displacement concept in `internal/ShaderEffects.kt`.

Portal retains the expanding distance field, smooth edge mask, and radial
sampling displacement. The contour instead uses low-amplitude angular lobes;
the displacement is confined to one glass edge. There are no ripple rings,
rotating pattern, demo tap trigger, or permanent shader loop. The mask uses
premultiplied RGBA, a nonzero soft-edge width, and a safe normal at the origin.
Colors, timing, geometry, blur capture, lifecycle and interaction are Portal-owned.

`MotionBlur.kt` was inspected but no moving-logo blur was incorporated: the
canonical vector stays crisp while its measured bounds move without overshoot.
No library dependency was added. Frost uses Compose GraphicsLayer recording and
Android RenderEffect blur locally, without Cloudy or cross-window capture.
