#!/bin/bash

set -e
shopt -s nullglob

. ./config

cd "$BUILD_DIR/proot/src"

for ARCH in $ARCHS
do

set-arch $ARCH

export CFLAGS="-I$STATIC_ROOT/include -Werror=implicit-function-declaration"
# 16 KB pages: Android 15+ devices and Google Play require every ELF LOAD
# segment to be 16 KB aligned. The loader carries the same flag in
# LOADER_LDFLAGS (build/proot/src/GNUmakefile).
export LDFLAGS="-L$STATIC_ROOT/lib -Wl,-z,max-page-size=16384 -Wl,--build-id=sha1"
export PROOT_UNBUNDLE_LOADER="$INSTALL_ROOT/libexec/proot"

if [ "$SUBARCH" == 'pre5' ]
then export ANDROID_PRE5=1
else unset ANDROID_PRE5
fi

make distclean || true
make V=1 "PREFIX=$INSTALL_ROOT" install
make distclean || true
CFLAGS="$CFLAGS -DUSERLAND" make V=1 "PREFIX=$INSTALL_ROOT" proot
cp -a ./proot "$INSTALL_ROOT/bin/proot-userland"

(
cd "$INSTALL_ROOT/bin"
for FN in *
do
"$STRIP" "$FN"
done
)

(
cd "$PROOT_UNBUNDLE_LOADER"
for FN in *
do
"$STRIP" "$FN"
done
)

done
