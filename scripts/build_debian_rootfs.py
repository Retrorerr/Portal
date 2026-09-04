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
    # Compression & archive tools
    "p7zip-full",
    "unzip",
    "zip",
    "zstd",
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

def extract_deb_package(deb_path: Path, dest_dir: Path, dpkg_info_dir: Path) -> str:
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

    # Debian dpkg on single-arch native systems uses <pkg_name>.<ext> in /var/lib/dpkg/info.
    # Furthermore, colons in filenames create Alternate Data Streams on Windows NTFS.
    pkg_id = pkg_name

    # Write other control files (md5sums, conffiles, postinst, etc.) to dpkg_info_dir
    for fname, m in other_control_members:
        f = ctrl_tar.extractfile(m)
        if f:
            target_path = dpkg_info_dir / f"{pkg_id}.{fname}"
            with open(target_path, "wb") as out_f:
                out_f.write(f.read())

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

    try:
        data_tar.extractall(path=dest_dir, filter="tar")
    except PermissionError:
        for member in data_tar.getmembers():
            target = dest_dir / member.name
            if target.exists() and not target.is_dir():
                try:
                    os.chmod(target, stat.S_IWRITE | stat.S_IREAD)
                except Exception:
                    pass
            try:
                data_tar.extract(member, path=dest_dir, filter="tar")
            except Exception:
                pass
    except Exception:
        for member in data_tar.getmembers():
            try:
                data_tar.extract(member, path=dest_dir, filter="tar")
            except Exception:
                pass

    # Write .list file
    list_path = dpkg_info_dir / f"{pkg_id}.list"
    with open(list_path, "w", newline="\n", encoding="utf-8") as f:
        f.write("\n".join(list_lines) + "\n")

    return status_stanza

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
        try:
            stanza = extract_deb_package(deb_file, output_dir, dpkg_info_dir)
            status_stanzas.append(stanza)
        except Exception as e:
            print(f"Error extracting {deb_file.name}: {e}")
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

    # Write /etc/apt/apt.conf.d/01portal-clean: non-interactive config prompts
    apt_clean = output_dir / "etc" / "apt" / "apt.conf.d" / "01portal-clean"
    with open(apt_clean, "w", newline="\n", encoding="utf-8") as f:
        f.write('DPkg::Options { "--force-confdef"; "--force-confold"; };\n')

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

if __name__ == "__main__":
    base_dir = Path(__file__).resolve().parent.parent
    target_rootfs = base_dir / "target" / "debian-13-rootfs"
    deb_cache = base_dir / "target" / "deb_cache"
    build_rootfs(target_rootfs, deb_cache)
