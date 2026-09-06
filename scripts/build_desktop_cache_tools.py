#!/usr/bin/env python3
"""Rebuild the APK's small Debian cache-tool payload from SHA256-locked packages.

These tools run the standard GLib/GTK triggers omitted by rootfs extraction.
They are embedded to repair both the published base image and existing installs.
"""
import hashlib
import json
import tarfile
import tempfile
import urllib.request
from pathlib import Path
from build_debian_rootfs import extract_deb_package

REPO = Path(__file__).resolve().parent.parent

def main():
    packages = json.loads((REPO / 'assets/debian-runtime-packages.json').read_text())
    output = REPO / 'assets/daily-use-tools.tar'
    with tempfile.TemporaryDirectory(prefix='portal-cache-tools-') as temporary:
        staging = Path(temporary)
        info = staging / 'info'
        info.mkdir()
        with tarfile.open(output, 'w') as archive:
            for name in ('libgtk-3-bin', 'libglib2.0-bin'):
                package = packages[name]
                cached = REPO / 'target/deb_cache' / Path(package['Filename']).name
                data = cached.read_bytes() if cached.exists() else urllib.request.urlopen(
                    'https://deb.debian.org/debian/' + package['Filename']).read()
                if hashlib.sha256(data).hexdigest() != package['SHA256']:
                    raise ValueError(f'Package checksum mismatch: {name}')
                deb = staging / cached.name
                deb.write_bytes(data)
                extract_deb_package(deb, staging, info, payload_tar=archive)
    print(f'{output}: {hashlib.sha256(output.read_bytes()).hexdigest()}')

if __name__ == '__main__':
    main()
