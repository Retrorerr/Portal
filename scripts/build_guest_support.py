#!/usr/bin/env python3
"""Cross-compile the existing PRoot socket-stat/crash shim for Debian ARM64.

Uses pinned Debian headers and clang (NDK clang works on Windows). The DSO
resolves glibc symbols from its guest process, never links Android bionic.
"""
import argparse
import hashlib
import io
import json
import subprocess
import tarfile
from pathlib import Path
from build_debian_rootfs import download_file, decompress_tar_data, fetch_package_index

REPO = Path(__file__).resolve().parent.parent
LOCK = REPO / "assets/guest-support-headers.json"

def build(clang, refresh_lock=False):
    cache = REPO / "target/deb_cache"
    if refresh_lock:
        index = fetch_package_index(cache / "Packages.txt")
        LOCK.write_text(json.dumps({p: {k: index[p][k] for k in ("Filename", "SHA256", "Version")}
            for p in ("libc6-dev", "linux-libc-dev")}, indent=2) + "\n", encoding="utf-8", newline="\n")
    sysroot = REPO / "target/guest-support-sysroot"
    for package in json.loads(LOCK.read_text()).values():
        deb = cache / Path(package["Filename"]).name
        if not download_file("https://deb.debian.org/debian/" + package["Filename"], deb):
            raise RuntimeError("Header download failed")
        with deb.open("rb") as stream:
            assert hashlib.file_digest(stream, "sha256").hexdigest() == package["SHA256"]
        data = deb.read_bytes()
        pos = 8
        while pos < len(data):
            header = data[pos:pos+60]
            name = header[:16].decode().strip()
            size = int(header[48:58])
            pos += 60
            payload = data[pos:pos+size]
            pos += size + size % 2
            if name.startswith("data.tar"):
                with tarfile.open(fileobj=io.BytesIO(decompress_tar_data(name, payload))) as archive:
                    for member in archive:
                        path = member.name.removeprefix("./")
                        if path.startswith("usr/include/") and (member.isfile() or member.islnk() or member.issym()):
                            target = sysroot / path
                            target.parent.mkdir(parents=True, exist_ok=True)
                            target.write_bytes(archive.extractfile(member).read())
    output = REPO / "assets/guest-arm64/localdesktop-crash-handler.so"
    output.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run([clang, "--target=aarch64-linux-gnu", "-fuse-ld=lld", "-nostdlib",
        "--sysroot=" + str(sysroot), "-isystem", str(sysroot / "usr/include/aarch64-linux-gnu"),
        "-shared", "-fPIC", "-fno-stack-protector", "-fno-omit-frame-pointer", "-O2", "-Wall", "-Wextra",
        "-Wl,-soname,localdesktop-crash-handler.so", "-Wl,--build-id=sha1",
        "-o", str(output), str(REPO / "assets/localdesktop-crash-handler.c")], check=True)
    print(output)

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--refresh-lock", action="store_true")
    args = parser.parse_args()
    build(args.clang, args.refresh_lock)
