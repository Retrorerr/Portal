#!/usr/bin/python3
"""Exercise Vulkan swapchain rendering with the installed Vulkan Tools cube."""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('backend', choices=['wayland', 'xcb'])
parser.add_argument('--seconds', type=float, default=4)
args = parser.parse_args()
if not shutil.which('vkcube'):
    print('vkcube not found; install vulkan-tools', file=sys.stderr)
    raise SystemExit(77)
frames = round(args.seconds * 60)
with tempfile.TemporaryFile(mode='w+') as log:
    process = subprocess.Popen(['vkcube', '--wsi', args.backend, '--c', str(frames)], stdout=log, stderr=log)
    first_buffer = None
    try:
        deadline = time.monotonic() + args.seconds + 20
        while process.poll() is None:
            if time.monotonic() > deadline:
                raise TimeoutError('Vulkan swapchain rendering did not complete')
            # Exclude startup from Wayland pacing. XCB is a functional
            # swapchain test here; it does not report an active timing range.
            if args.backend == 'wayland' and first_buffer is None:
                text = os.pread(log.fileno(), os.fstat(log.fileno()).st_size, 0).decode(errors='replace')
                if re.search(r'wl_surface[@#]\d+\.attach\(wl_buffer', text):
                    first_buffer = time.monotonic_ns()
            time.sleep(0.05)
        end = time.monotonic_ns()
        log.seek(0)
        text = log.read()
        print(text, file=sys.stderr)
        selected = next((line.strip() for line in text.splitlines() if 'Selected GPU' in line), 'unreported')
        print(json.dumps(dict(toolkit='Vulkan Tools cube', backend=args.backend,
                              selected_device=selected, requested_frames=frames,
                              swapchain_completed=process.returncode == 0,
                              flow_start_ns=first_buffer, flow_end_ns=end)), flush=True)
        raise SystemExit(process.returncode)
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)
