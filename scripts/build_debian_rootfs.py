#!/usr/bin/env python3
"""
Pre-provisions a minimal Debian 13 (Trixie) ARM64 KDE Plasma 6 desktop rootfs
off-device on the development machine.

Downloads the official Debian Trixie package catalog, resolves the transitive
dependency closure for Plasma 6, KWin, Dolphin, System Settings, Konsole, KScreen,
Breeze, D-Bus, XWayland, PipeWire, and Firefox ESR, downloads the .deb files
concurrently, and extracts them into a clean rootfs directory ready for packaging
and deployment into Local Desktop's Slot B.
"""

import os
import sys
import re
import io
import time
import lzma
import gzip
import tarfile
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

DEBIAN_MIRROR = "http://deb.debian.org/debian"
PACKAGES_URL = f"{DEBIAN_MIRROR}/dists/trixie/main/binary-arm64/Packages.xz"

SEED_PACKAGES = [
    # Core desktop & window manager
    "plasma-desktop",
    "kwin-wayland",
    "dolphin",
    "systemsettings",
    "konsole",
    "kscreen",
    "breeze",
    "breeze-icon-theme",
    "breeze-cursor-theme",
    # Essential Qt6 / KF6 components
    "qml6-module-qtquick-controls",
    "qml6-module-qtquick-layouts",
    "qml6-module-org-kde-kirigami",
    "qml6-module-org-kde-kquickcontrols",
    "qml6-module-org-kde-plasma-core",
    "qml6-module-org-kde-plasma-components",
    # Wayland, XWayland, IPC
    "xwayland",
    "dbus-user-session",
    "dbus-bin",
    "dbus-daemon",
    # Audio
    "pipewire",
    "wireplumber",
    "pipewire-pulse",
    # Browser
    "firefox-esr",
    # Fonts
    "fonts-dejavu-core",
    "fonts-noto-core",
    "fontconfig",
    # Graphics drivers / Mesa
    "libgl1-mesa-dri",
    "mesa-vulkan-drivers",
    # Essential base utilities & shells
    "base-files",
    "dash",
    "bash",
    "coreutils",
    "util-linux",
    "procps",
    "sed",
    "grep",
    "gawk",
    "findutils",
    "tar",
    "gzip",
    "xz-utils",
    "ca-certificates",
    "libc-bin",
]

# Packages to skip if pulled in as optional/heavy non-critical dependencies
EXCLUDE_PACKAGES = {
    "systemd",
    "systemd-boot",
    "systemd-resolved",
    "udev",
    "initramfs-tools",
    "linux-image-arm64",
    "grub-efi-arm64",
    "sddm",
    "lightdm",
    "gdm3",
}

def fetch_package_index(cache_file: Path) -> dict:
    if cache_file.exists() and cache_file.stat().st_size > 10 * 1024 * 1024:
        print(f"Loading cached Packages from {cache_file}...")
        with open(cache_file, "r", encoding="utf-8", errors="replace") as f:
            text = f.read()
    else:
        print(f"Downloading Packages.xz from {PACKAGES_URL}...")
        req = urllib.request.Request(PACKAGES_URL, headers={"User-Agent": "Portal-Provisioner/1.0"})
        with urllib.request.urlopen(req) as resp:
            compressed = resp.read()
        print(f"Decompressing {len(compressed)/(1024*1024):.1f} MB Packages.xz...")
        text = lzma.decompress(compressed).decode("utf-8", errors="replace")
        cache_file.parent.mkdir(parents=True, exist_ok=True)
        with open(cache_file, "w", encoding="utf-8") as f:
            f.write(text)

    packages = {}
    cur_pkg = {}
    for line in text.splitlines():
        if not line.strip():
            if "Package" in cur_pkg and "Filename" in cur_pkg:
                packages[cur_pkg["Package"]] = cur_pkg
            cur_pkg = {}
        elif ":" in line and not line.startswith(" "):
            k, v = line.split(":", 1)
            cur_pkg[k.strip()] = v.strip()
    if "Package" in cur_pkg and "Filename" in cur_pkg:
        packages[cur_pkg["Package"]] = cur_pkg

    print(f"Indexed {len(packages)} binary packages from Trixie main.")
    return packages

def resolve_dependencies(packages: dict, seeds: list) -> list:
    resolved = set()
    queue = list(seeds)

    while queue:
        pkg_name = queue.pop(0)
        clean = re.split(r"[\s(:]", pkg_name)[0].strip()
        if not clean or clean in resolved or clean in EXCLUDE_PACKAGES:
            continue

        if clean not in packages:
            # Check virtual package or provider
            continue

        resolved.add(clean)
        pkg = packages[clean]
        deps_str = pkg.get("Depends", "")
        if deps_str:
            for dep in deps_str.split(","):
                # Take first alternative in "pkgA | pkgB"
                alt = dep.strip().split("|")[0].strip()
                dep_name = re.split(r"[\s(:]", alt)[0].strip()
                if dep_name and dep_name not in resolved and dep_name not in EXCLUDE_PACKAGES:
                    queue.append(dep_name)

    print(f"Resolved {len(resolved)} total packages in dependency closure.")
    return sorted(list(resolved))

def download_file(url: str, dest: Path) -> bool:
    if dest.exists() and dest.stat().st_size > 0:
        return True
    dest.parent.mkdir(parents=True, exist_ok=True)
    temp = dest.with_suffix(".tmp")
    req = urllib.request.Request(url, headers={"User-Agent": "Portal-Provisioner/1.0"})
    try:
        with urllib.request.urlopen(req, timeout=30) as resp, open(temp, "wb") as f:
            while True:
                chunk = resp.read(64 * 1024)
                if not chunk:
                    break
                f.write(chunk)
        temp.replace(dest)
        return True
    except Exception as e:
        if temp.exists():
            temp.unlink()
        print(f"Failed to download {url}: {e}")
        return False

def extract_deb_data(deb_path: Path, dest_dir: Path):
    with open(deb_path, "rb") as f:
        deb_bytes = f.read()

    if deb_bytes[:8] != b"!<arch>\n":
        raise ValueError(f"{deb_path} is not a valid ar archive")

    pos = 8
    while pos < len(deb_bytes):
        header = deb_bytes[pos:pos+60]
        if len(header) < 60:
            break
        name = header[:16].decode("ascii", errors="replace").strip()
        size = int(header[48:58].decode("ascii", errors="replace").strip())
        pos += 60
        data = deb_bytes[pos:pos+size]
        pos += size
        if pos % 2 != 0:
            pos += 1

        if name.startswith("data.tar"):
            # Unpack data tarball
            if name.endswith(".xz"):
                decompressed = lzma.decompress(data)
                bio = io.BytesIO(decompressed)
            elif name.endswith(".gz"):
                decompressed = gzip.decompress(data)
                bio = io.BytesIO(decompressed)
            elif name.endswith(".zst"):
                import zstandard
                dctx = zstandard.ZstdDecompressor()
                decompressed = dctx.decompress(data)
                bio = io.BytesIO(decompressed)
            else:
                bio = io.BytesIO(data)

            with tarfile.open(fileobj=bio) as tar:
                tar.extractall(path=dest_dir, filter="tar")
            return

def build_rootfs(output_dir: Path, deb_cache_dir: Path):
    output_dir.mkdir(parents=True, exist_ok=True)
    deb_cache_dir.mkdir(parents=True, exist_ok=True)

    packages_cache = deb_cache_dir / "Packages.txt"
    packages = fetch_package_index(packages_cache)
    pkg_list = resolve_dependencies(packages, SEED_PACKAGES)

    total_bytes = sum(int(packages[p].get("Size", 0)) for p in pkg_list)
    print(f"Total download payload: {total_bytes / (1024*1024):.2f} MB across {len(pkg_list)} packages.")

    # 1. Download debs in parallel
    print("Downloading .deb files...")
    start_dl = time.time()
    download_tasks = []
    with ThreadPoolExecutor(max_workers=16) as executor:
        for pkg_name in pkg_list:
            pkg = packages[pkg_name]
            rel_path = pkg["Filename"]
            url = f"{DEBIAN_MIRROR}/{rel_path}"
            dest = deb_cache_dir / os.path.basename(rel_path)
            download_tasks.append(executor.submit(download_file, url, dest))

        completed = 0
        for future in as_completed(download_tasks):
            if future.result():
                completed += 1
                if completed % 50 == 0 or completed == len(download_tasks):
                    print(f"  Downloaded {completed}/{len(download_tasks)} debs...")
    print(f"Download complete in {time.time() - start_dl:.1f} s.")

    # 2. Extract debs into rootfs
    print(f"Extracting packages into {output_dir}...")
    start_ext = time.time()
    for idx, pkg_name in enumerate(pkg_list, 1):
        pkg = packages[pkg_name]
        deb_file = deb_cache_dir / os.path.basename(pkg["Filename"])
        try:
            extract_deb_data(deb_file, output_dir)
        except Exception as e:
            print(f"Error extracting {deb_file.name}: {e}")
        if idx % 100 == 0 or idx == len(pkg_list):
            print(f"  Extracted {idx}/{len(pkg_list)} packages...")
    print(f"Extraction complete in {time.time() - start_ext:.1f} s.")

    # 3. Setup standard system directories & symlinks
    print("Configuring system layout...")
    for d in ["dev", "dev/shm", "proc", "sys", "tmp", "run", "run/user/1000", "home/desktop", "var/lib/localdesktop", "etc/localdesktop"]:
        (output_dir / d).mkdir(parents=True, exist_ok=True)

    # Ensure /bin/sh exists
    bin_sh = output_dir / "bin" / "sh"
    if not bin_sh.exists() and (output_dir / "bin" / "dash").exists():
        try:
            os.symlink("dash", bin_sh)
        except Exception:
            pass

    # Ensure /etc/resolv.conf exists with UNIX newlines
    resolv_conf = output_dir / "etc" / "resolv.conf"
    if not resolv_conf.exists():
        with open(resolv_conf, "w", newline="\n", encoding="utf-8") as f:
            f.write("nameserver 8.8.8.8\nnameserver 1.1.1.1\n")

    # Ensure /etc/nsswitch.conf exists
    nsswitch_conf = output_dir / "etc" / "nsswitch.conf"
    if not nsswitch_conf.exists():
        with open(nsswitch_conf, "w", newline="\n", encoding="utf-8") as f:
            f.write(
                "passwd:         files\n"
                "group:          files\n"
                "shadow:         files\n"
                "gshadow:        files\n\n"
                "hosts:          files dns\n"
                "networks:       files\n\n"
                "protocols:      db files\n"
                "services:       db files\n"
                "ethers:         db files\n"
                "rpc:            db files\n\n"
                "netgroup:       nis\n"
            )

    # Ensure /etc/hosts exists
    hosts = output_dir / "etc" / "hosts"
    if not hosts.exists():
        with open(hosts, "w", newline="\n", encoding="utf-8") as f:
            f.write("127.0.0.1       localhost\n::1             localhost ip6-localhost ip6-loopback\n")

    # Setup SSL paths and symlinks
    (output_dir / "etc" / "ssl" / "certs").mkdir(parents=True, exist_ok=True)
    (output_dir / "usr" / "lib").mkdir(parents=True, exist_ok=True)
    usr_lib_ssl = output_dir / "usr" / "lib" / "ssl"
    if not usr_lib_ssl.exists():
        try:
            os.symlink("../../etc/ssl", usr_lib_ssl)
        except Exception:
            pass

    ssl_cert_pem = output_dir / "etc" / "ssl" / "cert.pem"
    if not ssl_cert_pem.exists():
        try:
            os.symlink("certs/ca-certificates.crt", ssl_cert_pem)
        except Exception:
            pass

    # Ensure /etc/passwd has root and desktop user
    passwd = output_dir / "etc" / "passwd"
    passwd_content = (
        "root:x:0:0:root:/root:/bin/bash\n"
        "desktop:x:1000:1000:desktop:/home/desktop:/bin/bash\n"
    )
    with open(passwd, "w", newline="\n", encoding="utf-8") as f:
        f.write(passwd_content)

    group = output_dir / "etc" / "group"
    group_content = (
        "root:x:0:\n"
        "desktop:x:1000:\n"
        "audio:x:29:desktop\n"
        "video:x:44:desktop\n"
    )
    group.write_text(group_content)

    print("Debian 13 rootfs pre-provisioning completed successfully!")

if __name__ == "__main__":
    base_dir = Path(__file__).resolve().parent.parent
    target_rootfs = base_dir / "target" / "debian-13-rootfs"
    deb_cache = base_dir / "target" / "deb_cache"
    build_rootfs(target_rootfs, deb_cache)
