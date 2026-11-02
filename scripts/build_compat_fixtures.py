#!/usr/bin/env python3
"""Build owned, disposable Linux/Windows binaries and short codec fixtures."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def build(output, kinds, appimage_runtime=None, runtime_sha256=None, media_size='320x180', frames=60):
    if not re.fullmatch(r'[1-9][0-9]*x[1-9][0-9]*', media_size) or frames < 1:
        raise ValueError('Media size must be WIDTHxHEIGHT and frame count must be positive')
    if any(int(value) % 2 for value in media_size.split('x')):
        raise ValueError('YUV 4:2:0 fixtures require even dimensions')
    output.mkdir(parents=True, exist_ok=True)
    results = {}
    def run(command, name):
        with (output / (name + '.build.log')).open('w') as log:
            subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=True)
    if 'linux' in kinds:
        if platform.machine().lower() != 'x86_64':
            raise RuntimeError('The Linux translation fixture must be built on x86-64')
        run(['cc', '-O2', str(ROOT / 'tests/compat/translated_probe.c'), '-o', str(output / 'linux-x86_64')], 'linux')
        results['linux'] = 'linux-x86_64'
    if 'windows' in kinds:
        for tool in ['clang', 'lld-link', 'llvm-dlltool']:
            if not shutil.which(tool):
                raise RuntimeError(f'{tool} is required to build the Windows fixture')
        definitions = output / 'kernel32.def'
        definitions.write_text('LIBRARY KERNEL32.dll\nEXPORTS\nGetStdHandle\nWriteFile\nExitProcess\nGetEnvironmentVariableA\nCreateFileA\nCloseHandle\n')
        run(['llvm-dlltool', '-m', 'i386:x86-64', '-d', str(definitions), '-l', str(output / 'kernel32.lib')], 'imports')
        run(['clang', '--target=x86_64-pc-windows-msvc', '-ffreestanding', '-fno-stack-protector', '-O2', '-c',
             str(ROOT / 'tests/compat/windows_probe.c'), '-o', str(output / 'windows.obj')], 'windows-compile')
        run(['lld-link', str(output / 'windows.obj'), str(output / 'kernel32.lib'),
             '/entry:mainCRTStartup', '/subsystem:console', '/nodefaultlib', '/out:' + str(output / 'windows.exe')], 'windows-link')
        results['windows'] = 'windows.exe'
    if 'media' in kinds:
        encoders = {'h264': ('libx264', ['-preset', 'ultrafast', '-pix_fmt', 'yuv420p']),
                    'hevc': ('libx265', ['-preset', 'ultrafast', '-x265-params', 'pools=2:frame-threads=2', '-pix_fmt', 'yuv420p']),
                    'vp9': ('libvpx-vp9', ['-deadline', 'realtime', '-cpu-used', '8', '-pix_fmt', 'yuv420p']),
                    'av1': ('libsvtav1', ['-preset', '12', '-svtav1-params', 'lp=2', '-pix_fmt', 'yuv420p'])}
        for name, (encoder, options) in encoders.items():
            filename = name + '.mp4'
            run(['ffmpeg', '-nostdin', '-hide_banner', '-y', '-f', 'lavfi', '-i',
                 'testsrc2=size=' + media_size + ':rate=30', '-frames:v', str(frames), '-an', '-threads', '2',
                 '-c:v', encoder, *options, str(output / filename)], name)
            results[name] = filename
    if 'appimage' in kinds:
        if not appimage_runtime or not runtime_sha256:
            raise RuntimeError('AppImage fixtures require --appimage-runtime and its --runtime-sha256')
        runtime = appimage_runtime.read_bytes()
        if hashlib.sha256(runtime).hexdigest() != runtime_sha256:
            raise RuntimeError('AppImage runtime SHA256 mismatch')
        if runtime[:4] != b'\x7fELF' or runtime[8:11] != b'AI\x02':
            raise RuntimeError('Expected an official type 2 AppImage runtime')
        with tempfile.TemporaryDirectory(dir=output) as tmp:
            appdir = Path(tmp) / 'AppDir'
            appdir.mkdir()
            entry = appdir / 'AppRun'
            entry.write_text('''#!/usr/bin/python3
import json, os, sys
from pathlib import Path
ok = (sys.argv[1:] == ['a b', '日本語'] and
      Path(os.environ['APPDIR']) == Path(__file__).parent and
      Path(os.environ['APPIMAGE']).is_file())
print(json.dumps(dict(appimage_extraction=ok, arguments_preserved=ok)), flush=True)
raise SystemExit(0 if ok else 1)
''')
            entry.chmod(0o755)
            squashfs = Path(tmp) / 'fixture.squashfs'
            run(['mksquashfs', str(appdir), str(squashfs), '-noappend', '-all-root',
                 '-comp', 'gzip', '-processors', '2'], 'appimage')
            image = output / 'fixture.AppImage'
            image.write_bytes(runtime + squashfs.read_bytes())
            image.chmod(0o755)
        results['appimage'] = 'fixture.AppImage'
    manifest = {kind: dict(file=file, sha256=hashlib.sha256((output / file).read_bytes()).hexdigest())
                for kind, file in results.items()}
    existing = output / 'manifest.json'
    if existing.is_file():
        for kind, item in json.loads(existing.read_text()).items():
            if kind not in manifest and Path(item['file']).name == item['file']:
                path = output / item['file']
                if path.is_file() and hashlib.sha256(path.read_bytes()).hexdigest() == item['sha256']:
                    manifest[kind] = item
    (output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    return manifest


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--kind', action='append', choices=['linux', 'windows', 'media', 'appimage'], required=True)
    parser.add_argument('--appimage-runtime', type=Path, help='Official type 2 runtime for the target architecture')
    parser.add_argument('--runtime-sha256', help='Expected SHA256 of the downloaded runtime')
    parser.add_argument('--media-size', default='320x180', help='Codec fixture dimensions, for example 1920x1080')
    parser.add_argument('--frames', type=int, default=60, help='Frames encoded in each codec fixture')
    args = parser.parse_args()
    print(json.dumps(build(args.output.resolve(), args.kind, args.appimage_runtime, args.runtime_sha256,
                           args.media_size, args.frames), indent=2))
