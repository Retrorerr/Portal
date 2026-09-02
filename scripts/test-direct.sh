#!/bin/sh
set -e
export PROOT_TMP_DIR=/data/data/app.polarbear/files
export PROOT_LOADER=/data/app/~~pAPzoZZEJIV4W3TqT9m35w==/app.polarbear-41LrSRzvxHGHUtMOX96a4g==/lib/arm64/libproot_loader.so
/data/app/~~pAPzoZZEJIV4W3TqT9m35w==/app.polarbear-41LrSRzvxHGHUtMOX96a4g==/lib/arm64/libproot.so \
  -r /data/data/app.polarbear/files/arch -w / -L --link2symlink --sysvipc --kill-on-exit --root-id \
  --bind=/dev --bind=/proc --bind=/sys --bind=/data/data/app.polarbear/files/arch/tmp:/dev/shm --bind=/proc/self/fd:/dev/fd \
  /usr/bin/env -i HOME=/root LANG=C.UTF-8 PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  sh -c '
    export LD_LIBRARY_PATH=/var/lib/localdesktop/build-kwin/stage-6.7.4/usr/lib
    export LD_PRELOAD=/var/lib/localdesktop/build-kwin/libkwin-null-udev.so
    export QT_FORCE_STDERR_LOGGING=1
    export QT_LOGGING_RULES="kwin_core.warning=true;kwin_core.debug=true"
    export XDG_RUNTIME_DIR=/var/lib/localdesktop/build-kwin/runtime-udev-test
    export KWIN_COMPOSE=Q
    timeout --foreground --kill-after=2s 4s /usr/bin/kwin_wayland --virtual --no-lockscreen --no-global-shortcuts --socket test-sock 2>&1
    echo "EXIT_CODE=$?"
  '