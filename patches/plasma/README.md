# Portal Plasma panel-startup overlay

This directory contains the narrow Portal patch used with Plasma 6.7.4 on the
ARM64 debug runtime. It changes only the restored-panel construction boundary
and adds `PORTAL_STARTUP` timestamps. The panel containment's own `isUiReady()`
visibility gate, desktop readiness/KSplash sequencing, screen mapping, and
runtime screen-change paths remain intact. No Plasma animation is changed.

The overlay is staged under `/usr/local/lib/portal-plasma`; the distro
`plasmashell` and `libPlasma` files are not replaced. `startplasma-localdesktop`
opts into the overlay only when both private files are present.

Source revisions used for the ARM64 build:

- `plasma-workspace` tag `v6.7.4`, commit `fd05f4c88ab093aee23ce137bf6f2412437c9bba`
- `libplasma` tag `v6.7.4`, commit `b1e346733ff527433e1849eb85305580d10e712a`

The two patch files are ordinary `git apply` patches against those revisions.
The shipped ARM64 assets are:

- `plasmashell`: 26,637,752 bytes, SHA-256 `8887861ab6cd31ab4eb288fbf3ff80e40d2c4375e73be8918c71a44f463a875b`
- `libPlasma.so.6.7.4`: 10,864,256 bytes, SHA-256 `6070f8619b4992074b5985b0d673b7504b597596f12bdc29efe5249ef942385f`

The next tray pass should instrument `PlasmoidRegistry::init()`,
`PluginLoader::listAppletMetaData(QString())`, nested tray containments, and
`PlasmoidItem.qml` before changing any tray behavior.
