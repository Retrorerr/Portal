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

from build_debian_rootfs import (
    SUDOERS_DROPIN_MODE,
    SUDOERS_DROPIN_PATH,
    build_rootfs,
    fetch_package_index,
    prepare_locked_packages_with_anland,
    resolve_dependencies,
    SEED_PACKAGES,
)

REPO = Path(__file__).resolve().parent.parent
LOCK = REPO / "assets/debian-runtime-packages.json"
# New canonical runtime: Debian base plus the pinned lfdevs Anland KWin/XWayland
# stack. Older versions (e.g. debian13-arm64-2026.09.05.3) are never rebuilt or
# replaced; pass --version explicitly to target a different image.
VERSION = "debian13-arm64-2026.10.08.1"
# The device decoder caps zstd windows at 2^27 (128 MiB); keep these in step
# with ZSTD_WINDOW_LOG_MAX in src/core/provisioning.rs.
ZSTD_LEVEL = 19
ZSTD_WINDOW_LOG = 27


def open_compressed(output):
    """zstd for .tar.zst (decodes ~7x faster than xz on device), xz otherwise."""
    if output.name.endswith(".tar.zst"):
        from compression import zstd  # Python 3.14+
        from compression.zstd import CompressionParameter as P
        return zstd.open(output, "wb", options={
            P.compression_level: ZSTD_LEVEL, P.window_log: ZSTD_WINDOW_LOG,
            P.enable_long_distance_matching: 1, P.checksum_flag: 1, P.nb_workers: 16})
    return lzma.open(output, "wb", preset=1)


def add_bytes(archive, name, data, mode=0o644):
    info = tarfile.TarInfo(name)
    info.mode = mode
    info.size = len(data)
    archive.addfile(info, io.BytesIO(data))


def extend_lock(seeds):
    """Lock the dependency closure of `seeds`, adding only what is not locked yet.

    Every entry already locked keeps its exact version, so a new package never
    drags the whole image onto newer Debian builds (that is --refresh-lock).
    Naming the seeds keeps unrelated drift between SEED_PACKAGES and the lock
    (packages Portal installs at first launch instead) out of the image. The
    index must be the one the lock was made from: an existing entry whose
    version differs from it means the cached index is stale, and nothing is
    written.
    """
    packages = fetch_package_index(REPO / "target/deb_cache/Packages.txt")
    locked = json.loads(LOCK.read_text())
    stale = sorted(n for n, entry in locked.items()
                   if n in packages and packages[n]["Version"] != entry["Version"])
    if stale:
        raise ValueError(f"Package index is not the one the lock was made from; differs for {stale[:5]}")
    unknown = sorted(set(seeds) - set(SEED_PACKAGES))
    if unknown:
        raise ValueError(f"Not in SEED_PACKAGES (add them there first): {unknown}")
    wanted = resolve_dependencies(packages, seeds)
    missing = set(seeds) - set(wanted)
    if missing:
        raise ValueError(f"Missing seed packages: {missing}")
    added = [name for name in wanted if name not in locked]
    for name in added:
        locked[name] = {key: packages[name][key] for key in ("Version", "Filename", "SHA256", "Size")}
    if added:
        LOCK.write_text(json.dumps(locked, indent=2) + "\n")
    print(f"Locked {len(added)} new packages: {', '.join(added) or 'none'}")


def build(output, refresh_lock=False, version=VERSION, with_anland=True):
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
    if with_anland:
        # Deterministic overlay: stock KWin/XWayland entries are replaced by
        # the verified lfdevs bundle (plus kwin-x11). The shipped
        # runtime-packages.json below records the effective set, so the
        # archive always describes exactly what was installed.
        packages = prepare_locked_packages_with_anland(packages, cache)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="portal-runtime-") as temporary:
        config = Path(temporary)
        payload_names: list[str] = []
        with open_compressed(output) as compressed, tarfile.open(fileobj=compressed, mode="w|") as archive:
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
                    mode = SUDOERS_DROPIN_MODE if name == SUDOERS_DROPIN_PATH else 0o755 if executable else 0o644
                    add_bytes(archive, name, path.read_bytes(), mode)
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
            add_bytes(archive, "etc/portal-runtime-version", (version + "\n").encode())
            add_bytes(archive, "usr/share/portal/runtime-packages.json",
                      (json.dumps(packages, indent=2) + "\n").encode())
    digest = hashlib.file_digest(output.open("rb"), "sha256").hexdigest()
    manifest = {"version": version,
        "url": f"https://github.com/Retrorerr/Portal/releases/download/runtime-{version}/{output.name}",
        "sha256": digest, "compressed_bytes": output.stat().st_size}
    print(json.dumps(manifest, indent=2), flush=True)
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--refresh-lock", action="store_true", help="Explicitly select new package versions")
    parser.add_argument("--extend-lock", nargs="+", metavar="PACKAGE",
                        help="Lock these SEED_PACKAGES and their new dependencies (existing versions unchanged) and exit")
    parser.add_argument("--version", default=VERSION, help="Runtime version marker (never reuse a published version)")
    parser.add_argument("--output", type=Path, default=None)
    parser.add_argument("--xz", action="store_true", help="Build the older .tar.xz image instead of .tar.zst")
    parser.add_argument("--no-anland", action="store_true", help="Build the pure Debian base without the lfdevs overlay")
    args = parser.parse_args()
    if args.extend_lock:
        extend_lock(args.extend_lock)
        raise SystemExit(0)
    output = args.output or (REPO / f"target/portal-{args.version}.tar.{'xz' if args.xz else 'zst'}")
    manifest = build(output, args.refresh_lock, args.version, not args.no_anland)
    # NOTE: assets/debian-runtime.json is only rewritten by
    # scripts/publish_runtime_release.py after the archive passes validation
    # and the bytes are durably published. A local build never retargets the
    # canonical manifest on its own.
