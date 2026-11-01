#!/usr/bin/python3
"""Require a receipt from inside the Windows process, not its launcher."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('launcher')
parser.add_argument('fixture')
args = parser.parse_args()
with tempfile.TemporaryDirectory(prefix='portal-windows-probe-') as tmp:
    receipt = Path(tmp) / 'receipt.json'
    env = dict(os.environ, PORTAL_COMPAT_RESULT='Z:' + str(receipt).replace('/', '\\'))
    result = subprocess.run(['python3', args.launcher, args.fixture], env=env,
                            capture_output=True, text=True, timeout=160)
    print(result.stdout, end='', file=sys.stderr)
    print(result.stderr, end='', file=sys.stderr)
    deadline = time.monotonic() + 5
    while not receipt.is_file() and time.monotonic() < deadline:
        time.sleep(0.1)
    passed = False
    if receipt.is_file():
        observation = json.loads(receipt.read_text())
        passed = observation.get('windows_execution') is True
        print(json.dumps(observation), flush=True)
    if not passed:
        print('The Windows fixture did not produce its execution receipt', file=sys.stderr)
    raise SystemExit(0 if passed and result.returncode == 0 else 1)
