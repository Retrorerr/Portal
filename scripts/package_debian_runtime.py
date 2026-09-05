#!/usr/bin/env python3
"""Build the release image from locked Debian packages, never from a device rootfs.

Payloads go directly from deb tar members to the release tar to preserve Linux
modes and links on Windows too. Only generated configuration uses the host FS.
"""
import argparse
import hashlib
import io
import json
import lzma
import tarfile
import tempfile
from pathlib import Path

from build_debian_rootfs import build_rootfs, fetch_package_index, resolve_dependencies, SEED_PACKAGES

REPO = Path(__file__).resolve().parent.parent
LOCK = REPO / "assets/debian-runtime-packages.json"
VERSION = "debian13-arm64-2026.09.05.2"


def add_bytes(archive, name, data, mode=0o644):
    info = tarfile.TarInfo(name)
    info.mode = mode
    info.size = len(data)
    archive.addfile(info, io.BytesIO(data))


def build(output, refresh_lock=False):
    cache = REPO / "target/deb_cache"
    if refresh_lock:
        packages = fetch_package_index(cache / "Packages.txt")
        names = resolve_dependencies(packages, SEED_PACKAGES)
        missing = set(SEED_PACKAGES) - set(names)
        if missing:
            raise ValueError(f"Missing seed packages: {missing}")
        LOCK.write_text(json.dumps({name: {key: packages[name][key] for key in
            ("Version", "Filename", "SHA256", "Size")} for name in names}, indent=2) + "\n")
    packages = json.loads(LOCK.read_text())
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="portal-runtime-") as temporary:
        config = Path(temporary)
        payload_names: list[str] = []
        with lzma.open(output, "wb", preset=1) as compressed, tarfile.open(fileobj=compressed, mode="w|") as archive:
            build_rootfs(config, cache, payload_tar=archive, locked_packages=packages,
                         payload_names=payload_names)
            # These are supplied from Android on every setup/launch, not a build machine.
            for relative in ("etc/resolv.conf", "etc/timezone", "etc/localtime", "etc/machine-id"):
                path = config / relative
                if path.exists() or path.is_symlink():
                    path.unlink()
            for path in sorted(config.rglob("*")):
                name = path.relative_to(config).as_posix()
                if path.is_symlink():
                    continue  # canonical relative links below
                if path.is_file():
                    executable = name == "usr/sbin/policy-rc.d" or name.endswith((".postinst", ".preinst", ".prerm", ".postrm", ".config"))
                    add_bytes(archive, name, path.read_bytes(), 0o755 if executable else 0o644)
                elif path.is_dir():
                    member = tarfile.TarInfo(name)
                    member.type = tarfile.DIRTYPE
                    member.mode = 0o1777 if name == "tmp" else 0o755
                    archive.addfile(member)
            # A canonical link must never shadow real payload content: the device
            # extractor strictly rejects a symlink over a non-empty directory,
            # and silently dropping payload files is worse. Empty payload
            # directories are still replaced by the link at install time.
            payload_paths = set(payload_names)
            for name, target in {"bin": "usr/bin", "sbin": "usr/sbin", "lib": "usr/lib",
                                 "usr/bin/sh": "dash", "usr/lib/ssl": "../../etc/ssl",
                                 "etc/ssl/cert.pem": "certs/ca-certificates.crt"}.items():
                if any(entry != name and entry.startswith(name + "/") for entry in payload_paths):
                    print(f"Skipping canonical link {name}: payload ships content below it")
                    continue
                member = tarfile.TarInfo(name)
                member.type = tarfile.SYMTYPE
                member.linkname = target
                member.mode = 0o777
                archive.addfile(member)
            # The APK synchronizes the authoritative session scripts before launching.
            add_bytes(archive, "etc/portal-runtime-version", (VERSION + "\n").encode())
            add_bytes(archive, "usr/share/portal/runtime-packages.json", LOCK.read_bytes())
    digest = hashlib.file_digest(output.open("rb"), "sha256").hexdigest()
    manifest = {"version": VERSION,
        "url": f"https://github.com/Retrorerr/Portal/releases/download/runtime-{VERSION}/{output.name}",
        "sha256": digest, "compressed_bytes": output.stat().st_size}
    (REPO / "assets/debian-runtime.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(manifest, indent=2), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--refresh-lock", action="store_true", help="Explicitly select new package versions")
    parser.add_argument("--output", type=Path, default=REPO / f"target/portal-{VERSION}.tar.xz")
    args = parser.parse_args()
    build(args.output, args.refresh_lock)
