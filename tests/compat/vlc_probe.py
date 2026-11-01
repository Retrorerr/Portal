#!/usr/bin/python3
"""Verify VLC's dynamically loaded Qt interface on Wayland or Xwayland."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('--seconds', type=float, default=4)
args = parser.parse_args()


def x11_window(pid):
    if not all(shutil.which(tool) for tool in ('xprop', 'xwininfo')):
        return None
    result = subprocess.run(['xprop', '-root', '_NET_CLIENT_LIST'], capture_output=True, text=True, timeout=5)
    for window in re.findall(r'0x[0-9a-fA-F]+', result.stdout):
        properties = subprocess.run(['xprop', '-id', window, '_NET_WM_PID'],
                                     capture_output=True, text=True, timeout=5).stdout
        if re.search(r'=\s*' + str(pid) + r'\s*$', properties):
            info = subprocess.run(['xwininfo', '-id', window], capture_output=True, text=True, timeout=5)
            if 'Map State: IsViewable' in info.stdout:
                return window
    return None


with tempfile.TemporaryFile(mode='w+') as log:
    process = subprocess.Popen(['vlc', '--no-one-instance', '--ignore-config',
                                '--no-media-library', '--no-qt-privacy-ask'], stdout=log, stderr=log)
    backend, libraries, platform_plugin = None, [], None
    try:
        deadline = time.monotonic() + args.seconds + 5
        while time.monotonic() < deadline and process.poll() is None:
            try:
                mappings = Path(f'/proc/{process.pid}/maps').read_text()
            except OSError:
                break
            libraries = sorted(set(re.findall(r'libQt[56](?:Core|Gui|Widgets)\.so\.[^/\s]+', mappings)))
            plugin = re.search(r'/platforms/(libq[^/\s]+\.so)', mappings)
            if plugin:
                platform_plugin = plugin[1]
            if libraries:
                if x11_window(process.pid):
                    backend = 'x11'
                events = os.pread(log.fileno(), os.fstat(log.fileno()).st_size, 0).decode(errors='replace')
                if re.search(r'wl_surface[@#]\d+\.attach\(wl_buffer', events):
                    backend = 'wayland'
                if backend:
                    break
            time.sleep(0.25)
        print(json.dumps(dict(application='VLC', qt_interface_window=bool(backend),
                              backend=backend, qt_libraries=libraries, platform_plugin=platform_plugin)), flush=True)
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        log.seek(0)
        print(log.read(), file=sys.stderr)
raise SystemExit(0 if backend else 1)
