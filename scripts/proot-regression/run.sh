#!/bin/bash
# Build the vendored PRoot for this Linux host and run Portal's regression
# cases against it (the same C sources the APK's libproot.so is built from).
#
# Needs a C compiler, make, libtalloc headers (libtalloc-dev) and lld: the
# loader links with --rosegment, which GNU ld does not support.
#
#   scripts/proot-regression/run.sh
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
cc=${CC:-cc}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cp -a "$repo/patches/build-proot-android/build/proot/src" "$work/src"

# The Android-only ashmem extension includes <linux/ashmem.h>, which desktop
# kernel headers do not ship; its constants are all it needs.
mkdir -p "$work/include/linux"
if ! echo '#include <linux/ashmem.h>' | "$cc" -E -x c - >/dev/null 2>&1; then
    cat >"$work/include/linux/ashmem.h" <<'EOF'
#ifndef _LINUX_ASHMEM_H
#define _LINUX_ASHMEM_H
#include <linux/ioctl.h>
#include <linux/types.h>
#include <stddef.h>
#define ASHMEM_NAME_LEN 256
#define __ASHMEMIOC 0x77
struct ashmem_pin { __u32 offset; __u32 len; };
#define ASHMEM_SET_NAME _IOW(__ASHMEMIOC, 1, char[ASHMEM_NAME_LEN])
#define ASHMEM_GET_NAME _IOR(__ASHMEMIOC, 2, char[ASHMEM_NAME_LEN])
#define ASHMEM_SET_SIZE _IOW(__ASHMEMIOC, 3, size_t)
#define ASHMEM_GET_SIZE _IO(__ASHMEMIOC, 4)
#define ASHMEM_SET_PROT_MASK _IOW(__ASHMEMIOC, 5, unsigned long)
#define ASHMEM_GET_PROT_MASK _IO(__ASHMEMIOC, 6)
#define ASHMEM_PIN _IOW(__ASHMEMIOC, 7, struct ashmem_pin)
#define ASHMEM_UNPIN _IOW(__ASHMEMIOC, 8, struct ashmem_pin)
#define ASHMEM_GET_PIN_STATUS _IO(__ASHMEMIOC, 9)
#define ASHMEM_PURGE_ALL_CACHES _IO(__ASHMEMIOC, 10)
#endif
EOF
fi

echo "Building PRoot for $(uname -m)"
if ! CPPFLAGS="-I$work/include" make -C "$work/src" -j"$(nproc)" proot \
    CC="$cc" LD="$cc -fuse-ld=lld" >"$work/build.log" 2>&1; then
    tail -n 60 "$work/build.log"
    exit 1
fi
"$cc" -O2 -Wall -Wextra -o "$work/getdents" "$repo/scripts/proot-regression/getdents.c"

export PROOT_TMP_DIR="$work"
proot=("$work/src/proot" -H -r /)
failures=0

expect() {
    local name=$1 expected=$2
    shift 2
    local actual status=0
    actual=$("${proot[@]}" "$@" 2>&1) || status=$?
    if [ "$status" -eq 0 ] && [ "$actual" = "$expected" ]; then
        echo "ok   $name"
    else
        echo "FAIL $name (exit $status)"
        echo "  expected: $(printf '%q' "$expected")"
        echo "  actual:   $(printf '%q' "$actual")"
        failures=$((failures + 1))
    fi
}

expect "getdents error on a removed directory passes through" \
    "deleted: No such file or directory" "$work/getdents" getdents deleted
expect "getdents error on a regular file passes through" \
    "notdir: Not a directory" "$work/getdents" getdents notdir

listing="$work/listing"
mkdir "$listing"
touch "$listing/visible" "$listing/.proot-meta-file.visible"
mkdir "$listing/.proot.l2s"
expect "-H hides .proot entries" \
    "visible" "$work/getdents" list "$listing" 32768
expect "-H survives a 64 MiB getdents buffer" \
    "visible" "$work/getdents" list "$listing" 67108864

if [ "$failures" -ne 0 ]; then
    echo "$failures PRoot regression case(s) failed"
    exit 1
fi
echo "All PRoot regression cases passed"
