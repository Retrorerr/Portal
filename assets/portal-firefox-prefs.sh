#!/bin/sh
# Managed by Portal. Re-applies Portal's Firefox preferences (the autoconfig
# that forces GPU compositing) to every Mozilla browser installed under
# /usr/lib.
#
# dpkg runs it after every install, upgrade, reinstall and removal
# (/etc/dpkg/dpkg.cfg.d/portal-firefox-prefs), so Mozilla's own `firefox`
# package, firefox-beta/-devedition/-nightly, or an upgrade of any of them is
# covered without restarting Portal. It is also safe to run by hand.
#
# An optional argument is a root directory to operate under (for tests).
# The script never fails: a failing dpkg hook makes every apt run report an
# error.

root="${1%/}"
share="$root/usr/local/share/portal/firefox"
[ -r "$share/autoconfig.js" ] && [ -r "$share/localdesktop.cfg" ] || exit 0

for dir in "$root"/usr/lib/firefox "$root"/usr/lib/firefox-*; do
    [ -d "$dir" ] || continue
    [ -e "$dir/application.ini" ] || [ -e "$dir/libxul.so" ] || [ -e "$dir/firefox-bin" ] || continue
    mkdir -p "$dir/defaults/pref" 2>/dev/null || continue
    cmp -s "$share/autoconfig.js" "$dir/defaults/pref/autoconfig.js" 2>/dev/null ||
        cp "$share/autoconfig.js" "$dir/defaults/pref/autoconfig.js" 2>/dev/null
    cmp -s "$share/localdesktop.cfg" "$dir/localdesktop.cfg" 2>/dev/null ||
        cp "$share/localdesktop.cfg" "$dir/localdesktop.cfg" 2>/dev/null
done
exit 0
