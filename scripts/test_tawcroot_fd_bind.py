#!/usr/bin/env python3
"""Regression: peer-fd directory mounts survive exec and retain guest names."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile


def run(engine):
    if os.getuid() == 0:
        raise RuntimeError('Run as an unprivileged user: root bypasses the permission regression')
    with tempfile.TemporaryDirectory(prefix='portal-fd-bind-') as temporary:
        base = Path(temporary)
        parent = base / 'hidden'
        source = parent / 'source'
        source.mkdir(parents=True)
        root = base / 'root'
        root.mkdir()
        (source / 'value').write_text('peer-fd-survived\n')
        fd = os.open(source, os.O_PATH)
        parent.chmod(0)
        try:
            try:
                os.open(source, os.O_PATH)
            except PermissionError:
                pass
            else:
                raise AssertionError('Source pathname must be inaccessible for this test')
            command = [str(Path(engine).resolve()), '-r', str(root)]
            for path in ('/usr', '/bin', '/lib', '/lib64', '/proc', '/dev'):
                if Path(path).exists():
                    command.extend(['-b', f'{path}:{path}'])
            command += ['-b', f'/proc/{os.getpid()}/fd/{fd}:/portal-peer-fd', '--',
                       '/bin/sh', '-c',
                       "exec /bin/sh -c 'cat /portal-peer-fd/value; cd /portal-peer-fd && pwd'"]
            result = subprocess.run(command, capture_output=True, text=True, timeout=15)
            assert result.returncode == 0, (result.returncode, result.stderr)
            assert result.stdout == 'peer-fd-survived\n/portal-peer-fd\n', result.stdout
            print('PASS: peer fd survives two execs and reverse-translates to its guest mount')
        finally:
            parent.chmod(0o700)
            os.close(fd)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('engine')
    run(parser.parse_args().engine)
