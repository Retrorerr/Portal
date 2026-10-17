#!/bin/sh
# portal-system-services start|stop|flatpak-gl
#
# Debian's system services for the Portal session: the D-Bus system bus,
# with polkit, PackageKit, UDisks and the rest started on demand through
# it, as on a native Plasma install (Discover, System Settings' admin
# pages and pkexec need them). There is no systemd, so Portal starts the
# bus itself, as root, inside the desktop session's PRoot: D-Bus and
# polkit identify clients by SO_PEERCRED, which PRoot fakes only between
# processes it traces, so the bus must share the session's instance.
# "stop" ends the bus and whatever it started, so the instance exits with
# the session.

pidfile=/run/dbus/pid
socket=/run/dbus/system_bus_socket
log=/var/log/portal-system-services.log

stop() {
	[ -s "$pidfile" ] && kill "$(cat "$pidfile")" 2>/dev/null
	for daemon in packagekitd polkitd udisksd accounts-daemon fwupd \
		ModemManager upowerd; do
		pkill -x "$daemon" 2>/dev/null
	done
	# Its name is longer than the 15 characters pkill -x compares.
	pkill -f /usr/libexec/flatpak-system-helper 2>/dev/null
	rm -f "$pidfile" "$socket"
}

start() {
	command -v dbus-daemon >/dev/null || return 0
	# Users and groups the services run as (messagebus, polkitd, ...);
	# the rootfs was unpacked without running their maintainer scripts.
	command -v systemd-sysusers >/dev/null && systemd-sysusers >/dev/null 2>&1
	for user in messagebus polkitd; do
		getent passwd "$user" >/dev/null ||
			useradd --system --user-group --home-dir /nonexistent \
				--shell /usr/sbin/nologin "$user" 2>/dev/null
	done
	# useradd leaves them root-only, and the bus then refuses clients.
	chmod 0644 /etc/passwd /etc/group
	helper=/usr/lib/dbus-1.0/dbus-daemon-launch-helper
	if [ -e "$helper" ] && getent group messagebus >/dev/null; then
		chown root:messagebus "$helper" && chmod 4754 "$helper"
	fi
	agent=/usr/lib/polkit-1/polkit-agent-helper-1
	[ -e "$agent" ] && chmod 4755 "$agent"
	[ -e /usr/bin/pkexec ] && chmod 4755 /usr/bin/pkexec
	[ -s /etc/machine-id ] || dbus-uuidgen --ensure=/etc/machine-id
	[ -e /var/lib/dbus/machine-id ] || {
		mkdir -p /var/lib/dbus
		ln -sf /etc/machine-id /var/lib/dbus/machine-id
	}
	# /run outlives sessions here; a previous bus left its pid and socket.
	stop
	mkdir -p /run/dbus
	dbus-daemon --system --nofork --nopidfile </dev/null >/dev/null 2>&1 &
	echo $! >"$pidfile"
	i=0
	while [ ! -S "$socket" ] && [ $i -lt 50 ]; do
		sleep 0.1
		i=$((i + 1))
	done
	probe_bwrap
	refresh_ld_cache
	divert_fwupd_backend
	flatpak_sandbox_env
	# app_store first: on older images it is what installs Flatpak. Its
	# own subshell, since it exits on failure.
	{
		[ -e "$app_store_marker" ] || (app_store)
		flatpak_gl_portal
	} >/dev/null 2>&1 </dev/null &
}

# The session's Mesa variables name drivers only Portal's own Mesa has;
# the runtime Mesa in a Flatpak sandbox then loads none and the app gets
# no GL at all. Unset there, the runtime's llvmpipe works everywhere, and
# Portal's GL extension (below) finds the kgsl driver through
# FD_FORCE_KGSL, which stays.
flatpak_sandbox_env() {
	command -v flatpak >/dev/null || return 0
	flatpak override --system --unset-env=MESA_LOADER_DRIVER_OVERRIDE \
		--unset-env=GALLIUM_DRIVER >/dev/null 2>&1
}

# Portal's KGSL Mesa for Flatpak apps: org.freedesktop.Platform.GL.portal,
# built by .github/workflows/mesa-kgsl-flatpak-arm64.yml, one bundle per
# freedesktop runtime branch. FLATPAK_GL_DRIVERS (session env) lists it
# first. Without it a Flatpak app's runtime Mesa is llvmpipe, which draws
# every frame on the CPU, pegs all cores and stutters the whole desktop.
# Every supported branch is installed up front, whether or not an app
# needs it yet, so an app installed later has GPU GL on its first start:
# first setup runs "flatpak-gl" before the desktop starts, and each session
# start repeats the (otherwise instant) check in the background for setups
# that were offline, images without Flatpak until app_store, and pin bumps.
flatpak_gl_tag=flatpak-gl-portal-26.3.0-20261016
flatpak_gl_url=https://github.com/Retrorerr/Portal/releases/download/$flatpak_gl_tag
flatpak_gl_branches="24.08 25.08"
flatpak_gl_sha256() {
	case "$1" in
	24.08) echo 78cd0f0649146c282ab655370b958a6a5287bb9166c1c57a8222361349985550 ;;
	25.08) echo 5f7006d8b64142c7d8fd79f83392315e4a0145b637759d588b043d894b8f7a95 ;;
	esac
}
flatpak_gl_portal() {
	local log=/var/log/portal-flatpak-gl.log
	[ -e /dev/kgsl-3d0 ] && command -v flatpak >/dev/null || return 0
	state=/var/lib/localdesktop/flatpak-gl-portal
	failed=0
	for branch in $flatpak_gl_branches; do
		sha=$(flatpak_gl_sha256 "$branch")
		# "flatpak uninstall --unused" removes it too, state file or not.
		[ "$(cat "$state/$branch" 2>/dev/null)" = "$sha" ] &&
			[ -e "/var/lib/flatpak/runtime/org.freedesktop.Platform.GL.portal/aarch64/$branch/active" ] &&
			continue
		{
			bundle=$(mktemp) || continue
			if curl -fsSL --retry 3 -o "$bundle" 				"$flatpak_gl_url/org.freedesktop.Platform.GL.portal-$branch-aarch64.flatpak" &&
				echo "$sha  $bundle" | sha256sum -c - &&
				flatpak install --system -y --noninteractive --reinstall 					--bundle "$bundle"; then
				mkdir -p "$state" && echo "$sha" >"$state/$branch"
			else
				failed=1
			fi
			rm -f "$bundle"
		} >>"$log" 2>&1
	done
	return $failed
}

# Portal stages libraries for the library path (its GLEW, listed by
# /etc/ld.so.conf.d/00-portal-glew.conf) from outside the guest, where
# ldconfig can't run; package installs re-run it afterwards anyway.
refresh_ld_cache() {
	for path in /etc/ld.so.conf.d/00-portal-*.conf /usr/local/lib/portal-glew/*.so.*; do
		[ "$path" -nt /etc/ld.so.cache ] || continue
		ldconfig
		return
	done
}

# Discover's firmware backend: fwupd finds no devices here (an app has no
# firmware to flash), and Discover 6.3 then never ends that backend's
# fetch, leaving Updates on "Fetching..." for good. A diversion keeps the
# plugin out across upgrades; fwupd itself stays for anything else.
divert_fwupd_backend() {
	for plugin in /usr/lib/*/qt6/plugins/discover/fwupd-backend.so; do
		[ -e "$plugin" ] || continue
		[ "$(dpkg-divert --truename "$plugin")" = "$plugin" ] || continue
		dpkg-divert --local --rename --divert "${plugin%.so}.portal-disabled" \
			--add "$plugin" >/dev/null 2>&1
	done
}

# Whether the real bubblewrap works under this PRoot; the /usr/local/bin
# shim runs it when it does and only strips the sandbox when it does not.
probe_bwrap() {
	rm -f /run/portal-bwrap-ok /run/portal-bwrap-failed
	[ -x /usr/bin/bwrap ] || return 0
	# As the desktop user, who runs the sandboxes, and bounded: the
	# session waits for this.
	user=$(getent passwd 1000 | cut -d: -f1)
	if output=$(timeout 10 runuser -u "${user:-root}" -- /usr/bin/bwrap \
		--unshare-all --share-net --die-with-parent \
		--ro-bind / / --proc /proc --dev /dev --tmpfs /tmp \
		/bin/true </dev/null 2>&1); then
		: >/run/portal-bwrap-ok
	else
		: >/run/portal-bwrap-failed
		echo "$(date '+%F %T') bwrap probe failed ($?): $output" >>"$log"
	fi
}

# Discover with its Debian (PackageKit) and Flatpak backends, as on a
# native Debian Plasma install. Images built before they were part of
# the rootfs get them once, in the background, when the network allows.
app_store_marker=/var/lib/localdesktop/app-store-v1
app_store() {
	local log=/var/log/portal-app-store.log
	missing=
	for package in plasma-discover plasma-discover-backend-flatpak flatpak \
		packagekit packagekit-tools pkexec appstream; do
		dpkg-query -W -f='${Status}' "$package" 2>/dev/null |
			grep -q 'install ok installed' || missing="$missing $package"
	done
	{
		if [ -n "$missing" ]; then
			export DEBIAN_FRONTEND=noninteractive
			dpkg --configure -a
			apt-get -o DPkg::Lock::Timeout=900 -q update &&
				apt-get -o DPkg::Lock::Timeout=900 install -y \
					--no-install-recommends --no-remove $missing
			status=$?
			chmod 0644 /etc/passwd /etc/group
			[ $status -eq 0 ] || exit 1
		fi
		if command -v flatpak >/dev/null; then
			flatpak remote-add --system --if-not-exists flathub \
				https://dl.flathub.org/repo/flathub.flatpakrepo || exit 1
		fi
		mkdir -p "${app_store_marker%/*}" && : >"$app_store_marker"
	} >>"$log" 2>&1
}

case "$1" in
start) start ;;
stop) stop ;;
flatpak-gl) flatpak_gl_portal || exit 1 ;;
*) echo "usage: $0 start|stop|flatpak-gl" >&2; exit 2 ;;
esac
exit 0
