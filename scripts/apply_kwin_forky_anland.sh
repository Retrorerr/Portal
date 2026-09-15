#!/bin/sh
# Apply Portal's pinned KWin fixes and the complete Anland v3 source overlay.
# This script is intentionally source-only: it never stages or replaces a
# package-managed KWin binary.
set -eu

source_root=${1:?usage: $0 KWIN_SOURCE_ROOT}
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
overlay_root=$repo_root/patches/kwin/anland-6.7.4
expected_commit=8438567a741826da8b7536a8b10eb3af8fc8820d

[ -d "$source_root/.git" ] || {
    echo "KWin source is not a Git checkout: $source_root" >&2
    exit 1
}
[ "$(git -C "$source_root" rev-parse HEAD)" = "$expected_commit" ] || {
    echo "KWin source commit does not match $expected_commit" >&2
    exit 1
}

for kwin_patch in "$repo_root"/patches/kwin/000[1-5]-*.patch; do
    if git -C "$source_root" apply --check --ignore-space-change --ignore-whitespace "$kwin_patch"; then
        git -C "$source_root" apply --ignore-space-change --ignore-whitespace "$kwin_patch"
    elif git -C "$source_root" apply --reverse --check --ignore-space-change --ignore-whitespace "$kwin_patch"; then
        echo "already applied: $(basename "$kwin_patch")"
    else
        echo "patch is neither applicable nor already applied: $kwin_patch" >&2
        exit 1
    fi
done

[ -f "$overlay_root/src/backends/anland/protocol.h" ] || {
    echo "Anland overlay is incomplete: protocol.h is missing" >&2
    exit 1
}
grep -Fq '#define ANLAND_PROTOCOL_VERSION 3' "$overlay_root/src/backends/anland/protocol.h"

find "$overlay_root/src" -type l -exec sh -c 'echo "overlay must not contain symlinks: $1" >&2; exit 1' sh {} \;
find "$overlay_root/src" -type f -print | while IFS= read -r overlay_file; do
    relative=${overlay_file#"$overlay_root/"}
    destination=$source_root/$relative
    mkdir -p "$(dirname -- "$destination")"
    cp -f -- "$overlay_file" "$destination"
done

echo "Applied KWin 0001-0005 and Anland v3 overlay to $source_root"
