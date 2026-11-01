#!/usr/bin/env python3
"""Build the version-scoped, guest-glibc Qt animation preload."""
import argparse
from pathlib import Path
import subprocess

REPO = Path(__file__).resolve().parents[1]


def build(output, compiler='clang', target='aarch64-linux-gnu'):
    command = [compiler]
    if target:
        command += ['--target=' + target]
    command += ['-shared', '-fPIC', '-nostdlib', '-O2', '-fuse-ld=lld',
                '-ffreestanding', '-fno-stack-protector', '-Wall', '-Wextra', '-Werror',
                '-Wl,-z,max-page-size=16384',
                '-Wl,-soname,qt-anim-rate.so',
                '-Wl,--version-script=' + str(REPO / 'assets/guest-arm64/qt-anim-rate.map'),
                '-Wl,--build-id=sha1', '-s', '-o', str(output),
                str(REPO / 'assets/guest-arm64/qt-anim-rate.c')]
    subprocess.run(command, check=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--compiler', default='clang')
    parser.add_argument('--target', default='aarch64-linux-gnu')
    parser.add_argument('--output', type=Path, default=REPO / 'assets/guest-arm64/qt-anim-rate.so')
    args = parser.parse_args()
    build(args.output, args.compiler, args.target)
