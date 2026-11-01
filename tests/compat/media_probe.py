#!/usr/bin/python3
"""Require real VA hardware surfaces for every decoded test frame."""
import argparse
import json
import math
from pathlib import Path
import re
import subprocess
import sys
import time
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument('clip', type=Path)
parser.add_argument('--frames', type=int, default=60)
args = parser.parse_args()
command = ['ffmpeg', '-nostdin', '-hide_banner', '-v', 'info', '-benchmark',
           '-hwaccel', 'vaapi', '-hwaccel_device', '/dev/dri/renderD128',
           '-hwaccel_output_format', 'vaapi', '-i', str(args.clip), '-map', '0:v:0',
           '-an', '-vf', 'hwdownload,format=nv12', '-frames:v', str(args.frames),
           '-fps_mode', 'passthrough', '-f', 'rawvideo']
with tempfile.TemporaryDirectory(prefix='portal-decode-probe-') as tmp:
    hardware, software = Path(tmp) / 'hardware.nv12', Path(tmp) / 'software.nv12'
    start = time.monotonic()
    result = subprocess.run([*command, str(hardware), '-progress', 'pipe:1', '-nostats'],
                            capture_output=True, text=True, timeout=120)
    elapsed = time.monotonic() - start
    print(result.stderr, file=sys.stderr)
    reference = subprocess.run(['ffmpeg', '-nostdin', '-hide_banner', '-v', 'error',
                                '-hwaccel', 'none', '-i', str(args.clip), '-map', '0:v:0', '-an',
                                '-frames:v', str(args.frames), '-pix_fmt', 'nv12', '-fps_mode', 'passthrough',
                                '-f', 'rawvideo', str(software)], capture_output=True, text=True, timeout=120)
    print(reference.stderr, file=sys.stderr)
    actual = hardware.read_bytes() if hardware.exists() else b''
    expected = software.read_bytes() if software.exists() else b''
    psnr, max_error = None, None
    if reference.returncode == 0 and actual and len(actual) == len(expected):
        mse = sum((a-b)**2 for a, b in zip(actual, expected)) / len(actual)
        # JSON has no infinity; exact is represented separately.
        psnr = 10 * math.log10(255**2 / mse) if mse else None
        max_error = max(abs(a-b) for a, b in zip(actual, expected))
frames = [int(x) for x in re.findall(r'^frame=(\d+)\s*$', result.stdout, re.M)]
count = frames[-1] if frames else 0
# hwdownload cannot accept software frames; a successful completed decode
# verifies VA surfaces instead of inferring acceleration from an installed driver.
decoded = result.returncode == 0 and count == args.frames and 'progress=end' in result.stdout
quality = max_error is not None and (max_error == 0 or psnr >= 40)
passed = decoded and quality
resources = re.search(r'utime=([\d.]+)s stime=([\d.]+)s rtime=([\d.]+)s', result.stderr)
print(json.dumps(dict(media=args.clip.name, hardware_surface_download=decoded, frames=count,
                      software_reference_matches=quality, exact_pixels=max_error == 0,
                      psnr_db=psnr, maximum_pixel_error=max_error,
                      expected_frames=args.frames, elapsed=elapsed,
                      cpu_seconds=float(resources[1])+float(resources[2]) if resources else None)), flush=True)
raise SystemExit(0 if passed else 1)
