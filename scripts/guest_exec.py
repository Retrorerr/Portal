#!/usr/bin/env python3
"""Execute in Portal Debug's live desktop with its recorded engine and mounts.

Requires an updated Debug build and a live desktop. Never switches engines:
tawcroot hardlinks cannot safely be opened through PRoot.
"""
import argparse
import json
import os
import shlex
import subprocess
import sys
from dataclasses import dataclass

PACKAGE = 'app.polarbear'


def get_device(serial=None):
    serial = serial or os.environ.get('ANDROID_SERIAL')
    rows = subprocess.check_output(['adb', 'devices'], text=True).splitlines()[1:]
    devices = [r.split()[0] for r in rows if len(r.split()) >= 2 and r.split()[1] == 'device']
    if serial in devices:
        return serial
    if not serial and len(devices) == 1:
        return devices[0]
    raise RuntimeError('Select one connected, authorized ADB device with --serial or ANDROID_SERIAL')


def find_process_pid(ps_output, process_name):
    rows = [line.split() for line in ps_output.splitlines()]
    if not rows or 'PID' not in rows[0] or 'NAME' not in rows[0]:
        return None
    pid_column, name_column = rows[0].index('PID'), rows[0].index('NAME')
    for fields in rows[1:]:
        if len(fields) > max(pid_column, name_column) and fields[name_column] == process_name:
            return fields[pid_column]
    return None


def parse_environment(data):
    return dict(item.decode('utf-8', 'surrogateescape').split('=', 1)
                for item in data.split(b'\0') if b'=' in item)


def command_arguments(record, environment, command, root=False):
    if record.get('version') != 1 or record.get('engine') not in ('tawcroot', 'proot'):
        raise RuntimeError('Unsupported Debug guest launch record; rebuild Portal Debug')
    env = dict(environment)
    if root:
        env.update(HOME='/root', USER='root', LOGNAME='root')
    elif env.get('USER') != record['user'] or env.get('HOME') == '/root':
        raise RuntimeError('Desktop identity differs from the recorded session')
    home = env['HOME']
    for key in list(env):
        if key in ('_', 'SHLVL', 'PWD', 'OLDPWD') or key.startswith(('PROOT_', 'TAWCROOT_')):
            env.pop(key)
    script = 'cd ' + shlex.quote(home) + ' && ' + command
    args = [record['program'], *record['args'], '/usr/bin/env', '-i']
    args.extend(f'{key}={value}' for key, value in sorted(env.items()))
    if not root:
        args.extend(['/usr/sbin/runuser', '-u', record['user'], '--'])
    args.extend(['/bin/sh', '-c', script])
    # Android's APK paths contain '='. env(1) misreads an engine path as
    # another variable assignment even when quoted; exec it through sh.
    exports = ''.join('export ' + shlex.quote(f'{k}={v}') + '; ' for k, v in record['env'].items())
    return ['/system/bin/sh', '-c', exports + 'exec ' + shlex.join(args)]


@dataclass
class Session:
    serial: str
    record: dict
    environment: dict
    desktop_pid: str

    def adb(self, *args, **kwargs):
        return subprocess.run(['adb', '-s', self.serial, *args], **kwargs)

    @classmethod
    def connect(cls, serial=None, package=PACKAGE):
        if package != PACKAGE:
            raise RuntimeError('Compatibility tooling only operates on Portal Debug (app.polarbear)')
        serial = get_device(serial)
        def read(path):
            return subprocess.check_output(['adb', '-s', serial, 'exec-out', 'run-as', PACKAGE, 'cat', path])
        try:
            record = json.loads(read('files/diagnostics/guest-launch.json'))
        except (subprocess.CalledProcessError, ValueError) as error:
            raise RuntimeError('Launch an updated Portal Debug once to create its guest launch record') from error
        # Early Debug snapshots serialized OsStr using serde's Unix bytes.
        if isinstance(record.get('program'), dict) and 'Unix' in record['program']:
            record['program'] = os.fsdecode(bytes(record['program']['Unix']))
        ps = subprocess.check_output(['adb', '-s', serial, 'shell', 'ps -A -o PID,PPID,NAME'], text=True)
        table = {row[0]: row for line in ps.splitlines()[1:] if len(row := line.split()) == 3}
        pid = None
        # Stable may also have plasmashell. Accept only descendants of Debug.
        for candidate, row in table.items():
            if row[2] != 'plasmashell':
                continue
            parent, visited = candidate, set()
            while parent in table and parent not in visited:
                if parent == str(record['host_pid']):
                    pid = candidate
                    break
                visited.add(parent)
                parent = table[parent][1]
        if not pid:
            raise RuntimeError('No live Portal Debug desktop for this launch record; open its desktop first')
        env = parse_environment(read(f'/proc/{pid}/environ'))
        # run-as lacks the app's external-storage mount namespace. Borrow
        # the app's already-open O_PATH descriptor rather than dropping its
        # storage bind or changing its permissions. The engine opens this
        # descriptor itself; no fd number is assumed across engine versions.
        links = subprocess.check_output(['adb', '-s', serial, 'shell', shlex.join([
            'run-as', PACKAGE, 'sh', '-c',
            f'for fd in /proc/{pid}/fd/*; do printf "%s " "$fd"; readlink "$fd"; done'
        ])], text=True)
        storage_fd = next((line.split(' ', 1)[0] for line in links.splitlines()
                           if line.endswith(' /storage/emulated/0')), None)
        if storage_fd:
            record['args'] = [storage_fd + arg[len('/sdcard'):] if arg.startswith('/sdcard:') else arg
                              for arg in record['args']]
        command_arguments(record, env, 'true')
        return cls(serial, record, env, pid)

    def run(self, command, *, root=False, timeout=60, environment=None, **kwargs):
        args = command_arguments(self.record, self.environment | (environment or {}), command, root)
        # Exactly one remote shell parse, with literal environment values.
        remote = shlex.join(['run-as', PACKAGE, *args])
        return self.adb('shell', '-T', remote, timeout=timeout, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial')
    parser.add_argument('--package', default=PACKAGE)
    parser.add_argument('--root', action='store_true', help='Explicit maintenance command; default is desktop')
    parser.add_argument('--timeout', type=float, default=60)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        parser.error('supply a shell command')
    try:
        return Session.connect(args.serial, args.package).run(
            ' '.join(command), root=args.root, timeout=args.timeout).returncode
    except (RuntimeError, subprocess.SubprocessError) as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
