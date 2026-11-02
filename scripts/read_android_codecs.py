#!/usr/bin/env python3
"""Query Android's public codec inventory without creating decoder instances."""
import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import uuid
import zipfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--serial', required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
sdk = Path(os.environ.get('ANDROID_HOME', str(Path.home() / 'Android/Sdk')))
platform = sdk / 'platforms/android-35/android.jar'
versions = sorted((sdk / 'build-tools').iterdir(), key=lambda p: tuple(int(x) if x.isdigit() else 0 for x in p.name.split('.')))
d8 = versions[-1] / 'd8'
javac = shutil.which('javac')
if not javac and shutil.which('java'):
    candidate = Path(shutil.which('java')).resolve().with_name('javac')
    if candidate.is_file():
        javac = str(candidate)
if not javac:
    raise RuntimeError('A JDK with javac is required to query Android codec flags')
args.output.parent.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(prefix='codec-inventory-', dir=args.output.parent) as tmp:
    tmp = Path(tmp)
    subprocess.run([javac, '--release', '8', '-cp', str(platform), '-d', str(tmp),
                    str(root / 'tests/compat/CodecInventory.java')], check=True)
    subprocess.run([str(d8), '--lib', str(platform), '--output', str(tmp), str(tmp / 'CodecInventory.class')], check=True)
    jar = tmp / 'inventory.jar'
    with zipfile.ZipFile(jar, 'w') as archive:
        archive.write(tmp / 'classes.dex', 'classes.dex')
    remote = '/data/local/tmp/portal-codecs-' + uuid.uuid4().hex + '.jar'
    adb = ['adb', '-s', args.serial]
    subprocess.run([*adb, 'push', str(jar), remote], check=True, capture_output=True)
    try:
        command = shlex.join(['env', 'CLASSPATH=' + remote, 'app_process', '/system/bin', 'CodecInventory'])
        result = subprocess.run([*adb, 'shell', command], capture_output=True, text=True, timeout=30, check=True)
        record = next(json.loads(line) for line in result.stdout.splitlines() if line.startswith('{'))
        args.output.write_text(json.dumps(record, indent=2) + '\n')
        print(json.dumps(record, indent=2))
    finally:
        subprocess.run([*adb, 'shell', 'rm -f ' + shlex.quote(remote)], capture_output=True, timeout=10)
