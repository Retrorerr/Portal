#!/usr/bin/python3
"""Play an owned clip with VLC's default decoder/output and report real counters."""
import argparse
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('clip', type=Path)
parser.add_argument('--seconds', type=float, default=4)
args = parser.parse_args()
metadata = json.loads(subprocess.check_output(['ffprobe', '-v', 'error', '-show_entries',
                                              'format=duration', '-of', 'json', str(args.clip)], timeout=10))
if float(metadata['format']['duration']) < args.seconds + 1:
    print('Playback fixture too short; encode a clip longer than the requested flow', file=sys.stderr)
    raise SystemExit(77)


def statistics(connection):
    connection.sendall(b'stats\n')
    data, deadline = b'', time.monotonic() + 1
    while time.monotonic() < deadline:
        try:
            chunk = connection.recv(65536)
        except socket.timeout:
            continue
        if not chunk:
            break
        data += chunk
        if b'end of statistical info' in data:
            break
    text = data.decode(errors='replace')
    result = {}
    for label, key in [('video decoded', 'decoded'), ('frames displayed', 'displayed'), ('frames lost', 'lost')]:
        values = re.findall(re.escape(label) + r'\s*:\s*(\d+)', text)
        if values:
            result[key] = int(values[-1])
    return result


with tempfile.TemporaryDirectory(prefix='pv-') as tmp, tempfile.TemporaryFile(mode='w+') as log:
    path = Path(tmp) / 'rc'
    process = subprocess.Popen(['vlc', '--no-one-instance', '--ignore-config', '--no-media-library',
                                '--no-qt-privacy-ask', '--no-video-title-show', '--verbose=2',
                                '--extraintf=oldrc', '--rc-fake-tty', '--rc-unix=' + str(path), '--play-and-pause',
                                str(args.clip)], stdout=log, stderr=log, env=dict(os.environ, LC_ALL='C'))
    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    connection.settimeout(0.2)
    started, start_ns, initial_displayed, final = None, None, None, {}
    try:
        deadline = time.monotonic() + 20
        while not path.exists() and process.poll() is None and time.monotonic() < deadline:
            time.sleep(0.1)
        connection.connect(str(path))
        while process.poll() is None and time.monotonic() < deadline:
            final = statistics(connection)
            if final.get('displayed', 0) >= 5:
                if started is None:
                    started, start_ns = time.monotonic(), time.monotonic_ns()
                    initial_displayed = final['displayed']
                if time.monotonic() - started >= args.seconds:
                    break
            time.sleep(0.1)
        end_ns = time.monotonic_ns()
        messages = os.pread(log.fileno(), os.fstat(log.fileno()).st_size, 0).decode(errors='replace')
        selected = [line.strip() for line in messages.splitlines()
                    if re.search(r'Using .*hardware decoding|using hw decoder module', line, re.I)]
        hardware = any(re.search(r'vaapi|termux|mediacodec', line, re.I) for line in selected)
        print(json.dumps(dict(application='VLC', clip=args.clip.name, decoder_selection=selected,
                              hardware_decoding_selected=hardware, decoder_selection_forced=False,
                              frames=final, flow_start_ns=start_ns, flow_end_ns=end_ns)), flush=True)
        if (started is None or time.monotonic() - started < args.seconds or
                final.get('displayed', 0) - initial_displayed < args.seconds * 15):
            raise RuntimeError('VLC did not complete a displayed-video flow')
    finally:
        connection.close()
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)
        log.seek(0)
        print(log.read(), file=sys.stderr)
