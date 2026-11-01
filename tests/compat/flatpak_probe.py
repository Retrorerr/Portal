#!/usr/bin/python3
"""Run an owned native graphics fixture with ordinary Flatpak GPU permissions."""
import argparse
import os
import json
import re
import shutil
from pathlib import Path
import subprocess
import sys
import uuid

parser = argparse.ArgumentParser()
parser.add_argument('--seconds', type=float, default=4)
parser.add_argument('--branch', default='25.08')
parser.add_argument('--directory', type=Path, required=True)
parser.add_argument('--cleanup', action='store_true')
parser.add_argument('--installation', choices=['user', 'system'], default='user')
args = parser.parse_args()
base = args.directory
identity = base / 'app-id'
scope = '--' + args.installation
administrator = ['sudo', '-n'] if args.installation == 'system' else []
if args.cleanup:
    if identity.is_file():
        app_id = identity.read_text().strip()
        if re.fullmatch(r'org\.portal\.CompatibilityProbe\.x[0-9a-f]{32}', app_id):
            subprocess.run([*administrator, 'flatpak', 'uninstall', scope, '--noninteractive', '-y', app_id],
                            capture_output=True, timeout=30)
            shutil.rmtree(Path.home() / '.var/app' / app_id, ignore_errors=True)
    raise SystemExit(0)
runtime = 'org.freedesktop.Platform/aarch64/' + args.branch
available = subprocess.run(['flatpak', 'info', '--system', runtime], capture_output=True, text=True)
if available.returncode:
    print('Flatpak runtime missing: install ' + runtime + ' through the normal Flatpak flow', file=sys.stderr)
    raise SystemExit(77)
base.mkdir(mode=0o700, parents=True, exist_ok=True)
canary = base / 'outside-app-canary'
canary.write_text('Owned permission fixture\n')
if not identity.is_file():
    app_id = 'org.portal.CompatibilityProbe.x' + uuid.uuid4().hex
    identity.write_text(app_id)
else:
    app_id = identity.read_text().strip()
if not re.fullmatch(r'org\.portal\.CompatibilityProbe\.x[0-9a-f]{32}', app_id):
    raise RuntimeError('Invalid owned Flatpak fixture identity')
installed = subprocess.run(['flatpak', 'info', scope, app_id], capture_output=True)
if installed.returncode:
    app = base / 'app'
    if app.exists():
        shutil.rmtree(app)
    files = app / 'files/bin'
    files.mkdir(parents=True)
    (app / 'metadata').write_text(f'[Application]\nname={app_id}\n'
                                 f'runtime={runtime}\nsdk=org.freedesktop.Sdk/aarch64/{args.branch}\n'
                                 'command=sdl-probe\n')
    subprocess.run(['cc', '-O2', str(Path(__file__).with_name('flatpak_sdl_probe.c')),
                    '-o', str(files / 'sdl-probe'), '-l:libSDL2-2.0.so.0', '-l:libvulkan.so.1'], check=True)
    desktop = files.parent / 'share/applications'
    desktop.mkdir(parents=True)
    (desktop / (app_id + '.desktop')).write_text('[Desktop Entry]\nType=Application\n'
                                               'Name=Portal compatibility fixture\nExec=sdl-probe\nNoDisplay=true\n')
    subprocess.run(['flatpak', 'build-finish', '--command=sdl-probe', '--share=ipc',
                    '--socket=wayland', '--socket=fallback-x11', '--device=dri', str(app)], check=True)
    repository = base / 'repo'
    subprocess.run(['flatpak', 'build-export', '--no-update-summary', str(repository), str(app), 'compat'], check=True)
    bundle = base / 'fixture.flatpak'
    bundle.unlink(missing_ok=True)
    subprocess.run(['flatpak', 'build-bundle', str(repository), str(bundle), app_id, 'compat'], check=True)
    subprocess.run([*administrator, 'flatpak', 'install', scope, '--noninteractive', '-y', '--no-deps', '--no-related',
                    '--bundle', str(bundle)], check=True)
command = ['flatpak', 'run', scope, '--branch=compat',
           '--env=WAYLAND_DEBUG=' + os.environ.get('WAYLAND_DEBUG', '')]
if os.environ.get('PORTAL_COMPAT_FLATPAK_BASELINE') == '1':
    command += ['--unset-env=' + name for name in ('FD_FORCE_KGSL', 'FD_KGSL_ENABLE_DMABUF', 'TURNIP_KMD')]
print(json.dumps(dict(flatpak_runtime=runtime, flatpak_fixture=app_id, installation=args.installation)), flush=True)
result = subprocess.run([*command, app_id, str(args.seconds), str(canary)], timeout=args.seconds + 45)
raise SystemExit(result.returncode)
