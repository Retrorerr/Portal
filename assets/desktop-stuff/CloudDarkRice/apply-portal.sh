#!/usr/bin/env bash
# Cloud Dark rice for Portal (Debian 13 + Plasma 6.3.6 in PRoot, no systemd).
# Runs INSIDE the Portal desktop session: double-click "Apply Cloud Dark rice" in
# ~/Desktop/Stuff/CloudDarkRice, or from Konsole:
#   bash ~/Desktop/Stuff/CloudDarkRice/apply-portal.sh            # all steps
#   bash ~/Desktop/Stuff/CloudDarkRice/apply-portal.sh theme bar  # only these
#   bash ~/Desktop/Stuff/CloudDarkRice/apply-portal.sh restore    # put back the backup taken on the first run
# Steps: backup packages theme windows outline bar power wallpaper apps terminal keys
# Differences from the laptop: the dock sits on the left; the power menu has no lock / suspend (Portal has
# no lock screen or logind); the outline effect is a build for Portal's own KWin (data/prebuilt).
set -euo pipefail

R=$(cd "$(dirname "$0")" && pwd)      # bundle: files/ cache/ overlay files
D=$R/data                              # everything the script installs
F=$D/files
STATE=~/.local/share/rice-portal-state
mkdir -p "$STATE"

# --- tunables (logical px; Pad 3 runs 3392x2400 at 2.625 => 1293x915 logical) ---
BAR=30
DOCK=44
DESKTOPS=7
UI_FONT="Inter,10,-1,5,400,0,0,0,0,0,0,0,0,0,0,1"
MONO_FONT="JetBrainsMono Nerd Font Mono,10,-1,5,400,0,0,0,0,0,0,0,0,0,0,1"   # bundled: Debian has no Nerd Font
BAR_FONT="Inter,10,-1,5,500,0,0,0,0,0,0,0,0,0,0,1"
TITLE_FONT="Inter,10,-1,5,500,0,0,0,0,0,0,0,0,0,0,1"

kw() { kwriteconfig6 "$@"; }
log() { printf '\n\033[1;33m==> %s\033[0m\n' "$*"; }
export PATH=~/.local/bin:$PATH
# Portal's home can't exec scripts directly (EACCES despite +x): call the Python helpers through python3
rice-key() { python3 ~/.local/bin/rice-key "$@"; }
rice-tray-fix() { python3 ~/.local/bin/rice-tray-fix "$@"; }
qdbus6 org.kde.plasmashell /PlasmaShell >/dev/null 2>&1 \
  || { echo "can't reach plasmashell on D-Bus: run this from Konsole on the Portal desktop" >&2; exit 1; }

# The large files (icons, cursors, fonts, wallpaper, widgets, the outline effect) are not shipped in the
# Portal APK: the first run downloads them from Portal's GitHub release and checks them against this hash.
DATA_URL=https://github.com/Retrorerr/Portal/releases/download/cloud-dark-rice-1/cloud-dark-rice-data-1.tar.xz
DATA_SHA256=96944c77523e483e0333f48dd4ffe7a22f80546bbfaad1ddc2304b4214ddaf6d
fetch_data() {
  [ -f "$D/icons.tgz" ] && [ -f "$D/wallpaper.jpg" ] && [ -d "$D/prebuilt" ] && return
  log "downloading the rice's icons, fonts and wallpaper (25 MB, first run only)"
  local tmp=$STATE/cloud-dark-rice-data.tar.xz
  curl -fL --retry 3 -o "$tmp" "$DATA_URL" || { echo "download failed: check the internet connection and run it again" >&2; exit 1; }
  echo "$DATA_SHA256  $tmp" | sha256sum -c --quiet - || { rm -f "$tmp"; echo "downloaded file is corrupt: run it again" >&2; exit 1; }
  tar -C "$R" -xJf "$tmp" && rm -f "$tmp"
}

CONFIGS=(kdeglobals kwinrc plasma-org.kde.plasma.desktop-appletsrc plasmashellrc kglobalshortcutsrc
         breezerc konsolerc kcminputrc dolphinrc plasmaparc kscreenlockerrc gtk-3.0/settings.ini gtk-4.0/settings.ini)

shell_wait() { for _ in $(seq 40); do qdbus6 org.kde.plasmashell /PlasmaShell >/dev/null 2>&1 && return; sleep 0.5; done; }

# Portal has no systemd: plasmashell runs under localdesktop-plasmashell-supervisor.sh, which respawns
# it only after a NON-zero exit and stops for good after a clean one (so never kquitapp6 it).
# Pause the supervisor, SIGKILL the shell (it can't save over our edits), run "$@", resume:
# the supervisor sees exit 137 and starts a fresh shell about a second later.
with_shell_down() {
  local sup shells
  shell_wait   # never start while the supervisor is mid-respawn
  sup=$(pgrep -f localdesktop-plasmashell-supervisor | head -1)
  [ -n "$sup" ] || { echo "plasmashell supervisor not found" >&2; return 1; }
  sleep 12   # plasmashell saves applet config on a delayed timer: let the last edits reach disk
  kill -STOP "$sup"
  trap 'kill -CONT '"$sup"' 2>/dev/null' RETURN
  shells=$(pgrep -x -P "$sup" plasmashell || true)
  # shellcheck disable=SC2086
  [ -n "$shells" ] && kill -KILL $shells
  for _ in $(seq 20); do
    pgrep -x -P "$sup" plasmashell | while read -r p; do [ "$(ps -o stat= -p "$p" | cut -c1)" = Z ] || echo "$p"; done | grep -q . || break
    sleep 0.25
  done
  # a killed shell leaves Panel Colorizer's D-Bus helpers (service.py, gdbus monitors) running: clear them
  pkill -f 'luisbocanegra\.panel\.colorizer' || true
  "$@" || true
  kill -CONT "$sup"; trap - RETURN
  sleep 2; shell_wait; sleep 3
}

step_backup() {
  [ -f "$STATE/backup.tgz" ] && { echo "  backup already exists ($STATE/backup.tgz), keeping the original"; return; }
  log "backing up current Plasma config to $STATE/backup.tgz"
  local have=() c
  for c in "${CONFIGS[@]}"; do [ -e ~/.config/$c ] && have+=(".config/$c"); done
  tar -C ~ -czf "$STATE/backup.tgz" "${have[@]}"
}

# KWin keeps the extra desktops live and writes them back over the restored kwinrc
restore_desktops() {
  local want ids
  want=$(tar -xzOf "$STATE/backup.tgz" .config/kwinrc 2>/dev/null |
    awk -F= '/^\[Desktops\]/ { d = 1; next } /^\[/ { d = 0 } d && $1 == "Number" { print $2 }')
  mapfile -t ids < <(gdbus call --session --dest org.kde.KWin --object-path /VirtualDesktopManager \
    --method org.freedesktop.DBus.Properties.Get org.kde.KWin.VirtualDesktopManager desktops |
    grep -oE "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
  while [ "${#ids[@]}" -gt "${want:-1}" ]; do
    vdm org.kde.KWin.VirtualDesktopManager.removeDesktop "${ids[-1]}"
    unset 'ids[-1]'
  done
}

step_restore() {
  [ -f "$STATE/backup.tgz" ] || { echo "no backup at $STATE/backup.tgz" >&2; return 1; }
  log "restoring config from backup (shell restarts)"
  plasma-apply-colorscheme BreezeDark >/dev/null || true
  /usr/lib/aarch64-linux-gnu/libexec/plasma-changeicons breeze-dark || true
  plasma-apply-cursortheme breeze_cursors || true
  # the outline effect and the tags worker divert are the two system-wide changes
  kw --file kwinrc --group Plugins --key kwin4_effect_shapecornersEnabled false
  qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.unloadEffect kwin4_effect_shapecorners >/dev/null 2>&1 || true
  ! dpkg-divert --list "$tags_so" | grep -q . || sudo -n dpkg-divert --local --rename --remove "$tags_so" || true
  restore_desktops
  with_shell_down tar -C ~ -xzf "$STATE/backup.tgz"
  qdbus6 org.kde.KWin /KWin reconfigure || true
  echo "  done; log out (Portal closes) and open Portal again for apps to drop the fonts"
}

step_packages() {
  log "packages (Debian): fonts, Panel Colorizer's D-Bus helper, fastfetch"
  # fonts-*: UI + terminal fonts; python3-dbus/python3-gi: Panel Colorizer's service.py;
  # fastfetch: the terminal greeter config; rofi + imagemagick + jq: the power menu and its blurred
  # backdrop; plasma-systemmonitor: Ctrl+Esc; alacritty: the laptop's second terminal (config only).
  # qdbus6 / gdbus (widgets, rice-key) ship with Portal.
  local need=() p
  for p in fonts-inter fonts-jetbrains-mono python3-dbus python3-gi fastfetch rofi imagemagick jq plasma-systemmonitor alacritty; do dpkg -s "$p" >/dev/null 2>&1 || need+=("$p"); done
  # JetBrainsMono Nerd Font (OFL, bundled from the laptop): Konsole + fastfetch's glyph keys
  if ! fc-list | grep -q "JetBrainsMono Nerd Font Propo"; then
    install -Dm644 -t ~/.local/share/fonts/JetBrainsMonoNerd "$D"/fonts/*.ttf "$D"/fonts/OFL.txt
    # some older homes can't write the user font cache; the fonts load without it
    fc-cache -f ~/.local/share/fonts >/dev/null 2>&1 || echo "  font cache not written (the fonts still work)"
  fi
  [ ${#need[@]} -eq 0 ] && { echo "  apt packages already installed"; return; }
  sudo -n apt-get update -qq && sudo -n DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "${need[@]}"
}

step_theme() {
  log "colour scheme, plasma style, icons, cursor, fonts, GTK"
  install -Dm644 -t ~/.local/share/color-schemes "$F"/color-schemes/*.colors
  mkdir -p ~/.local/share/icons
  [ -d ~/.local/share/icons/Gruvbox-Plus-Dark ] || tar -C ~/.local/share/icons -xzf "$D/icons.tgz"
  [ -d ~/.local/share/icons/capitaine-gruvbox ] || tar -C ~/.local/share/icons -xzf "$D/cursors.tgz"
  # Debian's libXcursor (and so the cursor KCM) only searches ~/.icons, not ~/.local/share/icons
  mkdir -p ~/.icons && ln -sfn ~/.local/share/icons/capitaine-gruvbox ~/.icons/capitaine-gruvbox

  kw --file kdeglobals --group General --key AccentColor --delete
  kw --file kdeglobals --group General --key LastUsedCustomAccentColor --delete
  kw --file kdeglobals --group General --key accentColorFromWallpaper false
  plasma-apply-colorscheme BreezeDark >/dev/null   # force a real re-apply
  plasma-apply-colorscheme CloudDark
  plasma-apply-desktoptheme default
  # Launcher logo: Gruvbox has no start-here-kde-symbolic, so the launcher gets Gruvbox's teal K tile.
  # Plasma 6.7 (laptop) draws that one-colour; 6.3 draws the tile. Ship the K + gear on its own as the
  # symbolic icon, coloured by the scheme like every other symbolic icon
  local sz
  for sz in 16 22 24; do
    install -Dm644 "$D/start-here-kde-symbolic.svg" ~/.local/share/icons/Gruvbox-Plus-Dark/panel/$sz/start-here-kde-symbolic.svg
  done
  rm -f ~/.local/share/icons/Gruvbox-Plus-Dark/icon-theme.cache ~/.cache/icon-cache.kcache
  /usr/lib/aarch64-linux-gnu/libexec/plasma-changeicons Gruvbox-Plus-Dark
  plasma-apply-cursortheme capitaine-gruvbox
  kw --file kcminputrc --group Mouse --key cursorSize 24

  kw --file kdeglobals --group General --key font "$UI_FONT"
  kw --file kdeglobals --group General --key menuFont "$UI_FONT"
  kw --file kdeglobals --group General --key toolBarFont "$UI_FONT"
  kw --file kdeglobals --group General --key fixed "$MONO_FONT"
  kw --file kdeglobals --group WM --key activeFont "$TITLE_FONT"

  kw --file gtk-3.0/settings.ini --group Settings --key gtk-theme-name Breeze
  kw --file gtk-4.0/settings.ini --group Settings --key gtk-theme-name Breeze
  dbus-send --session --dest=org.kde.kded6 /kded org.kde.kded6.loadModule string:gtkconfig >/dev/null 2>&1 || true
}

vdm() { qdbus6 org.kde.KWin /VirtualDesktopManager "$@"; }

step_windows() {
  log "windows: Breeze rounded corners (until the outline step swaps in the effect), numbered desktops"
  kw --file breezerc --group Common --key RoundedCorners true
  kw --file breezerc --group Common --key OutlineIntensity --delete
  while [ "$(vdm org.kde.KWin.VirtualDesktopManager.count)" -lt "$DESKTOPS" ]; do
    n=$(vdm org.kde.KWin.VirtualDesktopManager.count)
    vdm org.kde.KWin.VirtualDesktopManager.createDesktop "$n" "$((n + 1))"
  done
  kw --file kwinrc --group Desktops --key Rows 1
  qdbus6 org.kde.KWin /KWin reconfigure
}

# Active-window outline: KDE-Rounded-Corners v0.10.0 (the laptop's kwin-effect-rounded-corners), built for
# Portal's own KWin 6.3.6 (Anland build, aarch64). A binary KWin plugin must match KWin's version exactly,
# so if Portal ever updates KWin this rebuilds it from the bundled source. It's switched on for good only
# after KWin has run with it for a few seconds, so a bad build can't leave the desktop black at boot.
corners_so=/usr/lib/aarch64-linux-gnu/qt6/plugins/kwin/effects/plugins/kwin4_effect_shapecorners.so
# the effect draws nothing without its shaders: merge them from the bundled source the way its CMake does
install_outline_shaders() {
  local t s f g
  t=$(mktemp -d)
  tar -C "$t" -xzf "$D/KDE-Rounded-Corners.tgz" src/shaders
  s=$t/src/shaders
  for f in shapecorners shapecorners_core; do
    sed -e "/#include \"shapecorners.glsl\"/ { r $s/shapecorners.glsl" -e d -e } \
      "$s/shapecorners_qt6${f#shapecorners}.frag" > "$t/$f.frag"
    for g in shapecorners_shadows variables squircles; do
      sed -i -e "/#include \"$g.glsl\"/ { r $s/$g.glsl" -e d -e } "$t/$f.frag"
    done
  done
  sudo -n install -dm755 /usr/share/kwin/shaders
  sudo -n install -m644 -t /usr/share/kwin/shaders "$t/shapecorners.frag" "$t/shapecorners_core.frag"
  rm -rf "$t"
}
step_outline() {
  log "active-window outline (KDE-Rounded-Corners effect, as on the laptop)"
  local kv iid
  kv=$(/usr/bin/kwin_wayland --version 2>/dev/null | awk '{print $2}')
  iid=$(grep -ao 'EffectPluginFactory6[.0-9]*' "$D/prebuilt/kwin4_effect_shapecorners.so" | sed 's/EffectPluginFactory//')
  if [ "$iid" = "$kv" ]; then
    cmp -s "$D/prebuilt/kwin4_effect_shapecorners.so" "$corners_so" || {
      sudo -n install -Dm644 "$D/prebuilt/kwin4_effect_shapecorners.so" "$corners_so"
      sudo -n install -Dm644 "$D/prebuilt/kwin_shapecorners_config.so" "${corners_so%/plugins/*}/configs/kwin_shapecorners_config.so"; }
    install_outline_shaders
  else
    echo "  bundled build is for KWin $iid, Portal has $kv: building it (several minutes)"
    sudo -n bash "$D/build-outline.sh" "$D/KDE-Rounded-Corners.tgz" || { echo "  build failed: keeping Breeze's outline" >&2; return 0; }
  fi
  local RC=(--file kwinrc --group Round-Corners) k v
  while read -r k v; do kw "${RC[@]}" --key "$k" "$v"; done <<EOF
Size 5
InactiveCornerRadius 5
DisableRoundTile false
DisableRoundMaximize true
DisableRoundFullScreen true
Exclusions rofi
DisableOutlineTile false
DisableOutlineMaximize true
DisableOutlineFullScreen true
ActiveOutlineUsePalette false
OutlineColor 200,192,186
ActiveOutlineAlpha 255
OutlineThickness 2
ActiveOutlineUseCustom true
InactiveOutlineUseCustom true
UseSquircleShape false
InactiveOutlineUsePalette false
InactiveOutlineColor 74,71,69
InactiveOutlineAlpha 255
InactiveOutlineThickness 1
SecondOutlineThickness 0
InactiveSecondOutlineThickness 0
OuterOutlineThickness 0
InactiveOuterOutlineThickness 0
EOF
  qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.unloadEffect kwin4_effect_shapecorners >/dev/null 2>&1 || true
  qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.loadEffect kwin4_effect_shapecorners >/dev/null 2>&1 || true
  sleep 5
  if [ "$(qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.isEffectLoaded kwin4_effect_shapecorners 2>/dev/null)" = true ]; then
    kw --file kwinrc --group Plugins --key kwin4_effect_shapecornersEnabled true
    # one outline only: the effect's, over Breeze's own (as on the laptop)
    kw --file breezerc --group Common --key RoundedCorners false
    kw --file breezerc --group Common --key OutlineEnabled false
    kw --file breezerc --group Common --key OutlineIntensity OutlineOff
    qdbus6 org.kde.KWin /KWin reconfigure
  else
    echo "  KWin didn't load the effect: keeping Breeze's outline" >&2
  fi
}

step_bar() {
  log "top bar: launcher, desktop numbers, clock, now-playing, tray, power; left icon dock"
  local p w out ov
  for p in "$F"/plasmoids/*/ "$D"/colorizer.plasmoid "$D"/plasmusic.plasmoid; do
    kpackagetool6 -t Plasma/Applet -u "$p" >/dev/null 2>&1 || kpackagetool6 -t Plasma/Applet -i "$p"
  done
  install -Dm755 "$F/bin/rice-key" ~/.local/bin/rice-key
  install -Dm755 "$D/rice-tray-fix" ~/.local/bin/rice-tray-fix
  # the bar's power button copies its launcher as the widget is made: it has to exist by then
  install_power_launcher
  # a fresh shell so newly installed / upgraded widget QML is used
  with_shell_down true
  for w in $({ rice-key --who Alt+F1; rice-key --who Meta; } | grep -oE 'activate widget [0-9]+' | awk '{print $3}' || true); do
    rice-key plasmashell "activate widget $w" none
  done
  sed -e "s|@BAR@|$BAR|" -e "s|@DOCK@|$DOCK|" -e "s|@FONT@|$BAR_FONT|" -e "s|@HOME@|$HOME|" \
    "$D/panel-layout.js" > "$STATE/panel-layout.js"
  out=$(qdbus6 org.kde.plasmashell /PlasmaShell org.kde.PlasmaShell.evaluateScript "$(cat "$STATE/panel-layout.js")")
  echo "$out" | grep 'rice-panel' || { echo "$out" >&2; return 1; }
  ov=$(echo "$out" | sed -n 's/.*<<OV\(.*\)OV>>.*/\1/p')
  sleep 4   # let Panel Colorizer load before writing its overrides (it resets them on first load)
  { cat ~/.local/share/plasma/plasmoids/luisbocanegra.panel.colorizer/contents/ui/code/globals.js
    sed "s|@OVERRIDES@|$ov|" "$F/panel-post.js"; } > "$STATE/panel-post.js"
  qdbus6 org.kde.plasmashell /PlasmaShell org.kde.PlasmaShell.evaluateScript "$(cat "$STATE/panel-post.js")" | grep -o 'rice-post.*'
  # Plasma 6.3 reads the tray's item lists from its inner containment, and the new tray already made
  # its default widgets (a second volume icon among them): fix both in the saved layout, shell down
  with_shell_down rice-tray-fix
  # Kickoff: bare Meta tap only (panel-layout.js gave it Alt+F1 so it can be found here)
  for _ in $(seq 30); do
    w=$(rice-key --who Alt+F1 | grep -oE 'activate widget [0-9]+' | awk '{print $3}' | tail -1 || true)
    [ -n "$w" ] && break; sleep 1
  done
  # on Plasma 6.3 the generic "activate application launcher" action already holds Meta and opens Kickoff
  if [ -n "$w" ]; then rice-key plasmashell "activate widget $w" Meta; fi
}

install_power_launcher() {
  install -Dm755 -t ~/.local/bin "$D"/rice-powermenu "$F"/bin/rice-backdrop
  install -Dm644 -t ~/.local/share/applications "$D"/rice-powermenu.desktop "$F"/applications/rice-rofi-close.desktop
}

step_power() {
  log "power menu: full-screen rofi over the blurred wallpaper (bar power button, Meta+Esc)"
  local scale W H s
  install -Dm644 -t ~/.config/rofi "$F"/rofi/colors.rasi
  install_power_launcher
  # rofi is an X11 client here, drawn at the screen's native pixels: scale the laptop's sizes
  scale=$(kscreen-doctor -j | jq -r '[.outputs[] | select(.enabled)][0].scale')
  read -r W H < <(xdpyinfo | awk '/dimensions:/{split($2, d, "x"); print d[1], d[2]}')
  s() { awk -v a="$1" -v k="$scale" 'BEGIN { printf "%d", a * k + 0.5 }'; }
  sed -e "s/@S@/$scale/; s/@DPI@/$(s 96)/" -e "s/@MARGIN@/$(( (W - 3 * $(s 260) - 2 * $(s 22)) / 2 ))/" \
      $(for n in 2 12 22 30 34 50 260; do printf -- "-e s/@%s@/%s/g " "$n" "$(s $n)"; done) \
      "$D/powermenu.rasi.in" > ~/.config/rofi/powermenu.rasi
  bash ~/.local/bin/rice-backdrop --force
  # rofi stays open when you tap another window: this KWin script closes it
  kpackagetool6 --type=KWin/Script -u "$F/kwin/rice-rofi-autoclose" >/dev/null 2>&1 \
    || kpackagetool6 --type=KWin/Script -i "$F/kwin/rice-rofi-autoclose"
  kw --file kwinrc --group Plugins --key rice-rofi-autocloseEnabled true
  gdbus call --session --dest org.kde.kglobalaccel --object-path /kglobalaccel \
    --method org.kde.KGlobalAccel.doRegister "['rice-rofi-close.desktop','_launch','Rice: close rofi','Rice: close rofi']" >/dev/null
  qdbus6 org.kde.KWin /KWin reconfigure
  rm -f ~/.local/share/applications/rice-leave.desktop
}

step_wallpaper() {
  log "wallpaper: the one on your laptop (desktop + lock screen)"
  install -Dm644 "$D/wallpaper.jpg" ~/.local/share/wallpapers/rice/wallpaper.jpg
  local img=$HOME/.local/share/wallpapers/rice/wallpaper.jpg
  plasma-apply-wallpaperimage "$img"
  kw --file kscreenlockerrc --group Greeter --group Wallpaper --group org.kde.image --group General --key Image "file://$img"
  kw --file kscreenlockerrc --group Greeter --group Wallpaper --group org.kde.image --group General --key PreviewImage "file://$img"
}

tags_so=/usr/lib/aarch64-linux-gnu/qt6/plugins/kf6/kio/tags.so
tags_off=/usr/lib/aarch64-linux-gnu/qt6/tags.so.rice-disabled
step_apps() {
  log "app settings from the laptop: Dolphin, Konsole chrome, volume limit, night colour"
  kw --file dolphinrc --group MainWindow --key MenuBar Disabled
  kw --file dolphinrc --group PlacesPanel --key IconSize 48
  kw --file dolphinrc --group "KFileDialog Settings" --key "Places Icons Auto-resize" false
  kw --file dolphinrc --group "KFileDialog Settings" --key "Places Icons Static Size" 48
  kw --file konsolerc --key MenuBar Disabled
  kw --file konsolerc --group MainWindow --key ToolBarsMovable Enabled
  kw --file konsolerc --group MainWindow --group "Toolbar mainToolBar" --key IconSize 16
  kw --file konsolerc --group "Toolbar mainToolBar" --key IconSize 16
  kw --file konsolerc --group "Notification Messages" --key CloseAllTabs true
  kw --file plasmaparc --group General --key RaiseMaximumVolume true
  kw --file kwinrc --group NightColor --key Active true
  qdbus6 org.kde.KWin /KWin reconfigure
  # Portal has no Baloo index, so the file-tags KIO worker fails on every shell start and plasmashell
  # pops up "Could not enter folder tags:/". Divert it out of the plugin path (undo: restore step)
  dpkg-divert --list "$tags_so" | grep -q . \
    || sudo -n dpkg-divert --local --rename --divert "$tags_off" --add "$tags_so"
}

step_terminal() {
  log "Konsole colours + font"
  install -Dm644 "$F/konsole/GruvboxMaterial.colorscheme" ~/.local/share/konsole/GruvboxMaterial.colorscheme
  install -Dm644 "$F/konsole/Rice.profile" ~/.local/share/konsole/Rice.profile
  kw --file konsolerc --group "Desktop Entry" --key DefaultProfile Rice.profile
  install -Dm644 "$F/fastfetch/config.jsonc" ~/.config/fastfetch/config.jsonc

  install -Dm644 "$F/alacritty/gruvbox-material.toml" ~/.config/alacritty/gruvbox-material.toml
  local a=~/.config/alacritty/alacritty.toml
  [ -f "$a" ] || printf '[general]\nimport = ["~/.config/alacritty/gruvbox-material.toml"]\n' > "$a"
  grep -q gruvbox-material.toml "$a" || echo "  $a has its own settings: add gruvbox-material.toml to its import list"
}

step_keys() {
  log "shortcuts (live through kglobalaccel)"
  install -Dm755 "$F/bin/rice-key" ~/.local/bin/rice-key
  local K=kwin d s
  for d in Left Right; do rice-key $K "Window Quick Tile $d" Meta+$d; done
  rice-key $K "Window Quick Tile Top" Meta+Up; rice-key $K "Window Quick Tile Bottom" Meta+Down
  rice-key $K "Window to Previous Screen" Meta+Shift+Left; rice-key $K "Window to Next Screen" Meta+Shift+Right
  rice-key $K "Switch One Desktop to the Left" Meta+Ctrl+Left; rice-key $K "Switch One Desktop to the Right" Meta+Ctrl+Right
  rice-key $K "Switch One Desktop Up" Meta+Ctrl+Up; rice-key $K "Switch One Desktop Down" Meta+Ctrl+Down
  rice-key plasmashell "manage activities" none          # Meta+Q becomes "close window"
  for d in 1 2 3 4 5 6 7 8 9; do rice-key plasmashell "activate task manager entry $d" none; done
  rice-key $K "Window Close" Meta+Q Alt+F4
  # desktops: Meta+N switch, Meta+Shift+N send (second key covers a GB layout's Shift+1..7 symbols)
  s=('!' '"' '£' '$' '%' '^' '&')
  for d in $(seq 1 "$DESKTOPS"); do
    rice-key $K "Switch to Desktop $d" "Meta+$d" "Ctrl+F$d"
    rice-key $K "Window to Desktop $d" "Meta+Shift+$d" "Meta+${s[$((d-1))]}"
  done
  rice-key org.kde.plasma-systemmonitor.desktop _launch Ctrl+Escape
  gdbus call --session --dest org.kde.kglobalaccel --object-path /kglobalaccel \
    --method org.kde.KGlobalAccel.doRegister "['rice-powermenu.desktop','_launch','Rice: power menu','Rice: power menu']" >/dev/null
  rice-key rice-powermenu.desktop _launch Meta+Escape
  kw --file kwinrc --group ModifierOnlyShortcuts --key Meta --delete
  qdbus6 org.kde.KWin /KWin reconfigure
}

main() {
  local steps=("$@")
  [ ${#steps[@]} -eq 0 ] && steps=(backup packages theme windows outline bar power wallpaper apps terminal keys)
  [ "${steps[0]}" != restore ] && fetch_data
  [ "${steps[0]}" != restore ] && [ "${steps[0]}" != backup ] && step_backup
  for s in "${steps[@]}"; do "step_$s"; done
  [ "${steps[0]}" = restore ] && return
  log "done — log out (Portal closes) and open Portal again so every app picks up the theme"
}
main "$@"
