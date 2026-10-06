# Setup app icons

`src/android/res/drawable-nodpi/portal_app_*.png` are the icons the first-run
setup screen shows for included and optional apps. Each is the icon that app
already uses inside Portal's Plasma desktop. They were taken from the Debian 13
packages Portal installs and resized to 144×144 PNG. Nothing else was changed.

## KDE icons

| File | Source |
| --- | --- |
| `portal_app_konsole.png` | Breeze `apps/48/utilities-terminal.svg` |
| `portal_app_kate.png` | Breeze `apps/48/kate.svg` |
| `portal_app_okular.png` | Breeze `apps/48/okular.svg` |
| `portal_app_gwenview.png` | Breeze `apps/48/gwenview.svg` |
| `portal_app_ark.png` | Breeze `apps/48/ark.svg` |
| `portal_app_kcalc.png` | Breeze `apps/48/kcalc.svg` |
| `portal_app_systemsettings.png` | Breeze `apps/48/systemsettings.svg` |
| `portal_app_dolphin.png` | Dolphin `hicolor/scalable/apps/org.kde.dolphin.svg` |

Breeze icons: https://invent.kde.org/frameworks/breeze-icons, Debian package
`breeze-icon-theme` 4:5.116.0-1. Copyright 2014 Uri Herrera and the KDE
Visual Design Group. LGPL-3.0-or-later; full text in `LGPL-3.0.txt`.

Dolphin icon: https://invent.kde.org/system/dolphin, Debian package `dolphin`
4:25.04.3-1+deb13u1. Copyright the Dolphin developers. GPL-2.0-or-later.

## Other application icons

| File | Application |
| --- | --- |
| `portal_app_firefox.png` | Firefox (Debian `firefox-esr`) |
| `portal_app_thunderbird.png` | Thunderbird |
| `portal_app_libreoffice.png` | LibreOffice |
| `portal_app_vlc.png` | VLC media player |
| `portal_app_gimp.png` | GIMP |
| `portal_app_krita.png` | Krita |
| `portal_app_inkscape.png` | Inkscape |
| `portal_app_chatgpt.png` | ChatGPT desktop app |
| `portal_app_claude.png` | Claude desktop app (`claude-desktop`) |
| `portal_app_steam.png` | Steam (`hicolor/256x256/apps/steam.png` from Valve's `steam.deb` installer, also staged as `assets/steam/steam-256.png` for the guest launcher) |

These are each project's own upstream icon as shipped in its Debian package
(or, for ChatGPT and Claude, the icon in the vendor's official .deb). Names and logos are trademarks of
their respective owners. Portal shows them only to identify the application it
will install, and is not affiliated with or endorsed by any of them.
