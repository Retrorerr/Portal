#!/bin/sh
set -e
export PROOT_TMP_DIR=/data/data/app.polarbear/files
export PROOT_LOADER=/data/app/~~pAPzoZZEJIV4W3TqT9m35w==/app.polarbear-41LrSRzvxHGHUtMOX96a4g==/lib/arm64/libproot_loader.so
/data/app/~~pAPzoZZEJIV4W3TqT9m35w==/app.polarbear-41LrSRzvxHGHUtMOX96a4g==/lib/arm64/libproot.so \
  -r /data/data/app.polarbear/files/arch -w / -L --link2symlink --sysvipc --kill-on-exit --root-id \
  --bind=/dev --bind=/proc --bind=/sys --bind=/data/data/app.polarbear/files/arch/tmp:/dev/shm --bind=/proc/self/fd:/dev/fd \
  /usr/bin/env -i HOME=/root LANG=C.UTF-8 PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  sh -c '
    set -eu
    LOCALDESKTOP_KWIN_STAGE=/var/lib/localdesktop/build-kwin/stage-6.7.4 \
      /var/lib/localdesktop/build-kwin/scripts/test-kwin-udev-guard.sh
  '