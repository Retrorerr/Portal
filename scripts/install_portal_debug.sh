#!/usr/bin/env bash
# Linux port of install_portal_debug.ps1.
#
# Usage: scripts/install_portal_debug.sh <adb serial>
#
# Only ever updates app.polarbear (Portal Debug) in place with adb install -r.
# The serial is mandatory so this can never silently choose another device.
set -euo pipefail

device_id="${1:?usage: install_portal_debug.sh <adb serial>}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
debug_apk="$repo_root/target/Portal-Debug.apk"
debug_package=app.polarbear
export ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"

die() { echo "error: $*" >&2; exit 1; }

resolve_aapt() {
    local dir="$ANDROID_HOME/build-tools" version
    if [ -d "$dir" ]; then
        version="$(ls "$dir" | sort -V | tail -1)"
        if [ -n "$version" ] && [ -x "$dir/$version/aapt" ]; then
            echo "$dir/$version/aapt"
            return
        fi
    fi
    command -v aapt || die 'Android build-tools aapt is required to verify the DEBUG APK identity.'
}

assert_debug_apk() {
    [ -f "$debug_apk" ] || die "DEBUG APK is missing: $debug_apk"
    local badging package label debuggable=0
    badging="$("$1" dump badging "$debug_apk" 2>&1)" ||
        die "aapt could not inspect '$debug_apk': $badging"
    package="$(sed -nE "s/^package: name='([^']+)'[[:space:]].*/\1/p" <<<"$badging" | head -1)"
    label="$(sed -nE "s/^application-label:'([^']*)'.*/\1/p" <<<"$badging" | head -1)"
    grep -qE '^application-debuggable[[:space:]]*$' <<<"$badging" && debuggable=1
    if [ "$package" != "$debug_package" ] || [ "$label" != 'Portal Debug' ] || [ "$debuggable" != 1 ]; then
        die "Refusing to install mismatched APK. Expected DEBUG / $debug_package / Portal Debug / debuggable; got $package / $label / debuggable=$debuggable."
    fi
}

# Returns 0 when the debug package is installed (and debuggable), 1 when absent.
installed_debug_identity() {
    local packages details
    packages="$(adb -s "$device_id" shell pm list packages 2>&1)" ||
        die "Could not read installed packages from $device_id."
    grep -qxF "package:$debug_package" <<<"$(tr -d '\r' <<<"$packages")" || return 1

    details="$(adb -s "$device_id" shell dumpsys package "$debug_package" 2>&1)" ||
        die "Could not inspect installed DEBUG package $debug_package."
    grep -qE '^[[:space:]]*(pkgFlags|flags)=\[[^]]*\bDEBUGGABLE\b' <<<"$details" ||
        die "Installed package $debug_package is not debuggable. Refusing to replace it."
    return 0
}

assert_installed_debug_bytes() {
    local paths remote_path remote_hash local_hash
    paths="$(adb -s "$device_id" shell pm path "$debug_package" 2>&1 | tr -d '\r')" ||
        die "Could not read installed APK path for $debug_package."
    remote_path="$(sed -nE 's#^package:(.*/base\.apk)[[:space:]]*$#\1#p' <<<"$paths" | head -1)"
    [ -n "$remote_path" ] || die "No base.apk path returned for installed package $debug_package."
    remote_hash="$(adb -s "$device_id" shell sha256sum "$remote_path" 2>&1 | grep -oiE '\b[0-9a-f]{64}\b' | head -1)" ||
        die "Could not verify installed APK bytes for $debug_package."
    local_hash="$(sha256sum "$debug_apk" | cut -d' ' -f1)"
    [ -n "$remote_hash" ] && [ "${remote_hash,,}" = "$local_hash" ] ||
        die "Installed DEBUG APK bytes do not match '$debug_apk'."
}

assert_debug_apk "$(resolve_aapt)"

state="$(adb -s "$device_id" get-state 2>&1 | tr -d '\r')" || true
[ "$state" = device ] ||
    die "ADB device '$device_id' is not connected and ready ($state). No installation was attempted."

# Stable is entirely outside this script's scope. -r updates this exact package
# while preserving its data; signature or identity mismatches remain hard
# failures with no fallback.
installed_debug_identity || true

echo "Installing DEBUG package $debug_package with adb install -r..."
if ! output="$(adb -s "$device_id" install -r "$debug_apk" 2>&1)"; then
    die "DEBUG install failed; no alternate package or destructive fallback was attempted. Details: $output"
fi
echo "$output"
installed_debug_identity ||
    die "DEBUG package $debug_package is not installed after adb reported success."
assert_installed_debug_bytes
echo "Verified DEBUG package $debug_package was updated in place; its app data and the Stable app were not touched."
