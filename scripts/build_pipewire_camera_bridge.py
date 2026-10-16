"""Reproduce the guest's PipeWire camera bridges from the ABI-matched Debian packages.

The runtime image ships libpipewire 1.4.2 but not these two optional pieces:
- pipewire-v4l2: libpw-v4l2.so, the upstream V4L2 emulation that serves
  /dev/videoN from PipeWire camera nodes (loaded on demand by
  assets/guest-arm64/portal-v4l2.c);
- gstreamer1.0-pipewire: libgstpipewire.so, so GStreamer camera apps
  (Kamoso, Cheese, Snapshot) list PipeWire cameras.

This adds no on-device package installation. setup.rs installs each library
only where dpkg does not already own it.

Run: python3 scripts/build_pipewire_camera_bridge.py
"""
import hashlib
import io
from pathlib import Path
import tarfile
import urllib.request

POOL = "https://deb.debian.org/debian/pool/main/p/pipewire/"
PACKAGES = [
    (
        "pipewire-v4l2_1.4.2-1_arm64.deb",
        "b5a96bc2a204460f642d508ba723a1079c4d4ee99922cb56652f24162358fbeb",
        {"/libpw-v4l2.so": "libpw-v4l2.so"},
    ),
    (
        "gstreamer1.0-pipewire_1.4.2-1_arm64.deb",
        "bfee02ba71905327356802a2c0e2a8d9b8fe9cd95ab67b193abc4ceff85c1aa2",
        {"/libgstpipewire.so": "libgstpipewire.so"},
    ),
]


def members(data: bytes):
    pos = 8
    while pos < len(data):
        header = data[pos:pos + 60]
        size = int(header[48:58])
        name = header[:16].decode().strip().rstrip("/")
        pos += 60
        yield name, data[pos:pos + size]
        pos += size + size % 2


def main() -> None:
    root = Path(__file__).resolve().parent.parent
    out_dir = root / "assets/guest-arm64"
    for deb, sha256, wanted in PACKAGES:
        with urllib.request.urlopen(POOL + deb, timeout=60) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != sha256:
            raise ValueError(f"{deb} checksum mismatch")
        for name, member in members(data):
            if not name.startswith("data.tar"):
                continue
            with tarfile.open(fileobj=io.BytesIO(member)) as archive:
                for item in archive:
                    target = next((out for suffix, out in wanted.items() if item.name.endswith(suffix)), None)
                    if target is None and item.name.endswith("/copyright"):
                        target = "pipewire-copyright"
                    if target is None:
                        continue
                    payload_file = archive.extractfile(item)
                    if payload_file is None:
                        continue
                    payload = payload_file.read()
                    (out_dir / target).write_bytes(payload)
                    print(target, hashlib.sha256(payload).hexdigest())


if __name__ == "__main__":
    main()
