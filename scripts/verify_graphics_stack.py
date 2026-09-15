#!/usr/bin/env python3
"""Verify Portal's exact Android/PRoot graphics stack tuple.

This is intentionally a byte-and-identity check, not a best-effort version
check.  It verifies the active Forky package lock, the staged KWin binaries
and source patches, the pinned Mesa KGSL layer, the Anland protocol refs, and
the fact that the old XWayland candidate is quarantined rather than active.
It never downloads, extracts, installs, or mutates anything.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
MANIFEST_PATH = ROOT / "assets" / "graphics-stack-lock.json"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def source_overlay_sha256(path: Path) -> str:
    """Hash overlay paths and bytes in stable lexical order."""
    digest = hashlib.sha256()
    if not path.is_dir():
        raise ValueError(f"source overlay is not a directory: {path}")
    for entry in sorted(path.rglob("*")):
        if entry.is_symlink():
            raise ValueError(f"source overlay contains a symlink: {entry}")
        if not entry.is_file():
            continue
        relative = entry.relative_to(path).as_posix().encode("utf-8")
        digest.update(relative)
        digest.update(b"\0")
        with entry.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
        digest.update(b"\0")
    return digest.hexdigest()


def elf_identity(path: Path) -> dict[str, str | int | None]:
    data = path.read_bytes()
    if data[:4] != b"\x7fELF":
        raise ValueError(f"{path} is not an ELF file")
    if len(data) < 64 or data[4] != 2 or data[5] != 1:
        raise ValueError(f"{path} is not a little-endian ELF64 file")
    machine = struct.unpack_from("<H", data, 18)[0]
    if machine != 183:
        raise ValueError(f"{path} has ELF machine {machine}, expected AArch64 (183)")
    phoff = struct.unpack_from("<Q", data, 32)[0]
    phentsize = struct.unpack_from("<H", data, 54)[0]
    phnum = struct.unpack_from("<H", data, 56)[0]
    build_id: str | None = None
    for index in range(phnum):
        offset = phoff + index * phentsize
        if offset + 56 > len(data):
            raise ValueError(f"{path} has a truncated program-header table")
        p_type = struct.unpack_from("<I", data, offset)[0]
        if p_type != 4:  # PT_NOTE
            continue
        note_offset = struct.unpack_from("<Q", data, offset + 8)[0]
        note_size = struct.unpack_from("<Q", data, offset + 32)[0]
        cursor = note_offset
        end = note_offset + note_size
        while cursor + 12 <= end and cursor + 12 <= len(data):
            namesz, descsz, note_type = struct.unpack_from("<III", data, cursor)
            cursor += 12
            name_end = cursor + namesz
            name = data[cursor:name_end].rstrip(b"\0")
            cursor = (name_end + 3) & ~3
            desc_end = cursor + descsz
            desc = data[cursor:desc_end]
            cursor = (desc_end + 3) & ~3
            if note_type == 3 and name == b"GNU":
                build_id = desc.hex()
                break
        if build_id is not None:
            break
    return {
        "class": "ELF64",
        "data": "little",
        "machine": "AArch64",
        "machine_id": machine,
        "build_id": build_id,
    }


def load_json(path: Path) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise RuntimeError(f"cannot load {path}: {error}") from error


def check_path(path: Path, label: str, errors: list[str]) -> bool:
    if not path.is_file():
        errors.append(f"{label}: missing file {path.relative_to(ROOT)}")
        return False
    return True


def verify(
    manifest_path: Path = MANIFEST_PATH,
    *,
    require_xwayland_proof: bool = False,
) -> None:
    manifest = load_json(manifest_path)
    errors: list[str] = []

    runtime_path = ROOT / "assets" / "debian-runtime.json"
    packages_path = ROOT / "assets" / "debian-runtime-packages.json"
    if check_path(runtime_path, "runtime manifest", errors):
        runtime = load_json(runtime_path)
        expected_runtime = manifest["runtime"]
        for field in ("version", "sha256", "compressed_bytes"):
            if runtime.get(field) != expected_runtime[field]:
                errors.append(
                    f"runtime manifest {field}: {runtime.get(field)!r} != {expected_runtime[field]!r}"
                )
        if sha256(runtime_path) != manifest["runtime_manifest_sha256"]:
            errors.append("runtime manifest bytes do not match graphics-stack-lock.json")

    if check_path(packages_path, "package lock", errors):
        package_digest = sha256(packages_path)
        if package_digest != manifest["package_lock_sha256"]:
            errors.append(
                f"package lock SHA-256 {package_digest} != {manifest['package_lock_sha256']}"
            )
        packages = load_json(packages_path)
        for package, expected in manifest["packages"].items():
            actual = packages.get(package)
            if actual is None:
                errors.append(f"package lock is missing {package}")
                continue
            for field in ("Version", "SHA256"):
                if actual.get(field) != expected[field]:
                    errors.append(
                        f"package {package} {field}: {actual.get(field)!r} != {expected[field]!r}"
                    )

    for asset in manifest["kwin"]["assets"]:
        path = ROOT / asset["path"]
        if not check_path(path, asset["role"], errors):
            continue
        actual_size = path.stat().st_size
        actual_sha = sha256(path)
        if actual_size != asset["bytes"]:
            errors.append(f"{asset['role']} size {actual_size} != {asset['bytes']}")
        if actual_sha != asset["sha256"]:
            errors.append(f"{asset['role']} SHA-256 {actual_sha} != {asset['sha256']}")
        try:
            identity = elf_identity(path)
        except ValueError as error:
            errors.append(str(error))
        else:
            if identity != asset["elf"]:
                errors.append(f"{asset['role']} ELF identity {identity} != {asset['elf']}")

    for patch in manifest["kwin"]["patches"]:
        path = ROOT / patch["path"]
        if not check_path(path, patch["role"], errors):
            continue
        actual = sha256(path)
        if actual != patch["sha256"]:
            errors.append(f"{patch['role']} SHA-256 {actual} != {patch['sha256']}")

    overlay = manifest["kwin"].get("source_overlay")
    if not isinstance(overlay, dict):
        errors.append("KWin source overlay metadata is missing")
    else:
        overlay_path = ROOT / overlay["path"]
        if not overlay_path.is_dir():
            errors.append(f"KWin source overlay directory is missing: {overlay['path']}")
        else:
            try:
                actual_overlay = source_overlay_sha256(overlay_path)
            except (OSError, ValueError) as error:
                errors.append(str(error))
            else:
                if actual_overlay != overlay["sha256"]:
                    errors.append(
                        f"KWin source overlay SHA-256 {actual_overlay} != {overlay['sha256']}"
                    )
            protocol_header = overlay_path / "src" / "backends" / "anland" / "protocol.h"
            if check_path(protocol_header, "KWin Anland protocol header", errors):
                protocol = protocol_header.read_text(encoding="utf-8")
                needle = f"#define ANLAND_PROTOCOL_VERSION {overlay['protocol_version']}"
                if needle not in protocol:
                    errors.append("KWin Anland source overlay protocol version does not match the lock")

    candidate = manifest["xwayland"]["quarantined_candidate"]
    candidate_path = ROOT / candidate["path"]
    if check_path(candidate_path, "quarantined XWayland candidate", errors):
        if sha256(candidate_path) != candidate["sha256"]:
            errors.append("quarantined XWayland candidate bytes changed")
        if candidate["active"]:
            errors.append("quarantined XWayland candidate is marked active")

    mesa = manifest["mesa_layer"]
    mesa_source = ROOT / "src" / "android" / "proot" / "mesa_layer.rs"
    if check_path(mesa_source, "Mesa layer pin source", errors):
        source = mesa_source.read_text(encoding="utf-8")
        for field, needle in (
            ("version", f'pub const LAYER_VERSION: &str = "{mesa["version"]}";'),
            ("url", f'pub const LAYER_URL: &str = "{mesa["url"]}";'),
            ("sha256", f'pub const LAYER_SHA256: &str = "{mesa["sha256"]}";'),
            ("compressed_bytes", f'pub const LAYER_COMPRESSED_BYTES: u64 = {mesa["compressed_bytes"]};'),
        ):
            if needle not in source:
                errors.append(f"Mesa layer {field} pin is not present in mesa_layer.rs")

    xwayland = manifest["xwayland"]
    if xwayland["active"] != "debian-package:xwayland":
        errors.append("active XWayland policy is not the locked Debian package")
    proof = xwayland.get("hardware_proof", {})
    if require_xwayland_proof and proof.get("status") != "verified":
        errors.append(
            "XWayland hardware proof is not verified; release requires a Forky-native "
            "KGSL/DRI3 proof with no llvmpipe/softpipe/software EGL"
        )

    if errors:
        raise RuntimeError("graphics stack verification failed:\n- " + "\n- ".join(errors))

    print("Graphics stack lock verified.")
    print(f"  runtime: {manifest['runtime']['version']}")
    print(f"  KWin: {manifest['kwin']['source']['tag']} / {manifest['kwin']['source']['commit']}")
    print(
        f"  XWayland: {manifest['xwayland']['active']} "
        f"(hardware_proof={proof.get('status', 'missing')})"
    )
    print(f"  Mesa KGSL: {manifest['mesa_layer']['version']}")


def print_identities(manifest_path: Path = MANIFEST_PATH) -> None:
    manifest = load_json(manifest_path)
    for asset in manifest["kwin"]["assets"]:
        path = ROOT / asset["path"]
        identity = elf_identity(path)
        print(f"{asset['role']}: {json.dumps(identity, sort_keys=True)}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=MANIFEST_PATH)
    parser.add_argument(
        "--print-identities",
        action="store_true",
        help="print current KWin ELF identities without enforcing the lock",
    )
    parser.add_argument(
        "--require-xwayland-proof",
        action="store_true",
        help="fail unless the locked XWayland path has verified hardware evidence",
    )
    args = parser.parse_args()
    try:
        if args.print_identities:
            print_identities(args.manifest)
        else:
            verify(args.manifest, require_xwayland_proof=args.require_xwayland_proof)
    except (OSError, KeyError, TypeError, ValueError, RuntimeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
