#!/usr/bin/env python3
"""
Packages newly added completeness packages, the complete dpkg database,
apt configuration, and policy-rc.d into an update tarball, pushes it to
the OnePlus Pad 3 via adb, and extracts it into Slot B (/data/data/app.polarbear/files/runtime-B).
"""

import os
import sys
import io
import time
import lzma
import gzip
import tarfile
import subprocess
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor

def get_default_device_id() -> str:
    import os
    if os.environ.get("ANDROID_SERIAL"):
        return os.environ["ANDROID_SERIAL"]
    try:
        out = subprocess.check_output(["adb", "devices"], text=True)
        for line in out.strip().splitlines()[1:]:
            parts = line.split()
            if len(parts) >= 2 and parts[1] == "device":
                return parts[0]
    except Exception:
        pass
    return ""

DEVICE_ID = get_default_device_id()
APP_PKG = "app.polarbear"

def run_adb(cmd_args, capture=False):
    full_cmd = ["adb", "-s", DEVICE_ID] + cmd_args
    if capture:
        return subprocess.check_output(full_cmd, text=True, errors="replace")
    subprocess.check_call(full_cmd)

def decompress_tar_data(name: str, data: bytes) -> bytes:
    if name.endswith(".xz"):
        return lzma.decompress(data)
    elif name.endswith(".gz"):
        return gzip.decompress(data)
    elif name.endswith(".zst"):
        import zstandard
        return zstandard.ZstdDecompressor().decompress(data)
    return data

def process_deb_info(deb_path: Path):
    with open(deb_path, "rb") as f:
        data = f.read()
    pos = 8
    ctrl_data = None
    data_data = None
    while pos < len(data):
        h = data[pos:pos+60]
        if len(h) < 60:
            break
        name = h[:16].decode("ascii", errors="replace").strip()
        sz = int(h[48:58].decode("ascii", errors="replace").strip())
        pos += 60
        mdata = data[pos:pos+sz]
        pos += sz
        if pos % 2 != 0:
            pos += 1
        if name.startswith("control.tar"):
            ctrl_data = (name, mdata)
        elif name.startswith("data.tar"):
            data_data = (name, mdata)

    cname, cbytes = ctrl_data
    cdec = decompress_tar_data(cname, cbytes)

    ctrl_text = ""
    meta = {}
    other_ctrl = []
    with tarfile.open(fileobj=io.BytesIO(cdec)) as ctar:
        for m in ctar.getmembers():
            fn = Path(m.name).name
            if fn == "control":
                f = ctar.extractfile(m)
                if f:
                    ctrl_text = f.read().decode("utf-8", errors="replace")
                    for l in ctrl_text.splitlines():
                        if ":" in l and not l.startswith(" "):
                            k, v = l.split(":", 1)
                            meta[k.strip()] = v.strip()
            elif fn and fn != ".":
                f = ctar.extractfile(m)
                if f:
                    other_ctrl.append((fn, f.read()))

    pname = meta.get("Package", deb_path.name.split("_")[0])
    arch = meta.get("Architecture", "arm64")
    pkg_id = pname

    # data list
    dname, dbytes = data_data
    ddec = decompress_tar_data(dname, dbytes)
    list_lines = []
    with tarfile.open(fileobj=io.BytesIO(ddec)) as dtar:
        for m in dtar.getmembers():
            p = m.name
            if p.startswith("./"):
                p = p[1:]
            elif not p.startswith("/"):
                p = "/" + p
            p = p.rstrip("/")
            if not p:
                p = "/."
            list_lines.append(p)

    return pkg_id, other_ctrl, ("\n".join(list_lines) + "\n").encode("utf-8")

def main():
    repo_root = Path(__file__).resolve().parent.parent
    rootfs = repo_root / "target" / "debian-13-rootfs"
    deb_cache = repo_root / "target" / "deb_cache"
    packages_cache = deb_cache / "Packages.txt"

    sys.path.append(str(repo_root / "scripts"))
    from build_debian_rootfs import fetch_package_index, resolve_dependencies, SEED_PACKAGES

    packages = fetch_package_index(packages_cache)
    pkg_list = resolve_dependencies(packages, SEED_PACKAGES)
    print(f"Total packages in closure: {len(pkg_list)}")

    tar_path = repo_root / "target" / "plasma-completeness-update.tar"
    print(f"Creating completeness update archive at {tar_path}...")

    # Newly added packages that must have their payload files unpacked into guest
    new_packages = {
        "7zip", "apt", "ark", "breeze-gtk-theme", "bzip2", "ca-certificates",
        "debian-archive-keyring", "desktop-file-utils", "dpkg", "ffmpegthumbnailer",
        "fonts-urw-base35", "gpgv", "gtk2-engines-pixbuf", "gwenview", "kate",
        "kate-data", "kcalc", "kdegraphics-thumbnailers", "kdeplasma-addons-data",
        "kimageformat6-plugins", "kio-extras", "kio-extras-data", "kpackagetool6",
        "libapt-pkg7.0", "libavif16", "libc-bin", "libc-l10n", "libcfitsio10t64",
        "libde265-0", "libffmpegthumbnailer4v5", "libgav1-1", "libgdbm-compat4t64",
        "libgdbm6t64", "libgs-common", "libgs10", "libgs10-common", "libharfbuzz-subset0",
        "libheif-plugin-dav1d", "libheif-plugin-j2kdec", "libheif-plugin-libde265",
        "libheif-plugin-x265", "libheif1", "libijs-0.35", "libimath-3-1-29t64",
        "libjansson4", "libjbig2dec0", "libkcolorpicker-qt6-0", "libkdcrawqt6-5",
        "libkdsoap-qt6-2", "libkdsoapwsdiscoveryclient0", "libkf6bluezqt-data",
        "libkf6bluezqt6", "libkf6dnssd-data", "libkf6dnssd6", "libkf6pulseaudioqt5",
        "libkf6purpose-bin", "libkf6purpose-data", "libkf6purpose6", "libkf6purposewidgets6",
        "libkf6texteditor-katepart", "libkf6threadweaver6", "libkimageannotator-common",
        "libkimageannotator-qt6-0", "liblastlog2-2", "libldb2", "libminizip1t64",
        "libokular6core3", "libopenexr-3-1-30", "libpam-modules-bin", "libpaper2",
        "libperl5.40", "libpopt0", "libqt6concurrent6", "libqt6keychain1",
        "libqt6svgwidgets6", "libqt6webchannel6", "libqt6webchannelquick6",
        "libqt6webengine6-data", "libqt6webenginecore6", "libqt6webenginecore6-bin",
        "libqt6webenginequick6", "libraw23t64", "libseccomp2", "libsigsegv2",
        "libsmartcols1", "libsmbclient0", "libspectre1", "libtalloc2", "libtevent0t64",
        "libwbclient0", "libwebpdemux2", "libyuv0", "locales", "okular", "okular-data",
        "openssl", "p7zip-full", "perl", "perl-modules-5.40", "plasma-dataengines-addons",
        "plasma-pa", "plasma-runners-addons", "plasma-widgets-addons",
        "plasma-workspace-wallpapers", "poppler-data", "psmisc", "python3-minimal",
        "qml6-module-org-kde-bluezqt", "qml6-module-qtwebchannel", "qml6-module-qtwebengine",
        "qml6-module-sso-onlineaccounts", "samba-libs", "sqv", "unzip",
        "xfonts-encodings", "xfonts-utils", "zip", "zstd"
    }

    # Process all debs for dpkg info
    print("Extracting dpkg metadata and .list information across all 978 packages in parallel...")
    start = time.time()
    deb_paths = [deb_cache / Path(packages[p]["Filename"]).name for p in pkg_list]
    with ThreadPoolExecutor(max_workers=16) as ex:
        dpkg_results = list(ex.map(process_deb_info, deb_paths))
    print(f"Extracted dpkg info for {len(dpkg_results)} packages in {time.time() - start:.1f} s.")

    now = int(time.time())
    with tarfile.open(tar_path, "w") as tar:
        # Helper to add bytes
        def add_bytes(arcname, bcontent, mode=0o644):
            ti = tarfile.TarInfo(name=arcname)
            ti.size = len(bcontent)
            ti.mode = mode
            ti.mtime = now
            ti.uid = 0
            ti.gid = 0
            tar.addfile(ti, io.BytesIO(bcontent))

        # Helper to add dir
        def add_dir(arcname, mode=0o755):
            ti = tarfile.TarInfo(name=arcname)
            ti.type = tarfile.DIRTYPE
            ti.mode = mode
            ti.mtime = now
            ti.uid = 0
            ti.gid = 0
            tar.addfile(ti)

        # 1. Base directories
        for d in [
            "var/lib/dpkg", "var/lib/dpkg/info", "var/lib/dpkg/updates",
            "var/lib/dpkg/alternatives", "var/lib/dpkg/triggers",
            "var/lib/apt/lists/partial", "var/cache/apt/archives/partial",
            "var/log/apt", "etc/apt/apt.conf.d", "etc/apt/sources.list.d",
            "usr/local/bin"
        ]:
            add_dir(d)

        # 2. Write all /var/lib/dpkg/info/* files
        print("Adding /var/lib/dpkg/info files to tar archive...")
        for pkg_id, other_ctrl, list_bytes in dpkg_results:
            add_bytes(f"var/lib/dpkg/info/{pkg_id}.list", list_bytes)
            for fname, fbytes in other_ctrl:
                mode = 0o755 if fname in ("postinst", "preinst", "prerm", "postrm") else 0o644
                add_bytes(f"var/lib/dpkg/info/{pkg_id}.{fname}", fbytes, mode=mode)

        # 3. Write /var/lib/dpkg/status and status-old
        status_src = rootfs / "var" / "lib" / "dpkg" / "status"
        if status_src.exists():
            with open(status_src, "rb") as sf:
                status_bytes = sf.read()
            add_bytes("var/lib/dpkg/status", status_bytes)
            add_bytes("var/lib/dpkg/status-old", status_bytes)

        # 4. Standard dpkg files
        add_bytes("var/lib/dpkg/arch", b"arm64\n")
        add_bytes("var/lib/dpkg/available", b"")
        add_bytes("var/lib/dpkg/cmethopt", b"")
        add_bytes("var/lib/dpkg/lock", b"")
        add_bytes("var/lib/dpkg/lock-frontend", b"")
        add_bytes("var/lib/dpkg/triggers/Lock", b"")
        add_bytes("var/lib/dpkg/triggers/Unresolved", b"")
        add_bytes("var/log/dpkg.log", b"")

        # 5. Apt files
        add_bytes("var/log/apt/history.log", b"")
        add_bytes("var/log/apt/term.log", b"")
        sources_list = (
            "deb http://deb.debian.org/debian trixie main\n"
            "deb http://deb.debian.org/debian trixie-updates main\n"
            "deb http://security.debian.org/debian-security trixie-security main\n"
        ).encode("utf-8")
        add_bytes("etc/apt/sources.list", sources_list)
        add_bytes("etc/apt/apt.conf.d/01no-sandbox", b'APT::Sandbox::User "root";\n')
        add_bytes("etc/apt/apt.conf.d/01portal-clean", b'DPkg::Options { "--force-confdef"; "--force-confold"; };\n')

        # 6. Policy-rc.d
        add_bytes("usr/sbin/policy-rc.d", b"#!/bin/sh\nexit 101\n", mode=0o755)

        # 7. Startplasma launcher
        launcher_src = repo_root / "assets" / "localdesktop-startplasma.sh"
        with open(launcher_src, "rb") as lf:
            add_bytes("usr/local/bin/startplasma-localdesktop", lf.read(), mode=0o755)

        # 8. Unpack new packages' data files directly from their debs
        print(f"Unpacking payload files for {len(new_packages)} new completeness packages into tar...")
        for idx, pkg_name in enumerate(sorted(list(new_packages)), 1):
            if pkg_name not in packages:
                continue
            deb_path = deb_cache / Path(packages[pkg_name]["Filename"]).name
            with open(deb_path, "rb") as f:
                data = f.read()
            pos = 8
            data_data = None
            while pos < len(data):
                h = data[pos:pos+60]
                if len(h) < 60: break
                name = h[:16].decode("ascii", errors="replace").strip()
                sz = int(h[48:58].decode("ascii", errors="replace").strip())
                pos += 60
                mdata = data[pos:pos+sz]
                pos += sz
                if pos % 2 != 0: pos += 1
                if name.startswith("data.tar"):
                    data_data = (name, mdata)
                    break

            if not data_data:
                continue
            dname, dbytes = data_data
            ddec = decompress_tar_data(dname, dbytes)
            with tarfile.open(fileobj=io.BytesIO(ddec)) as dtar:
                for member in dtar.getmembers():
                    clean_name = member.name.lstrip("./").lstrip("/")
                    if not clean_name:
                        continue
                    member.name = clean_name
                    member.uid = 0
                    member.gid = 0

                    if member.islnk():
                        # Convert hardlink to relative symlink to avoid Android SELinux hardlink errors
                        member.type = tarfile.SYMTYPE
                        target = member.linkname.lstrip("./").lstrip("/")
                        member_dir = Path(member.name).parent
                        try:
                            rel = os.path.relpath(target, member_dir).replace("\\", "/")
                            member.linkname = rel
                        except Exception:
                            member.linkname = target
                    elif member.issym() and member.linkname.startswith("/"):
                        # Convert absolute symlink to relative symlink so toybox tar doesn't reject it
                        target = member.linkname.lstrip("/")
                        member_dir = Path(member.name).parent
                        try:
                            rel = os.path.relpath(target, member_dir).replace("\\", "/")
                            member.linkname = rel
                        except Exception:
                            pass

                    if member.isreg():
                        f = dtar.extractfile(member)
                        if f:
                            tar.addfile(member, f)
                    else:
                        tar.addfile(member)

    size_mb = tar_path.stat().st_size / (1024 * 1024)
    print(f"Completeness update archive created successfully: {size_mb:.1f} MB.")

    # Push to device
    print("Pushing update archive to device /data/local/tmp/...")
    run_adb(["push", str(tar_path), "/data/local/tmp/plasma-completeness-update.tar"])

    # Extract in guest
    print("Extracting update archive into Slot B (/data/data/app.polarbear/files/runtime-B)...")
    remote_extract = (
        "run-as app.polarbear /system/bin/tar -xf /data/local/tmp/plasma-completeness-update.tar "
        "-C /data/data/app.polarbear/files/runtime-B"
    )
    run_adb(["shell", remote_extract])

    # Clean up temp archive on device
    run_adb(["shell", "rm -f /data/local/tmp/plasma-completeness-update.tar"])

    print("Deployment to device Slot B completed successfully!")

if __name__ == "__main__":
    main()
