"""Reproduce the APK's small Pulse backend from the ABI-matched Debian package.

This adds no on-device package installation or runtime image replacement.
The existing image supplies libcanberra0 0.30-18 and libpulse0.

Run: python3 scripts/build_canberra_backend.py
"""
import hashlib
import io
from pathlib import Path
import tarfile
import urllib.request

URL = "https://deb.debian.org/debian/pool/main/libc/libcanberra/libcanberra-pulse_0.30-18_arm64.deb"
SHA256 = "552ea19cfffa8d5e85fd7447e6629daefd5289df60b5d777022a523eb9c1b326"


def main() -> None:
    root = Path(__file__).resolve().parent.parent
    with urllib.request.urlopen(URL, timeout=60) as response:
        data = response.read()
    if hashlib.sha256(data).hexdigest() != SHA256:
        raise ValueError("Debian backend checksum mismatch")
    pos = 8
    while pos < len(data):
        header = data[pos:pos + 60]
        size = int(header[48:58])
        name = header[:16].decode().strip().rstrip("/")
        pos += 60
        member = data[pos:pos + size]
        pos += size + size % 2
        if name.startswith("data.tar"):
            with tarfile.open(fileobj=io.BytesIO(member)) as archive:
                for item in archive:
                    if item.name.endswith("/libcanberra-pulse.so") or item.name.endswith("/copyright"):
                        payload_file = archive.extractfile(item)
                        if payload_file is None:
                            continue
                        payload = payload_file.read()
                        output = root / "assets/guest-arm64" / ("libcanberra-pulse.so" if item.name.endswith(".so") else "libcanberra-copyright")
                        output.write_bytes(payload)
                        print(output.name, hashlib.sha256(payload).hexdigest())


if __name__ == "__main__":
    main()
