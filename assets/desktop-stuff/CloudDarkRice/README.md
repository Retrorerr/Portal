# Cloud Dark rice for Portal

The laptop's Cloud Dark KDE rice, ported to Portal (Debian 13, Plasma 6.3).

## Apply

Double-click **Apply Cloud Dark rice** in this folder. It opens Konsole and runs every step; it takes a
minute or two and the bar restarts a couple of times. Then log out (Portal closes) and open Portal again.
The first run downloads the icons, fonts and wallpaper (25 MB) from Portal's GitHub release.

From Konsole you can also run single steps:

    bash ~/Desktop/Stuff/CloudDarkRice/apply-portal.sh            # everything
    bash ~/Desktop/Stuff/CloudDarkRice/apply-portal.sh theme bar  # only these steps
    bash ~/Desktop/Stuff/CloudDarkRice/apply-portal.sh restore    # undo (config backed up on the first run)

Steps: `packages theme windows outline bar power wallpaper apps terminal keys`

| Step | What it does |
|---|---|
| packages | Inter + JetBrains Mono fonts, the bundled JetBrainsMono Nerd Font, python3-dbus/gi (Panel Colorizer), fastfetch, rofi, imagemagick, jq, System Monitor, Alacritty |
| theme | CloudDark colours, Breeze style, Gruvbox Plus Dark icons, Capitaine Gruvbox cursor, Inter UI font, GTK Breeze, the one-colour K launcher icon |
| windows | 7 numbered desktops in one row |
| outline | the 2px cloud-grey active-window outline (KDE-Rounded-Corners effect, built for Portal's KWin) |
| bar | top bar (launcher, desktops, clock, now playing, tray, power) and the floating dock on the left |
| power | full-screen rofi power menu over the blurred wallpaper |
| wallpaper | the laptop's wallpaper |
| apps | Dolphin / Konsole without menu bars, volume above 100%, night colour; turns off Baloo's file-tags worker (Portal has no file index, and it popped up a "Could not enter folder tags:/" error at every start) |
| terminal | Konsole Gruvbox colours + Nerd Font profile, fastfetch config, Alacritty colours |
| keys | the shortcuts below |

## Keys

| Keys | Action |
|---|---|
| Meta (tap) | Open / close the launcher |
| Meta+Esc, bar power button | Power menu (shut down / restart / log out) |
| Ctrl+Esc | System Monitor |
| Meta+Q, Alt+F4 | Close window |
| Meta+←/→/↑/↓ | Snap window to half / maximise |
| Meta+Shift+←/→ | Move window to previous / next screen |
| Meta+1…7, Ctrl+F1…7 | Switch to desktop N |
| Meta+Shift+1…7 | Send window to desktop N |
| Meta+Ctrl+←/→/↑/↓ | Previous / next desktop |

## Different from the laptop

- The dock is on the left edge instead of the bottom.
- The power menu has no lock or suspend: Portal has no lock screen, and Android handles sleep.
- Shut down, restart and log out all end the Plasma session and close Portal; open Portal again to start a fresh one.
