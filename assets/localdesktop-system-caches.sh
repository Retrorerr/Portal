#!/bin/sh
# Debian package extraction skips maintainer triggers. Run this as guest root
# during provisioning/one-time login migration, never inside Plasma.
set -eu
marker=/var/lib/localdesktop/system-caches-v1
[ -f "$marker" ] && exit 0
if [ ! -e /usr/lib/locale/locale-archive ]; then
    localedef -i en_GB -f UTF-8 en_GB.UTF-8
    localedef -i en_US -f UTF-8 en_US.UTF-8
    localedef -i C -f UTF-8 C.UTF-8
fi
if ! command -v awk >/dev/null 2>&1; then
    update-alternatives --install /usr/bin/awk awk /usr/bin/gawk 10
fi
glib-compile-schemas /usr/share/glib-2.0/schemas
if [ -x /usr/lib/aarch64-linux-gnu/libgtk-3-0/gtk-query-immodules-3.0 ]; then
    /usr/lib/aarch64-linux-gnu/libgtk-3-0/gtk-query-immodules-3.0 --update-cache
fi
update-mime-database /usr/share/mime
update-desktop-database /usr/share/applications
printf 'complete\n' > "$marker.tmp"
mv -f "$marker.tmp" "$marker"
