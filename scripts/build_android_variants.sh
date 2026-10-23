#!/usr/bin/env bash
# Linux port of build_android_variants.ps1.
#
# Usage: scripts/build_android_variants.sh [--debug-only] [--validate-only]
#
# --debug-only     Build only Portal Debug. This is the default for device
#                  testing where Stable must keep using its last explicitly
#                  produced APK.
# --validate-only  Validate the previously built artifacts without invoking xbuild.
#
# Redirect the output to a file rather than piping it: the Gradle daemon can
# hold the pipe open after the build finishes. GRADLE_OPTS disables the daemon
# here as well.
set -euo pipefail

debug_only=0
validate_only=0
for arg in "$@"; do
    case "$arg" in
        --debug-only) debug_only=1 ;;
        --validate-only) validate_only=1 ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
manifest_path="$repo_root/manifest.yaml"
debug_manifest_path="$repo_root/manifest.debug.yaml"
xbuild_manifest_path="$repo_root/patches/xbuild/xbuild/Cargo.toml"
target_dir="$repo_root/target"

export ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
export GRADLE_OPTS="${GRADLE_OPTS:--Dorg.gradle.daemon=false}"
ndk_bin="$ANDROID_HOME/ndk/27.2.12479018/toolchains/llvm/prebuilt/linux-x86_64/bin"
[ -d "$ndk_bin" ] && export PATH="$ndk_bin:$PATH"

# xbuild compiles against the sysroot in ~/.cache/x/Android.ndk. It must be
# NDK 27.2: r28 headers hide API 26+ symbols (strict availability guards) and
# android-activity's system_utils.cpp stops compiling.
xbuild_ndk_usr="$HOME/.cache/x/Android.ndk/usr"
if [ -e "$xbuild_ndk_usr" ] && [[ "$(readlink -f "$xbuild_ndk_usr")" != */ndk/27.2.12479018/* ]]; then
    echo "error: $xbuild_ndk_usr resolves to $(readlink -f "$xbuild_ndk_usr"), expected NDK 27.2.12479018." >&2
    exit 1
fi

# SHA-256 of the maintainer's debug certificate; CI checks the same digest.
debug_cert_sha256=a512a376e17078bc9f8f4167457bc6c7be221a59d2e159c6d5750750266d748a

# This mapping is intentionally explicit. Stable and Debug are separate Android
# applications; never infer the target package from a file name or UI label.
declare -A stable=(
    [name]=Stable
    [manifest]="$manifest_path"
    [package]=app.polarbear.portal
    [label]=Portal
    [icon]=assets/portal-icon.png
    [gradle_root]="$target_dir/x/release/android/gradle"
    [apk]="$target_dir/Portal-stable-unsigned.apk"
    [debuggable]=0
)
declare -A debug=(
    [name]=DEBUG
    [manifest]="$debug_manifest_path"
    [package]=app.polarbear
    [label]="Portal Debug"
    [icon]=assets/portal-debug-icon.png
    [gradle_root]="$target_dir/x/debug/android/gradle"
    [apk]="$target_dir/Portal-Debug.apk"
    [debuggable]=1
)

die() { echo "error: $*" >&2; exit 1; }

latest_build_tools() {
    local dir="$ANDROID_HOME/build-tools"
    [ -d "$dir" ] || die "Android build-tools not found under $dir"
    local version
    version="$(ls "$dir" | sort -V | tail -1)"
    [ -n "$version" ] || die "no Android build-tools version installed in $dir"
    echo "$dir/$version"
}

resolve_aapt() {
    local tools aapt
    tools="$(latest_build_tools)"
    aapt="$tools/aapt"
    if [ -x "$aapt" ]; then echo "$aapt"; return; fi
    command -v aapt || die 'Android build-tools aapt is required to verify APK package identity and debuggability.'
}

assert_manifest_variant() {
    local -n v=$1
    [ -f "${v[manifest]}" ] || die "${v[name]} manifest is missing: ${v[manifest]}"
    local package label icon
    package="$(sed -nE 's/^[[:space:]]*package:[[:space:]]*([A-Za-z0-9_.]+)[[:space:]]*$/\1/p' "${v[manifest]}" | head -1)"
    label="$(sed -nE 's/^[[:space:]]+label:[[:space:]]*"([^"]+)"[[:space:]]*$/\1/p' "${v[manifest]}" | head -1)"
    icon="$(sed -nE 's/^icon:[[:space:]]*([^[:space:]]+)[[:space:]]*$/\1/p' "${v[manifest]}" | head -1)"
    if [ "$package" != "${v[package]}" ] || [ "$label" != "${v[label]}" ] || [ "$icon" != "${v[icon]}" ]; then
        die "${v[name]} manifest identity mismatch. Expected package=${v[package]}, label='${v[label]}', icon=${v[icon]}; got package=$package, label='$label', icon=$icon."
    fi
    [ -f "$repo_root/${v[icon]}" ] || die "${v[name]} icon asset is missing: $repo_root/${v[icon]}"
}

assert_adaptive_icon_resources() {
    local adaptive_dir="$1/app/src/main/res/mipmap-anydpi-v26"
    local f
    for f in ic_launcher.xml ic_launcher_foreground.xml ic_launcher_monochrome.xml; do
        [ -f "$adaptive_dir/$f" ] || die "Missing adaptive icon resource: $adaptive_dir/$f"
    done
    grep -q '#191B1C' "$adaptive_dir/ic_launcher.xml" ||
        die "Adaptive icon background is not Portal charcoal: $adaptive_dir/ic_launcher.xml"
}

assert_generated_variant() {
    local -n v=$1
    local app_dir="${v[gradle_root]}/app"
    local gradle_path="$app_dir/build.gradle"
    local android_manifest="$app_dir/src/main/AndroidManifest.xml"
    local f
    for f in "$gradle_path" "$android_manifest"; do
        [ -f "$f" ] || die "${v[name]} generated Android project is missing: $f"
    done

    local application_id
    application_id="$(sed -nE "s/^[[:space:]]*applicationId[[:space:]]*=[[:space:]]*'([^']+)'[[:space:]]*$/\1/p" "$gradle_path" | head -1)"
    [ "$application_id" = "${v[package]}" ] ||
        die "${v[name]} generated Gradle applicationId is '$application_id', expected '${v[package]}'."

    local generated_label
    generated_label="$(python3 -I -c '
import sys, xml.etree.ElementTree as ET
app = ET.parse(sys.argv[1]).getroot().find("application")
print(app.get("{http://schemas.android.com/apk/res/android}label", "") if app is not None else "")
' "$android_manifest")"
    [ "$generated_label" = "${v[label]}" ] ||
        die "${v[name]} generated Android label is '$generated_label', expected '${v[label]}'."

    assert_adaptive_icon_resources "${v[gradle_root]}"
}

assert_apk_variant() {
    local -n v=$1
    local aapt="$2"
    [ -f "${v[apk]}" ] || die "${v[name]} APK is missing: ${v[apk]}"
    [ -s "${v[apk]}" ] || die "${v[name]} APK is empty: ${v[apk]}"

    local badging
    badging="$("$aapt" dump badging "${v[apk]}" 2>&1)" ||
        die "aapt could not inspect ${v[apk]}: $badging"
    local package label debuggable=0
    package="$(sed -nE "s/^package: name='([^']+)'[[:space:]].*/\1/p" <<<"$badging" | head -1)"
    label="$(sed -nE "s/^application-label:'([^']*)'.*/\1/p" <<<"$badging" | head -1)"
    grep -qE '^application-debuggable[[:space:]]*$' <<<"$badging" && debuggable=1
    if [ "$package" != "${v[package]}" ] || [ "$label" != "${v[label]}" ] || [ "$debuggable" != "${v[debuggable]}" ]; then
        die "${v[name]} APK identity mismatch at '${v[apk]}'. Expected package=${v[package]}, label='${v[label]}', debuggable=${v[debuggable]}; got package=$package, label='$label', debuggable=$debuggable."
    fi
}

# The debug APK must carry the maintainer's debug key, or adb install -r
# cannot update the Portal Debug already on the device.
assert_debug_signature() {
    local apksigner certs
    apksigner="$(latest_build_tools)/apksigner"
    [ -x "$apksigner" ] || die "apksigner not found: $apksigner"
    certs="$("$apksigner" verify --print-certs "${debug[apk]}" 2>&1)" ||
        die "apksigner could not verify ${debug[apk]}: $certs"
    grep -q "certificate SHA-256 digest: $debug_cert_sha256" <<<"$certs" ||
        die "DEBUG APK is not signed with the maintainer's debug key (check ~/.android/debug.keystore)."
}

assert_variant_icons_differ() {
    local rel=app/src/main/res/mipmap-xxxhdpi/ic_launcher_foreground.png
    local stable_fg="${stable[gradle_root]}/$rel" debug_fg="${debug[gradle_root]}/$rel"
    local f
    for f in "$stable_fg" "$debug_fg"; do
        [ -f "$f" ] || die "Generated launcher foreground is missing: $f"
    done
    if [ "$(sha256sum <"$stable_fg")" = "$(sha256sum <"$debug_fg")" ]; then
        die 'Stable and DEBUG launcher artwork are identical; refusing to publish ambiguously branded APKs.'
    fi
}

assert_all_variants() {
    assert_manifest_variant stable
    assert_manifest_variant debug
    assert_generated_variant stable
    assert_generated_variant debug
    assert_variant_icons_differ
    assert_apk_variant stable "$1"
    assert_apk_variant debug "$1"
    assert_debug_signature
}

assert_manifest_variant stable
assert_manifest_variant debug
aapt="$(resolve_aapt)"

if [ "$validate_only" = 1 ]; then
    assert_all_variants "$aapt"
    echo 'Validated Stable app.polarbear.portal and DEBUG app.polarbear APK identities and distinct icons.'
    exit 0
fi

# Restore manifest.yaml however the build ends, including Ctrl-C, so a later
# Stable build can never pick up the debug identity.
manifest_backup="$(mktemp)"
cp -p "$manifest_path" "$manifest_backup"
restore_manifest() {
    cp -p "$manifest_backup" "$manifest_path"
    rm -f "$manifest_backup"
}
trap restore_manifest EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

cd "$repo_root"

if [ "$debug_only" = 0 ]; then
    echo 'Building optimized Stable Portal (app.polarbear.portal)...'
    cargo run --manifest-path "$xbuild_manifest_path" -- build --release --platform android --arch arm64 --format apk ||
        die "Stable xbuild failed with exit code $?"

    assert_generated_variant stable
    stable_unsigned="${stable[gradle_root]}/app/build/outputs/apk/release/app-release-unsigned.apk"
    [ -f "$stable_unsigned" ] || die "Stable unsigned APK is missing: $stable_unsigned"
    cp -f "$stable_unsigned" "${stable[apk]}"
    assert_apk_variant stable "$aapt"
fi

echo 'Building DEBUG Portal (app.polarbear)...'
cp -f "$debug_manifest_path" "$manifest_path"
assert_manifest_variant debug
cargo run --manifest-path "$xbuild_manifest_path" -- build --debug --features portal-debug --platform android --arch arm64 --format apk ||
    die "DEBUG xbuild failed with exit code $?"

assert_generated_variant debug
debug_gradle_apk="${debug[gradle_root]}/app/build/outputs/apk/debug/app-debug.apk"
[ -f "$debug_gradle_apk" ] || die "DEBUG APK is missing: $debug_gradle_apk"
cp -f "$debug_gradle_apk" "${debug[apk]}"
assert_apk_variant debug "$aapt"
assert_debug_signature
if [ "$debug_only" = 0 ]; then
    assert_variant_icons_differ
fi

if [ "$debug_only" = 1 ]; then
    echo "Built and verified DEBUG only: ${debug[apk]} -> ${debug[package]} (debuggable)"
else
    echo 'Built and verified:'
    echo "  Stable unsigned: ${stable[apk]} -> ${stable[package]} (not debuggable)"
    echo "  DEBUG:           ${debug[apk]} -> ${debug[package]} (debuggable)"
fi
echo 'This script only builds APKs. It never installs, uninstalls, or clears app data.'
