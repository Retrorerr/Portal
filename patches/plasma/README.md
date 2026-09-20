# Portal Plasma 6.7.4 preload/readiness overlay

This directory contains the narrow Portal overlay used for the Plasma 6.7.4
regression investigation on the ARM64 debug runtime. The final behavior keeps
the normal restored-layout panel scheduling: `PanelView` construction and
visibility still wait for the panel containment's own `isUiReady()` gate, and
desktop readiness/KSplash sequencing, screen mapping, runtime screen changes,
and animations remain stock.

The functional fix restores libplasma's Plasma 6.3.6 adaptive preload-weight
persistence. Plasma 6.7.4 removed the persisted `PreloadWeight`, leaving
ordinary applets at weight 50 and scheduling synchronous
`preloadForExpansion()` calls after containment readiness. The overlay restores
the bounded increment/decrement behavior and retains `PreloadWeight` in the
applet config. It does not set UiReady state, remove or reorder applets, or
use `PLASMA_PRELOAD_POLICY=none`.

The overlay also carries timestamped `PORTAL_PRELOAD`, `PORTAL_STARTUP`,
`PORTAL_TRAY`, and `PORTAL_EVENT_LOOP` diagnostics. The system-tray changes are
instrumentation only: they time metadata enumeration, registry initialization,
nested-containment readiness, and tray initialization. They do not change tray
preloading or representation ownership.

The overlay is staged under `/usr/local/lib/portal-plasma`; the distro Plasma
files are not replaced. `startplasma-localdesktop` opts into it only when all
required private assets are present. The two patch files are ordinary
`git apply` patches against the following upstream revisions:

- `plasma-workspace` tag `v6.7.4`, commit `fd05f4c88ab093aee23ce137bf6f2412437c9bba`
- `libplasma` tag `v6.7.4`, commit `b1e346733ff527433e1849eb85305580d10e712a`

Relevant upstream comparisons:

- libplasma `c65a6c954def1180fd16d7c9219bca48478737d6`, “Drop dynamic preload weight adjustments”
- plasma-workspace `46ada24fd8c45cdab95bc769847bdd04682f7462`, “applets/systray: port to a nested containment”

Shipped ARM64 assets:

- `plasmashell`: 26,649,232 bytes, SHA-256 `B520986083036BEA184071E2A00E49C7CF30DF55BAD99CA0A89CDF02D0646155`
- `libPlasma.so.6.7.4`: 10,974,824 bytes, SHA-256 `3D6A1CAD851F62CF47E5A18FD3DB52BCB6C1D0993792F83AC694D060E101CF80`
- `libPlasmaQuick.so.6.7.4`: 16,256,312 bytes, SHA-256 `BD429E184D40E72EDF9E60C7C40321C06A88E6E2FEDEF3E901C9A321FE0B8C9A`
- `org.kde.plasma.systemtray.so`: 26,220,256 bytes, SHA-256 `51E002C64A67F8CDD1E1A05D7A939EDCF22AB249784938A1AA455F5B09389A32`

Use `scripts/measure_cold_launch.ps1` for the clean debug-app startup harness
and `scripts/analyze_plasma_preload.ps1` to parse per-applet preload/full-
representation durations and event-loop stalls from its `plasma.log` output.
