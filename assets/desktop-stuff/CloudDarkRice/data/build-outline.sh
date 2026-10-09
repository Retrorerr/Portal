#!/usr/bin/env bash
# Build KDE-Rounded-Corners (the outline effect) against the KWin Portal is running. Run as root:
#   sudo bash build-outline.sh KDE-Rounded-Corners.tgz
# Portal pins its own KWin build (Anland backend) and Debian's kwin-dev depends on Debian's KWin, so
# kwin-dev is never installed: its headers + CMake files are unpacked to /opt/kwin-dev instead, with the
# library pointed at Portal's libkwin.so.6.
set -euo pipefail
src=$(realpath "$1")
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq cmake extra-cmake-modules g++ make gettext qt6-base-dev qt6-base-private-dev \
  qt6-declarative-dev libkf6configwidgets-dev libkf6i18n-dev libkf6kcmutils-dev libkf6config-dev \
  libkf6coreaddons-dev libkf6windowsystem-dev libkf6globalaccel-dev libepoxy-dev libxcb1-dev libdrm-dev libwayland-dev
rm -rf /opt/kwin-dev && mkdir -p /opt/kwin-dev && cd /opt/kwin-dev
apt-get download kwin-dev && dpkg -x kwin-dev_*.deb root
lib=/usr/lib/aarch64-linux-gnu
real=$(readlink -f $lib/libkwin.so.6)
ln -sfn "$real" "root$lib/$(basename "$real")"
ln -sfn "$real" "root$lib/libkwin.so"
rm -rf /opt/krc && mkdir -p /opt/krc && tar -C /opt/krc -xzf "$src"
cmake -S /opt/krc -B /opt/krc/build -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr \
  -DCMAKE_PREFIX_PATH=/opt/kwin-dev/root/usr >/dev/null
make -C /opt/krc/build -j"$(nproc)" >/dev/null
make -C /opt/krc/build install >/dev/null
echo "outline effect built for $(/usr/bin/kwin_wayland --version)"
