# Patched turnip

`libvulkan_freedreno.so` replaces the pinned lfdevs layer's turnip
(`mesa-26.3.0-devel-20260824`) with a newer one: lfdevs
`dev/adreno-main` 71160a88 (2026-10-08: KGSL Present wait-fence and native
BO fixes, A850) merged with upstream Mesa main 1f495287 (2026-10-12).

Upstream main brings the KGSL fixes Portal used to carry or lacked:
e984ef29 (fence polls sent WAITTIMESTAMP with timeout 0, which KGSL treats
as "wait forever", so wgpu/VKD3D-Proton polls blocked until the GPU idled)
and 1da50a1b (a submit waiting on both a timestamp and a sync file
converted the wrong one and crashed; Cemu, RPCS3, Dolphin).

Source:

    git fetch https://github.com/lfdevs/mesa-for-android-container 71160a888e198d11befe5f782cb007a5017ff503
    git checkout FETCH_HEAD
    git fetch https://gitlab.freedesktop.org/mesa/mesa.git 1f4952878d78caba95afcb4efa1a1dfaf4fbc692
    git merge -X theirs FETCH_HEAD     # then `git rm` .github/workflows/macos.yml
    git apply patches/mesa/lfdevs-71160a88-up-1f495287/0001-merge-resolution.patch

The patch keeps lfdevs' KGSL dma-buf/ION fallback (adapted to upstream's
new `tu_bo_init_dmabuf` signature), both new pipe context params, lfdevs'
extra A732 ids, and drops `magic_regs` from lfdevs-only GPU entries
(removed upstream). It also adds a WSI flag, `x11.xwayland_wait_for_fences`,
that Turnip sets on KGSL: Mesa presents to Xwayland without waiting for the
image's rendering and relies on dma-buf implicit fences, which KGSL never
attaches, so Xwayland could copy an image mid-render and show its previous
contents (games "jumping back" a frame). Verify with `vulkaninfo --summary`:
driverInfo `Mesa 26.3.0-devel (git-52147cad1)`, apiVersion 1.4.363.

Build (cross, Debian trixie amd64 -> arm64; the cross file also needs
`nm = 'aarch64-linux-gnu-nm'`):

    meson setup build --cross-file cross-arm64.ini --buildtype=release -Db_ndebug=true       -Dvulkan-drivers=freedreno -Dfreedreno-kmds=kgsl -Dgallium-drivers=freedreno       -Dplatforms=x11,wayland -Dopengl=false -Dgles1=disabled -Dgles2=disabled       -Degl=disabled -Dglx=disabled -Dgbm=disabled -Dllvm=disabled -Dvideo-codecs=       -Dtools= -Dvulkan-layers= -Dbuild-tests=false
    ninja -C build src/freedreno/vulkan/libvulkan_freedreno.so
    aarch64-linux-gnu-strip build/src/freedreno/vulkan/libvulkan_freedreno.so

Keep `-Dgallium-drivers=freedreno` even though only turnip is shipped: a
turnip-only KGSL build sets `system_has_kms_drm = false`, which compiles out
the X11 DRI3/Present WSI (`HAVE_X11_DRM`). X11 Vulkan clients (Proton/DXVK
games) then present by `xcb_put_image` of every frame, which tears and
stutters under Xwayland. Check with `nm -D` that the library imports
`xcb_dri3_*` and `xcb_present_*`.
