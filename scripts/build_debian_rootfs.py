#!/usr/bin/env python3
"""
Pre-provisions a minimal Debian 13 (Trixie) ARM64 KDE Plasma 6 desktop rootfs
off-device on the development machine.

Downloads the official Debian Trixie package catalog, resolves the transitive
dependency closure for Plasma 6, KWin, Dolphin, System Settings, Konsole, KScreen,
Breeze, D-Bus, XWayland, PipeWire, and Firefox ESR, downloads the .deb files
concurrently, and extracts them into a clean rootfs directory ready for packaging
and deployment into Portal's runtime slots.
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
import hashlib
import json
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

DEBIAN_MIRROR = "https://deb.debian.org/debian"
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
    # Wayland, XWayland, IPC
    "xwayland",
    "dbus-user-session",
    "dbus-bin",
    "dbus-daemon",
    # Audio
    "pipewire",
    "wireplumber",
    "pipewire-pulse",
    "plasma-pa",
    # Browser
    "firefox-esr",
    "libgtk-3-bin",
    "libglib2.0-bin",
    # Fonts
    "fonts-dejavu-core",
    "fonts-noto-core",
    "fontconfig",
    # Locale, timezone, and desktop metadata tools
    "locales",
    "desktop-file-utils",
    "tzdata",
    "shared-mime-info",
    # Graphics drivers / Mesa
    "libgl1-mesa-dri",
    "mesa-vulkan-drivers",
    # Essential base utilities & shells
    "base-files",
    "dash",
    "bash",
    "coreutils",
    "util-linux",
    "bsdutils",
    "diffutils",
    "hostname",
    "ncurses-base",
    "ncurses-bin",
    "procps",
    "sed",
    "grep",
    "gawk",
    "findutils",
    "tar",
    "gzip",
    "xz-utils",
    "bzip2",
    "ca-certificates",
    "libc-bin",
    # D-Bus IPC system bus
    "dbus",
    "dbus-system-bus-common",
    # Debian package management suite
    "dpkg",
    "apt",
    "debian-archive-keyring",
    "gnupg",
    "gpgv",
    # Curated desktop application suite
    "ark",
    "gwenview",
    "okular",
    "kate",
    "kcalc",
    # Desktop integration, plugins & wallpapers
    "breeze-gtk-theme",
    "kio-extras",
    "kdegraphics-thumbnailers",
    "ffmpegthumbnailer",
    "plasma-widgets-addons",
    "plasma-workspace-wallpapers",
    "plasma-runners-addons",
    # GTK/Qt integration, browser integration, fonts & spellcheck
    "kde-config-gtk-style",
    "xsettings-kde",
    "plasma5-integration",
    "plasma-browser-integration",
    "webext-plasma-browser-integration",
    "fonts-noto-color-emoji",
    "fonts-hack",
    "sonnet6-plugins",
    "hunspell-en-gb",
    # Thumbnails and image formats
    "ffmpegthumbs",
    "kimageformat6-plugins",
    "qt6-image-formats-plugins",
    # Plasma addons, window management, help & docs
    "plasma-dataengines-addons",
    "plasma-calendar-addons",
    "plasma-wallpapers-addons",
    "kwin-addons",
    "kmenuedit",
    "khelpcenter",
    "plasma-desktop-doc",
    # Utilities
    "kcharselect",
    "filelight",
    "sweeper",
    "kde-spectacle",
    # Wallet, secrets & indexing
    "kwallet6",
    "kwalletmanager",
    "ksshaskpass",
    "baloo6",
    # Compression & archive tools
    "p7zip-full",
    "unzip",
    "zip",
    "zstd",
    "python3",  # Portal IME bridge
    "kdialog",
]

# Packages to skip if pulled in as optional/heavy non-critical dependencies
EXCLUDE_PACKAGES = {
    "plasma-systemmonitor",
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
    "network-manager",
    "bluez",
    "fwupd",
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
        deps_str = ", ".join(
            value for value in (pkg.get("Pre-Depends", ""), pkg.get("Depends", "")) if value
        )
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

def decompress_tar_data(name: str, data: bytes) -> bytes:
    if name.endswith(".xz"):
        return lzma.decompress(data)
    elif name.endswith(".gz"):
        return gzip.decompress(data)
    elif name.endswith(".zst"):
        import zstandard
        return zstandard.ZstdDecompressor().decompress(data)
    return data

def extract_deb_package(deb_path: Path, dest_dir: Path, dpkg_info_dir: Path, payload_tar=None, payload_names=None) -> str:
    """
    Extracts data.tar.* to dest_dir,
    captures extracted paths to /var/lib/dpkg/info/<pkg_id>.list,
    extracts control.tar.* metadata files to /var/lib/dpkg/info/<pkg_id>.<ext>,
    and returns the Status stanza to be written to /var/lib/dpkg/status.
    """
    with open(deb_path, "rb") as f:
        deb_bytes = f.read()

    if deb_bytes[:8] != b"!<arch>\n":
        raise ValueError(f"{deb_path} is not a valid ar archive")

    pos = 8
    control_bytes = None
    control_tar_name = None
    data_bytes = None
    data_tar_name = None

    while pos < len(deb_bytes):
        header = deb_bytes[pos:pos+60]
        if len(header) < 60:
            break
        name = header[:16].decode("ascii", errors="replace").strip()
        size = int(header[48:58].decode("ascii", errors="replace").strip())
        pos += 60
        member_data = deb_bytes[pos:pos+size]
        pos += size
        if pos % 2 != 0:
            pos += 1

        if name.startswith("control.tar"):
            control_tar_name = name
            control_bytes = member_data
        elif name.startswith("data.tar"):
            data_tar_name = name
            data_bytes = member_data

    if not control_bytes or not data_bytes:
        raise ValueError(f"Incomplete deb: {deb_path}")

    # Process control tarball
    ctrl_raw = decompress_tar_data(control_tar_name, control_bytes)
    ctrl_tar = tarfile.open(fileobj=io.BytesIO(ctrl_raw))

    control_content = ""
    control_meta = {}
    other_control_members = []

    for m in ctrl_tar.getmembers():
        fname = Path(m.name).name
        if fname == "control":
            f = ctrl_tar.extractfile(m)
            if f:
                control_content = f.read().decode("utf-8", errors="replace")
                for line in control_content.splitlines():
                    if ":" in line and not line.startswith(" "):
                        k, v = line.split(":", 1)
                        control_meta[k.strip()] = v.strip()
        elif fname and fname != ".":
            other_control_members.append((fname, m))

    pkg_name = control_meta.get("Package", deb_path.name.split("_")[0])
    arch = control_meta.get("Architecture", "arm64")
    multi_arch = control_meta.get("Multi-Arch", "")

    # dpkg qualifies the infodb basename for Multi-Arch:same packages.  The
    # package stanza remains `Package: libfoo` plus `Architecture: arm64`, but
    # every file below /var/lib/dpkg/info is named `libfoo:arm64.*`.  Keep this
    # distinction in the release tar: the archive is assembled directly from
    # Debian members and therefore preserves the colon even when the builder
    # itself is running on Windows (where a colon cannot be a normal filename).
    pkg_id = f"{pkg_name}:{arch}" if multi_arch == "same" and arch not in ("", "all") else pkg_name

    def write_info_member(fname: str, content: bytes, mode: int) -> None:
        archive_name = f"var/lib/dpkg/info/{pkg_id}.{fname}"
        if payload_tar is not None and ":" in pkg_id:
            info = tarfile.TarInfo(archive_name)
            info.mode = mode
            info.size = len(content)
            payload_tar.addfile(info, io.BytesIO(content))
            if payload_names is not None:
                payload_names.append(archive_name)
            return
        target_path = dpkg_info_dir / f"{pkg_id}.{fname}"
        with open(target_path, "wb") as out_f:
            out_f.write(content)
        target_path.chmod(mode)

    # Write other control files (md5sums, conffiles, postinst, etc.) to dpkg_info_dir
    for fname, m in other_control_members:
        f = ctrl_tar.extractfile(m)
        if f:
            write_info_member(fname, f.read(), m.mode)

    # Build status stanza: add Status: install ok installed after Package:
    status_lines = []
    has_status = False
    for line in control_content.splitlines():
        status_lines.append(line)
        if line.startswith("Package: ") and not has_status:
            status_lines.append("Status: install ok installed")
            has_status = True
    if not has_status:
        status_lines.append("Status: install ok installed")
    status_stanza = "\n".join(status_lines).strip() + "\n\n"

    # Process data tarball and record file paths
    data_raw = decompress_tar_data(data_tar_name, data_bytes)
    data_tar = tarfile.open(fileobj=io.BytesIO(data_raw))
    list_lines = []
    for m in data_tar.getmembers():
        p = m.name
        if p.startswith("./"):
            p = p[1:]
        elif not p.startswith("/"):
            p = "/" + p
        p = p.rstrip("/")
        if not p:
            p = "/."
        list_lines.append(p)

    if payload_tar is not None:
        import posixpath
        for member in data_tar.getmembers():
            member.name = member.name.removeprefix("./").rstrip("/")
            if member.name in ("", "."):
                continue
            if member.name.startswith("/") or ".." in member.name.split("/"):
                raise ValueError(f"Unsafe package path: {member.name}")
            member.uid = member.gid = 0
            member.uname = member.gname = "root"
            member.mtime = 0
            if member.islnk():
                # Android disallows hardlinks; materialize their package contents.
                content = data_tar.extractfile(member).read()
                member.type = tarfile.REGTYPE
                member.linkname = ""
                member.size = len(content)
                payload_tar.addfile(member, io.BytesIO(content))
                if payload_names is not None:
                    payload_names.append(member.name)
                continue
            if member.issym() and member.linkname.startswith("/"):
                member.linkname = posixpath.relpath(member.linkname.lstrip("/"), posixpath.dirname(member.name))
            if member.isreg():
                payload_tar.addfile(member, data_tar.extractfile(member))
            elif member.isdir() or member.issym():
                payload_tar.addfile(member)
            else:
                raise ValueError(f"Unsupported package entry: {member.name}")
            if payload_names is not None:
                payload_names.append(member.name)
    else:
        data_tar.extractall(path=dest_dir, filter="tar")

    # Write .list file.  Multi-Arch:same metadata is written directly to the
    # output tar when necessary so Windows never turns the colon into an NTFS
    # alternate data stream.
    write_info_member(
        "list",
        ("\n".join(list_lines) + "\n").encode("utf-8"),
        0o644,
    )

    return status_stanza

def build_rootfs(output_dir: Path, deb_cache_dir: Path, payload_tar=None, locked_packages=None, payload_names=None):
    output_dir.mkdir(parents=True, exist_ok=True)
    deb_cache_dir.mkdir(parents=True, exist_ok=True)

    packages_cache = deb_cache_dir / "Packages.txt"
    packages = locked_packages or fetch_package_index(packages_cache)
    pkg_list = sorted(packages) if locked_packages else resolve_dependencies(packages, SEED_PACKAGES)

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
            else:
                raise RuntimeError("Package download failed; refusing incomplete runtime")
    print(f"Download complete in {time.time() - start_dl:.1f} s.")

    # 2. Setup standard packaging directories
    dpkg_dir = output_dir / "var" / "lib" / "dpkg"
    dpkg_info_dir = dpkg_dir / "info"
    dpkg_updates_dir = dpkg_dir / "updates"
    dpkg_alternatives_dir = dpkg_dir / "alternatives"
    dpkg_triggers_dir = dpkg_dir / "triggers"
    for d in [dpkg_info_dir, dpkg_updates_dir, dpkg_alternatives_dir, dpkg_triggers_dir]:
        d.mkdir(parents=True, exist_ok=True)

    # 3. Extract debs into rootfs and build dpkg database
    print(f"Extracting packages into {output_dir} and populating dpkg database...")
    start_ext = time.time()
    status_stanzas = []
    for idx, pkg_name in enumerate(pkg_list, 1):
        pkg = packages[pkg_name]
        deb_file = deb_cache_dir / os.path.basename(pkg["Filename"])
        if hashlib.file_digest(deb_file.open("rb"), "sha256").hexdigest() != pkg["SHA256"]:
            raise ValueError(f"Package integrity failure: {deb_file}")
        stanza = extract_deb_package(deb_file, output_dir, dpkg_info_dir,
                                       payload_tar, payload_names)
        status_stanzas.append(stanza)
        if idx % 50 == 0 or idx == len(pkg_list):
            print(f"  Processed {idx}/{len(pkg_list)} packages...")
    print(f"Extraction and dpkg database generation complete in {time.time() - start_ext:.1f} s.")

    # Write /var/lib/dpkg/status and /var/lib/dpkg/status-old
    status_file = dpkg_dir / "status"
    with open(status_file, "w", newline="\n", encoding="utf-8") as f:
        f.write("".join(status_stanzas))
    status_old_file = dpkg_dir / "status-old"
    with open(status_old_file, "w", newline="\n", encoding="utf-8") as f:
        f.write("".join(status_stanzas))

    # Write /var/lib/dpkg/arch
    with open(dpkg_dir / "arch", "w", newline="\n", encoding="utf-8") as f:
        f.write("arm64\n")

    # Write /var/lib/dpkg/info/format (indicates infodb format 1)
    with open(dpkg_info_dir / "format", "w", newline="\n", encoding="utf-8") as f:
        f.write("1\n")

    # Touch /var/lib/dpkg/available and lock files
    for touch_file in [
        dpkg_dir / "available",
        dpkg_dir / "cmethopt",
        dpkg_dir / "lock",
        dpkg_dir / "lock-frontend",
        dpkg_triggers_dir / "Lock",
        dpkg_triggers_dir / "Unresolved",
        output_dir / "var" / "log" / "dpkg.log",
    ]:
        touch_file.parent.mkdir(parents=True, exist_ok=True)
        touch_file.touch(exist_ok=True)

    # Setup apt directories
    for apt_d in [
        output_dir / "var" / "lib" / "apt" / "lists" / "partial",
        output_dir / "var" / "lib" / "apt" / "mirrors" / "partial",
        output_dir / "var" / "lib" / "apt" / "periodic",
        output_dir / "var" / "cache" / "apt" / "archives" / "partial",
        output_dir / "var" / "log" / "apt",
        output_dir / "etc" / "apt" / "apt.conf.d",
        output_dir / "etc" / "apt" / "preferences.d",
        output_dir / "etc" / "apt" / "sources.list.d",
    ]:
        apt_d.mkdir(parents=True, exist_ok=True)

    for apt_log in [output_dir / "var" / "log" / "apt" / "history.log", output_dir / "var" / "log" / "apt" / "term.log"]:
        apt_log.touch(exist_ok=True)

    # Write /etc/apt/sources.list
    sources_list = output_dir / "etc" / "apt" / "sources.list"
    with open(sources_list, "w", newline="\n", encoding="utf-8") as f:
        f.write(
            "deb http://deb.debian.org/debian trixie main\n"
            "deb http://deb.debian.org/debian trixie-updates main\n"
            "deb http://security.debian.org/debian-security trixie-security main\n"
        )

    # Write /etc/apt/apt.conf.d/01no-sandbox: tells apt not to drop privileges to _apt in PRoot
    apt_no_sandbox = output_dir / "etc" / "apt" / "apt.conf.d" / "01no-sandbox"
    with open(apt_no_sandbox, "w", newline="\n", encoding="utf-8") as f:
        f.write('APT::Sandbox::User "root";\n')

    # Write /usr/sbin/policy-rc.d: prevent maintainer scripts from failing on init/systemd
    policy_rc_d = output_dir / "usr" / "sbin" / "policy-rc.d"
    policy_rc_d.parent.mkdir(parents=True, exist_ok=True)
    with open(policy_rc_d, "w", newline="\n", encoding="utf-8") as f:
        f.write("#!/bin/sh\nexit 101\n")
    try:
        os.chmod(policy_rc_d, 0o755)
    except Exception:
        pass

    # 4. Setup standard system directories & symlinks
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

    # Ensure /etc/default/locale exists
    default_locale = output_dir / "etc" / "default" / "locale"
    default_locale.parent.mkdir(parents=True, exist_ok=True)
    if not default_locale.exists():
        with open(default_locale, "w", newline="\n", encoding="utf-8") as f:
            f.write("LANG=en_GB.UTF-8\nLC_ALL=en_GB.UTF-8\n")

    # Ensure /etc/locale.gen exists
    locale_gen = output_dir / "etc" / "locale.gen"
    if not locale_gen.exists():
        with open(locale_gen, "w", newline="\n", encoding="utf-8") as f:
            f.write("en_GB.UTF-8 UTF-8\nen_US.UTF-8 UTF-8\n")

    # Ensure default Konsole profile exists
    konsole_dir = output_dir / "usr" / "share" / "konsole"
    konsole_dir.mkdir(parents=True, exist_ok=True)
    konsole_profile = konsole_dir / "Profile 1.profile"
    if not konsole_profile.exists():
        with open(konsole_profile, "w", newline="\n", encoding="utf-8") as f:
            f.write("[General]\nCommand=/bin/bash\nName=Profile 1\nParent=FALLBACK/\n\n[Appearance]\nColorScheme=Breeze\n")

    print("Debian 13 rootfs pre-provisioning completed successfully!")

# ---------------------------------------------------------------------------
# Official lfdevs Anland Termux KWin/XWayland stack (Debian 13, ARM64).
#
# Stock Debian kwin_wayland has no Anland backend (its CLI rejects --anland),
# so a runtime assembled from Debian alone cannot boot the Anland desktop.
# The finished Portal runtime therefore deterministically overlays the pinned
# lfdevs bundle below on top of the Debian base. Every external byte is
# hard-pinned (URL + size + SHA-256) and any mismatch fails the build closed.
# ---------------------------------------------------------------------------

LFDEVS_ANLAND_RELEASE_URL = "https://github.com/lfdevs/anland-termux/releases/download/5.13.3"
LFDEVS_KWIN_ZIP = {
    "filename": "kwin_anland-5.13-debian-4_6.3.6-95.zip",
    "size": 10604606,
    "sha256": "56ce1da27b640c977bad5ca0b7b13196e609b5fc419703429e51805ec05e4ee4",
    # Inner .deb digests exactly as published by lfdevs sha256sums.txt
    # inside the zip. Verified again after extraction; never trusted blindly.
    "members": {
        "kwin-common_6.3.6-95_arm64.deb": "bd4d59b6d00ddc64825ea791a30381c92207b5158c746f9d1c7aca19114dc16c",
        "kwin-data_6.3.6-95_all.deb": "5d8be6876fa86ccf9a4588c6fd9e2cc1c497e2de2f2d88a3ce371a8c8ff8c67c",
        "kwin-wayland_6.3.6-95_arm64.deb": "9d975f02f6b8e7970bc69f9a9fa71b9148de20ca2849fe3755862c45c0676993",
        "kwin-x11_6.3.6-95_arm64.deb": "fb406e1cd519b0dbe6a3c2f33eda931b90e13da397b9d7d757b5bebca5628713",
        "libkwin6_6.3.6-95_arm64.deb": "0e1659dec3c82577c52bf137d243cf29eb8ad3bba56e98d591ae11d004bf5c8b",
    },
}
LFDEVS_XWAYLAND_DEB = {
    "filename": "xwayland_24.1.6-91_arm64.deb",
    "size": 825848,
    "sha256": "59f9c7486d6a10ad50a13622bf1d1bbf5accd015d630e4b2b0152a80577dcc64",
}

# The complete intended bundle, installed as a unit. kwin-x11 is new (the
# Debian closure never seeded it); every hard dependency of all six packages
# was verified to resolve inside the existing locked Debian closure, so no
# extra Debian packages are pulled in.
LFDEVS_OVERLAY_ORDER = ("kwin-common", "kwin-data", "kwin-wayland", "kwin-x11", "libkwin6", "xwayland")

# Payload path fragments the overlay must never touch. Portal's Mesa/KGSL
# integration and the Debian Mesa/GL stacks live outside the KWin/XWayland
# file sets (verified: zero hits); any overlap fails the build instead of
# silently overwriting GPU files.
LFDEVS_OVERLAY_FORBIDDEN_PATH_FRAGMENTS = (
    "mesa",
    "libgl",
    "libegl",
    "libgles",
    "/dri/",
    "gallium",
    "kgsl",
    "vulkan",
    "freedreno",
    "adreno",
    "usr/local/",
)


def download_pinned(url: str, dest: Path, size: int, sha256: str) -> Path:
    """Download one external asset, failing closed on any size/hash mismatch."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    if not (dest.exists() and dest.stat().st_size == size):
        print(f"Downloading pinned external asset {dest.name} ({size} bytes)...")
        temp = dest.with_suffix(".tmp")
        req = urllib.request.Request(url, headers={"User-Agent": "Portal-Provisioner/1.0"})
        try:
            with urllib.request.urlopen(req, timeout=120) as resp, open(temp, "wb") as f:
                while True:
                    chunk = resp.read(1024 * 1024)
                    if not chunk:
                        break
                    f.write(chunk)
            temp.replace(dest)
        except Exception:
            if temp.exists():
                temp.unlink()
            raise
    actual_size = dest.stat().st_size
    if actual_size != size:
        raise ValueError(
            f"Pinned asset size mismatch for {dest.name}: got {actual_size}, expected {size}"
        )
    digest = hashlib.file_digest(dest.open("rb"), "sha256").hexdigest()
    if digest != sha256:
        raise ValueError(
            f"Pinned asset SHA-256 mismatch for {dest.name}: got {digest}, expected {sha256}"
        )
    print(f"Pinned asset verified: {dest.name} ({actual_size} bytes, sha256={digest[:16]}...)")
    return dest


def read_deb_control_fields(deb_path: Path) -> dict:
    """Parse the control file of a .deb without extracting its payload."""
    with open(deb_path, "rb") as f:
        deb_bytes = f.read()
    if deb_bytes[:8] != b"!<arch>\n":
        raise ValueError(f"{deb_path} is not a valid ar archive")
    pos = 8
    control_bytes = None
    control_tar_name = None
    while pos < len(deb_bytes):
        header = deb_bytes[pos:pos + 60]
        if len(header) < 60:
            break
        name = header[:16].decode("ascii", errors="replace").strip()
        size = int(header[48:58].decode("ascii", errors="replace").strip())
        pos += 60
        member_data = deb_bytes[pos:pos + size]
        pos += size + (size % 2)
        if name.startswith("control.tar"):
            control_tar_name = name
            control_bytes = member_data
    if not control_bytes:
        raise ValueError(f"No control member in {deb_path}")
    ctrl_raw = decompress_tar_data(control_tar_name, control_bytes)
    ctrl_tar = tarfile.open(fileobj=io.BytesIO(ctrl_raw))
    fields = {}
    for m in ctrl_tar.getmembers():
        if Path(m.name).name == "control":
            f = ctrl_tar.extractfile(m)
            if f:
                for line in f.read().decode("utf-8", errors="replace").splitlines():
                    if ":" in line and not line.startswith(" "):
                        k, v = line.split(":", 1)
                        fields[k.strip()] = v.strip()
    if "Package" not in fields or "Version" not in fields:
        raise ValueError(f"Control file of {deb_path} lacks Package/Version")
    return fields


def deb_data_member_names(deb_path: Path) -> list:
    """List data.tar member names of a .deb (for conflict inspection)."""
    with open(deb_path, "rb") as f:
        deb_bytes = f.read()
    pos = 8
    data_bytes = None
    data_tar_name = None
    while pos < len(deb_bytes):
        header = deb_bytes[pos:pos + 60]
        if len(header) < 60:
            break
        name = header[:16].decode("ascii", errors="replace").strip()
        size = int(header[48:58].decode("ascii", errors="replace").strip())
        pos += 60
        member_data = deb_bytes[pos:pos + size]
        pos += size + (size % 2)
        if name.startswith("data.tar"):
            data_tar_name = name
            data_bytes = member_data
    if not data_bytes:
        raise ValueError(f"No data member in {deb_path}")
    data_raw = decompress_tar_data(data_tar_name, data_bytes)
    data_tar = tarfile.open(fileobj=io.BytesIO(data_raw))
    return [m.name for m in data_tar.getmembers()]


def fetch_lfdevs_anland_stack(cache_dir: Path) -> dict:
    """
    Download (through this builder, never from Temp or device state) and
    verify the pinned lfdevs Anland stack. Returns {package_name: deb_path}
    for the complete intended bundle. Anything unexpected fails closed.
    """
    cache_dir.mkdir(parents=True, exist_ok=True)
    zip_path = download_pinned(
        f"{LFDEVS_ANLAND_RELEASE_URL}/{LFDEVS_KWIN_ZIP['filename']}",
        cache_dir / LFDEVS_KWIN_ZIP["filename"],
        LFDEVS_KWIN_ZIP["size"],
        LFDEVS_KWIN_ZIP["sha256"],
    )
    import zipfile

    with zipfile.ZipFile(zip_path) as zf:
        present = set(zf.namelist())
        expected = set(LFDEVS_KWIN_ZIP["members"])
        if present != expected | {"sha256sums.txt"}:
            raise ValueError(
                f"KWin zip contents differ from the pinned bundle: extra={sorted(present - expected - {'sha256sums.txt'})} "
                f"missing={sorted(expected - present)}"
            )
        deb_paths = {}
        for member, pinned_digest in LFDEVS_KWIN_ZIP["members"].items():
            data = zf.read(member)
            digest = hashlib.sha256(data).hexdigest()
            if digest != pinned_digest:
                raise ValueError(
                    f"Inner KWin bundle member SHA-256 mismatch for {member}: got {digest}"
                )
            dest = cache_dir / member
            if not (dest.exists() and hashlib.file_digest(dest.open("rb"), "sha256").hexdigest() == pinned_digest):
                dest.write_bytes(data)
            fields = read_deb_control_fields(dest)
            deb_paths[fields["Package"]] = dest
    xw_path = download_pinned(
        f"{LFDEVS_ANLAND_RELEASE_URL}/{LFDEVS_XWAYLAND_DEB['filename']}",
        cache_dir / LFDEVS_XWAYLAND_DEB["filename"],
        LFDEVS_XWAYLAND_DEB["size"],
        LFDEVS_XWAYLAND_DEB["sha256"],
    )
    xw_fields = read_deb_control_fields(xw_path)
    if xw_fields["Package"] != "xwayland":
        raise ValueError(f"Unexpected package in XWayland asset: {xw_fields['Package']}")
    deb_paths[xw_fields["Package"]] = xw_path
    missing = [name for name in LFDEVS_OVERLAY_ORDER if name not in deb_paths]
    if missing:
        raise ValueError(f"lfdevs bundle incomplete after verification: missing {missing}")
    assert_overlay_payload_safe(deb_paths)
    print(f"lfdevs Anland stack ready: {len(deb_paths)} verified packages.")
    return deb_paths


def assert_overlay_payload_safe(deb_paths: dict) -> None:
    """
    Deliberate conflict inspection: the overlay must not ship Mesa/GL/GPU or
    Portal-owned paths. Fails closed instead of overwriting GPU integration.
    """
    for name, deb_path in deb_paths.items():
        for member in deb_data_member_names(deb_path):
            lowered = member.lower().lstrip("./")
            for fragment in LFDEVS_OVERLAY_FORBIDDEN_PATH_FRAGMENTS:
                if fragment in lowered:
                    raise ValueError(
                        f"Refusing overlay: {deb_path.name} ships protected path '{member}' "
                        f"(fragment '{fragment}'). Resolve the Mesa/KGSL conflict deliberately."
                    )
    print("Overlay payload inspection passed: no Mesa/GL/GPU/Portal path overlap.")


def lfdevs_lock_entries(deb_paths: dict) -> dict:
    """Build lock-style entries ({Version, Filename, SHA256, Size}) for overlay debs."""
    entries = {}
    for name in LFDEVS_OVERLAY_ORDER:
        deb_path = deb_paths[name]
        fields = read_deb_control_fields(deb_path)
        if fields["Package"] != name:
            raise ValueError(f"Overlay deb {deb_path.name} is package {fields['Package']}, expected {name}")
        entries[name] = {
            "Version": fields["Version"],
            "Filename": deb_path.name,
            "SHA256": hashlib.file_digest(deb_path.open("rb"), "sha256").hexdigest(),
            "Size": deb_path.stat().st_size,
        }
    return entries


def prepare_locked_packages_with_anland(base_locked: dict, cache_dir: Path) -> dict:
    """
    Return the effective install set: Debian base lock with the stock
    kwin-wayland/kwin-common/kwin-data/libkwin6/xwayland payloads replaced by
    the verified lfdevs -95/-91 bundle, plus kwin-x11 added. Stock KWin/XWayland
    debs are never downloaded or extracted, so no conflicting payload or stale
    dpkg metadata can survive; status/info stanzas are generated from the
    actually installed debs by the existing machinery.
    """
    deb_paths = fetch_lfdevs_anland_stack(cache_dir)
    overlay = lfdevs_lock_entries(deb_paths)
    effective = dict(base_locked)
    replaced = sorted(name for name in overlay if name in effective)
    added = sorted(name for name in overlay if name not in effective)
    effective.update(overlay)
    print(f"Anland overlay: replaced stock packages {replaced}; added {added}.")
    return effective

if __name__ == "__main__":
    base_dir = Path(__file__).resolve().parent.parent
    target_rootfs = base_dir / "target" / "debian-13-rootfs"
    deb_cache = base_dir / "target" / "deb_cache"
    build_rootfs(target_rootfs, deb_cache)
