#!/bin/bash
# Build Portal's Mesa KGSL layer as a Flatpak GL extension,
# org.freedesktop.Platform.GL.portal, for one freedesktop runtime branch.
#
# Same lfdevs tag and Portal patches as ../build.sh (the guest's own layer),
# plus this directory's patches, built against org.freedesktop.Sdk//<branch>
# so it links the runtime's libraries. Inside the sandbox it is mounted at
# /usr/lib/aarch64-linux-gnu/GL/portal and, being listed first in
# FLATPAK_GL_DRIVERS and carrying a higher priority than GL.default, its
# libGLX_mesa/libEGL_mesa/Vulkan ICD win. No LLVM: softpipe is the software
# fallback (kgsl, then zink on Turnip, come first).
#
# Runs natively on ARM64 Linux with flatpak and flatpak-builder; CI uses
# .github/workflows/mesa-kgsl-flatpak-arm64.yml. Writes to <out-dir>:
#
#   org.freedesktop.Platform.GL.portal-<branch>-aarch64.flatpak
#   SHA256SUMS
#
#   build.sh <portal-checkout> <out-dir> <branch>
set -euo pipefail

PORTAL=$(realpath "$1")
OUT=$(realpath -m "$2")
BRANCH=$3

LFDEVS_MESA_REPO=https://github.com/lfdevs/mesa-for-android-container.git
LFDEVS_MESA_SHA=98f3d6229d61452cef80f8563af7c56ae599dc14
LAYER_DIR="$PORTAL/patches/mesa/kgsl-layer-26.3.0"
EXT_ID=org.freedesktop.Platform.GL.portal
PREFIX=/usr/lib/aarch64-linux-gnu/GL/portal

case "$BRANCH" in
    24.08 | 25.08) ;;
    *) echo "unsupported runtime branch: $BRANCH" >&2; exit 2 ;;
esac

flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user -y --noninteractive flathub \
    "org.freedesktop.Platform//$BRANCH" "org.freedesktop.Sdk//$BRANCH"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/patches"
cp "$LAYER_DIR"/[0-9][0-9][0-9][0-9]-*.patch "$work/patches/"
for patch in "$LAYER_DIR"/flatpak/[0-9][0-9][0-9][0-9]-*.patch; do
    cp "$patch" "$work/patches/flatpak-$(basename "$patch")"
done
patch_list=$(cd "$work/patches" && ls | sort | sed 's|^|          - patches/|')

cat > "$work/$EXT_ID.yml" <<EOF
id: $EXT_ID
branch: '$BRANCH'
runtime: org.freedesktop.Platform
runtime-version: '$BRANCH'
sdk: org.freedesktop.Sdk
build-extension: true
separate-locales: false
appstream-compose: false
build-options:
  prefix: $PREFIX
  libdir: $PREFIX/lib
  prepend-pkg-config-path: $PREFIX/lib/pkgconfig
  env:
    PYTHONPATH: $PREFIX/pytools
cleanup:
  - /include
  - /lib/pkgconfig
  - /pytools
modules:
  # Mesa's code generators; removed from the extension by cleanup.
  - name: python-build-deps
    buildsystem: simple
    build-commands:
      - pip3 install --no-index --no-build-isolation --find-links=. --target=\${FLATPAK_DEST}/pytools mako pyyaml packaging
    sources:
      - type: file
        url: https://files.pythonhosted.org/packages/87/fb/99f81ac72ae23375f22b7afdb7642aba97c00a713c217124420147681a2f/mako-1.3.10-py3-none-any.whl
        sha256: baef24a52fc4fc514a0887ac600f9f1cff3d82c61d4d700a1fa84d597b88db59
      - type: file
        url: https://files.pythonhosted.org/packages/b2/97/5d42485e71dfc078108a86d6de8fa46db44a1a9295e89c5d6d4a06e23a62/markupsafe-3.0.2.tar.gz
        sha256: ee55d3edf80167e48ea11a923c7386f4669df67d7994554387f84e7d8b0a2bf0
      - type: file
        url: https://files.pythonhosted.org/packages/54/ed/79a089b6be93607fa5cdaedf301d7dfb23af5f25c398d5ead2525b063e17/pyyaml-6.0.2.tar.gz
        sha256: d584d9ec91ad65861cc08d42e834324ef890a082e591037abe114850ff7bbc3e
      - type: file
        url: https://files.pythonhosted.org/packages/20/12/38679034af332785aac8774540895e234f4d07f7545804097de4b666afd8/packaging-25.0-py3-none-any.whl
        sha256: 29572ef2b1f17581046b3a2227d5c611fb25ec70ca1ba8554b24b0e69331a484

  - name: mesa
    buildsystem: meson
    config-opts:
      - -Dplatforms=x11,wayland
      - -Dgallium-drivers=freedreno,zink,softpipe
      - -Dgallium-va=disabled
      - -Dgallium-mediafoundation=disabled
      - -Dvulkan-drivers=freedreno
      - -Dvulkan-layers=
      - -Degl=enabled
      - -Dgbm=enabled
      - -Dgles2=enabled
      - -Dglvnd=enabled
      - -Dglx=dri
      - -Dllvm=disabled
      - -Dlmsensors=disabled
      - -Dlibunwind=disabled
      - -Dintel-rt=disabled
      - -Dmicrosoft-clc=disabled
      - -Dvalgrind=disabled
      - -Dgles1=disabled
      - -Dfreedreno-kmds=kgsl
      - -Dbuildtype=release
    sources:
      - type: git
        url: $LFDEVS_MESA_REPO
        commit: $LFDEVS_MESA_SHA
      - type: patch
        use-git: true
        paths:
$patch_list
    post-install:
      # The GL extension point merges these from the extension's top level.
      - mv \${FLATPAK_DEST}/share/glvnd \${FLATPAK_DEST}/glvnd
      - mv \${FLATPAK_DEST}/share/vulkan \${FLATPAK_DEST}/vulkan
EOF
cat "$work/$EXT_ID.yml"

cd "$work"
flatpak-builder --user --disable-rofiles-fuse --force-clean build "$EXT_ID.yml"
# Mounted ahead of GL.default (priority 0) when both are active drivers.
sed -i '/^\[ExtensionOf\]/a priority=1' build/metadata
cat build/metadata
flatpak build-export repo build "$BRANCH"
mkdir -p "$OUT"
name="$EXT_ID-$BRANCH-aarch64.flatpak"
flatpak build-bundle --runtime repo "$OUT/$name" "$EXT_ID" "$BRANCH"
find build/files -maxdepth 3 | sort
(cd "$OUT" && sha256sum "$name" > SHA256SUMS && cat SHA256SUMS)
