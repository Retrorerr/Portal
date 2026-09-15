#!/bin/sh
# Forky-native XWayland hardware-acceleration proof capture (Pad 3).
#
# Runs INSIDE the Portal Debian Forky guest (proot) with the active runtime.
# It never installs, replaces, or reconfigures anything: stock Forky XWayland
# (2:24.1.13-1 per assets/graphics-stack-lock.json) stays exactly as shipped.
# The script collects the evidence needed to decide whether stock XWayland
# gets hardware acceleration inside Portal, or whether a minimal KGSL
# surfaceless/DRI3 forward-port to the exact 24.1.13 source is required.
#
# Required proofs (all must reject llvmpipe/softpipe/software EGL):
#   - XWayland package version + Xwayland -version
#   - XWayland log (glamor/DRI3/EGL lines) from a real X11 client session
#   - glxinfo -B from an X11 client (renderer/vendor/accelerated)
#   - xdpyinfo / EGL client info if available
#
# Usage (from an adb shell with the guest running):
#   proot-distro login debian -- ./xwayland_forky_proof.sh
# Or copy this file into the guest and run it there. Output goes to
# /var/lib/localdesktop/xwayland-forky-proof-<timestamp>/ plus stdout summary.
#
# Exit codes: 0 = evidence collected (verdict printed, may be FAIL);
#             2 = environment unusable (no XWayland, no DISPLAY, missing tools).
set -eu

out_base=${1:-/var/lib/localdesktop}
stamp=$(date -u +%Y%m%dT%H%M%SZ 2>/dev/null || echo unknown)
out_dir="$out_base/xwayland-forky-proof-$stamp"
mkdir -p "$out_dir"

fail() { printf 'PROOF-ERROR: %s\n' "$*" >&2; exit 2; }

printf '== Forky XWayland hardware proof capture ==\n'
printf 'out_dir=%s\n' "$out_dir"

# 1. Package + binary identity (must match assets/graphics-stack-lock.json).
{
    printf '# dpkg-query xwayland\n'
    dpkg-query -W -f='${Package} ${Version} ${Architecture}\n' xwayland 2>&1 || echo 'MISSING: xwayland package'
    printf '# Xwayland -version\n'
    Xwayland -version 2>&1 || echo 'MISSING: Xwayland binary'
    printf '# which Xwayland\n'
    command -v Xwayland || echo 'MISSING: Xwayland in PATH'
} | tee "$out_dir/01-identity.txt"

# 2. Environment that matters for GL dispatch (informational only; the active
# Portal path scopes KGSL overrides to the KWin wrapper, never globally).
{
    printf '# env (filtered)\n'
    env | grep -E '^(DISPLAY|WAYLAND_DISPLAY|XDG_RUNTIME_DIR|MESA_|GALLIUM_|FD_|TURNIP_|EGL_|LIBGL_|__GLX)' | sort || true
    printf '# DISPLAY=%s WAYLAND_DISPLAY=%s\n' "${DISPLAY:-unset}" "${WAYLAND_DISPLAY:-unset}"
    printf '# /dev/dri nodes (expected: absent for app UID)\n'
    ls -la /dev/dri 2>&1 || echo 'no /dev/dri (expected inside Portal guest)'
} | tee "$out_dir/02-env.txt"

# 3. X11 client proof. Requires a running XWayland (started by KWin) and an
# X DISPLAY. glxinfo must come from mesa-utils; install is OUT OF SCOPE here
# (do not apt-install during proof: record absence instead).
{
    printf '# xdpyinfo\n'
    if command -v xdpyinfo >/dev/null 2>&1 && [ -n "${DISPLAY:-}" ]; then
        xdpyinfo 2>&1 | head -n 60 || echo 'xdpyinfo failed'
    else
        echo 'SKIP: xdpyinfo missing or DISPLAY unset'
    fi
    printf '# glxinfo -B\n'
    if command -v glxinfo >/dev/null 2>&1 && [ -n "${DISPLAY:-}" ]; then
        glxinfo -B 2>&1 | tee "$out_dir/03-glxinfo-B.txt" | head -n 60
    else
        echo 'SKIP: glxinfo missing or DISPLAY unset (install mesa-utils on a test guest only, never in the release rootfs)'
    fi
} | tee "$out_dir/03-x11-clients.txt"

# 4. Verdict on the collected glxinfo (strict: any software renderer fails).
verdict=INCONCLUSIVE
if [ -f "$out_dir/03-glxinfo-B.txt" ]; then
    if grep -qiE 'llvmpipe|softpipe|software|swrast|Software Rasterizer' "$out_dir/03-glxinfo-B.txt"; then
        verdict=FAIL-software-renderer
    elif grep -qiE 'freedreno|kgsl|turnip|adreno' "$out_dir/03-glxinfo-B.txt"; then
        verdict=PASS-hardware-renderer
    else
        verdict=INCONCLUSIVE-unknown-renderer
    fi
fi
printf '%s\n' "$verdict" | tee "$out_dir/00-VERDICT.txt"
printf 'verdict=%s\n' "$verdict"

# 5. Pointer to the logs the release owner must attach.
printf 'Attach %s plus /var/lib/localdesktop/kwin.log (glamor/DRI3 lines) for the release gate.\n' "$out_dir"
printf 'Gate: scripts/verify_graphics_stack.py --require-xwayland-proof stays RED until a PASS-hardware-renderer proof with no llvmpipe/softpipe/software EGL is recorded in assets/graphics-stack-lock.json.\n'
