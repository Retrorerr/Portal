# Patched turnip

`libvulkan_freedreno.so` replaces the pinned lfdevs layer's turnip
(`mesa-26.3.0-devel-20260824`, commit 98f3d622) with the same source plus
`patches/mesa/lfdevs-26.3.0-20260824/0001-tu-kgsl-poll-zero-timeout-waits.patch`.

The stock build sends KGSL WAITTIMESTAMP with timeout 0 for fence polls, and
KGSL treats 0 as "wait forever", so every wgpu/Firefox WebGPU poll blocked
until the GPU idled. The patch is a backport of upstream Mesa MR 44838: polls read the
retired timestamp instead. Drop this file once the layer pin moves past it.

Build (cross, Debian trixie amd64 -> arm64):

    meson setup build --cross-file cross-arm64.ini --buildtype=release -Db_ndebug=true \
      -Dvulkan-drivers=freedreno -Dfreedreno-kmds=kgsl -Dgallium-drivers= \
      -Dplatforms=x11,wayland -Dopengl=false -Dgles1=disabled -Dgles2=disabled \
      -Degl=disabled -Dglx=disabled -Dgbm=disabled -Dllvm=disabled -Dvideo-codecs= \
      -Dtools= -Dvulkan-layers= -Dbuild-tests=false
    ninja -C build src/freedreno/vulkan/libvulkan_freedreno.so
    aarch64-linux-gnu-strip build/src/freedreno/vulkan/libvulkan_freedreno.so
