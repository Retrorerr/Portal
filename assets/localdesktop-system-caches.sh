#!/bin/sh
# Debian package extraction skips maintainer triggers. Run this as guest root
# during provisioning/one-time login migration, never inside Plasma.
set -eu
marker=/var/lib/localdesktop/system-caches-v2
[ -f "$marker" ] && exit 0
if [ ! -e /usr/lib/locale/locale-archive ]; then
    localedef -i en_GB -f UTF-8 en_GB.UTF-8
    localedef -i en_US -f UTF-8 en_US.UTF-8
    localedef -i C -f UTF-8 C.UTF-8
fi
# Alternatives the image's postinsts would have registered. Each is added only
# when its target is installed and one of its links is still absent, so links
# apt or the user already manage are left alone. Spectacle (via OpenCV) needs
# the BLAS/LAPACK library links to start at all.
alt() {
    [ -e "$3" ] || return 0
    missing=
    [ -e "$1" ] || [ -L "$1" ] || missing=1
    previous=
    for word in "$@"; do
        if [ "$previous" = --slave ] && ! [ -e "$word" ] && ! [ -L "$word" ]; then
            missing=1
        fi
        previous=$word
    done
    [ -z "$missing" ] || update-alternatives --quiet --install "$@"
}
alt /usr/bin/awk awk /usr/bin/gawk 10 \
    --slave /usr/bin/nawk nawk /usr/bin/gawk
alt /usr/lib/aarch64-linux-gnu/libblas.so.3 libblas.so.3-aarch64-linux-gnu \
    /usr/lib/aarch64-linux-gnu/blas/libblas.so.3 10
alt /usr/lib/aarch64-linux-gnu/liblapack.so.3 liblapack.so.3-aarch64-linux-gnu \
    /usr/lib/aarch64-linux-gnu/lapack/liblapack.so.3 10
alt /usr/bin/which which /usr/bin/which.debianutils 0
alt /usr/bin/lzma lzma /usr/bin/xz 20 \
    --slave /usr/bin/unlzma unlzma /usr/bin/unxz \
    --slave /usr/bin/lzcat lzcat /usr/bin/xzcat
alt /usr/bin/x-terminal-emulator x-terminal-emulator /usr/bin/konsole 40
alt /usr/bin/x-session-manager x-session-manager /usr/bin/startplasma-x11 40
alt /usr/bin/x-window-manager x-window-manager /usr/bin/kwin_x11 50
alt /usr/bin/ssh-askpass ssh-askpass /usr/bin/ksshaskpass 35
alt /usr/bin/pinentry pinentry /usr/bin/pinentry-curses 50
alt /usr/sbin/rmt rmt /usr/sbin/rmt-tar 50
alt /lib/cpp cpp /usr/bin/cpp 10
alt /usr/share/icons/default/index.theme x-cursor-theme \
    /usr/share/icons/Adwaita/cursor.theme 90
glib-compile-schemas /usr/share/glib-2.0/schemas
if [ -x /usr/lib/aarch64-linux-gnu/libgtk-3-0/gtk-query-immodules-3.0 ]; then
    /usr/lib/aarch64-linux-gnu/libgtk-3-0/gtk-query-immodules-3.0 --update-cache
fi
update-mime-database /usr/share/mime
update-desktop-database /usr/share/applications
# GTK apps otherwise rescan every icon directory on each start.
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    for theme in /usr/share/icons/*/; do
        [ -f "$theme/index.theme" ] && gtk-update-icon-cache -q -t -f "$theme" || true
    done
fi
printf 'complete\n' > "$marker.tmp"
mv -f "$marker.tmp" "$marker"
