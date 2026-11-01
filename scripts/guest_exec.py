#!/usr/bin/env python3
"""Execute in Portal Debug's live desktop with its recorded engine and mounts.

Requires an updated Debug build and a live desktop. Never switches engines:
tawcroot hardlinks cannot safely be opened through PRoot.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import time
import uuid
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

    def run_desktop(self, command, *, timeout=60, environment=None, capture_output=False,
                    text=False, check=False):
        """Launch through the real session, including its Android SELinux domain."""
        token = uuid.uuid4().hex
        name = 'org.portal.CompatibilityExec.x' + token
        directory = '/tmp/portal-desktop-exec-' + token
        env = self.environment | (environment or {})
        data_home = env.get('XDG_DATA_HOME') or env['HOME'] + '/.local/share'
        service = data_home + '/dbus-1/services/' + name + '.service'
        service_text = f'[D-BUS Service]\nName={name}\nExec=/usr/bin/python3 -I {directory}/broker.py {name} {directory}\n'
        payload = dict(directory=directory, service=service, service_text=service_text,
                       broker=Path(__file__).with_name('desktop_exec_broker.py').read_text(),
                       request=dict(command=command, environment=env, timeout=timeout))
        setup = """import json,os,sys
from pathlib import Path
os.umask(0o077)
p=json.load(sys.stdin)
d=Path(p['directory']);d.mkdir(mode=0o700)
(d/'broker.py').write_text(p['broker'])
(d/'request.json').write_text(json.dumps(p['request']))
s=Path(p['service']);s.parent.mkdir(parents=True,exist_ok=True)
with s.open('x') as f:f.write(p['service_text'])
"""
        self.run('python3 -c ' + shlex.quote(setup), input=json.dumps(payload).encode(),
                 capture_output=True, check=True)
        def read(file):
            result = self.adb('shell', '-T', shlex.join(['run-as', PACKAGE, 'cat',
                              self.record['rootfs'] + directory + '/' + file]),
                              capture_output=True, timeout=10)
            return result.stdout if result.returncode == 0 else None
        status = None
        try:
            activate = ("import dbus; b=dbus.SessionBus(); b.call_blocking('org.freedesktop.DBus', "
                        "'/org/freedesktop/DBus', 'org.freedesktop.DBus', 'StartServiceByName', "
                        "'su', [" + repr(name) + ", 0], timeout=20)")
            activated = self.run('python3 -c ' + shlex.quote(activate), capture_output=True, text=True, timeout=25)
            if activated.returncode:
                raise RuntimeError('Desktop session activation failed: ' + activated.stderr.strip())
            deadline = time.monotonic() + timeout + 15 if timeout is not None else None
            while True:
                data = read('status.json')
                if data is not None:
                    status = json.loads(data)
                    break
                if deadline and time.monotonic() >= deadline:
                    raise subprocess.TimeoutExpired(command, timeout)
                time.sleep(0.1)
            stdout, stderr = read('stdout') or b'', read('stderr') or b''
            if status.get('error'):
                stderr += status['error'].encode() + b'\n'
            if text:
                stdout, stderr = stdout.decode(errors='replace'), stderr.decode(errors='replace')
            result = subprocess.CompletedProcess(command, status['returncode'], stdout, stderr)
            context = status.get('selinux_context', '').split(':')
            result.portal_launch_context = context[2] if len(context) > 2 else 'unavailable'
            if not capture_output:
                sys.stdout.buffer.write(stdout.encode() if text else stdout)
                sys.stderr.buffer.write(stderr.encode() if text else stderr)
                result.stdout = result.stderr = None
            if check:
                result.check_returncode()
            return result
        finally:
            cleanup = """import json,shutil,sys
from pathlib import Path
p=json.load(sys.stdin);d=Path(p['directory']);s=Path(p['service'])
if not (d/'status.json').exists():
 (d/'cancel').touch()
else:
 shutil.rmtree(d)
if s.is_file() and s.read_text()==p['service_text']:s.unlink()
"""
            self.run('python3 -c ' + shlex.quote(cleanup), input=json.dumps(payload).encode(),
                     capture_output=True, timeout=15)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial')
    parser.add_argument('--package', default=PACKAGE)
    parser.add_argument('--root', action='store_true', help='Explicit maintenance command; default is desktop')
    parser.add_argument('--adb-context', action='store_true', help='Run directly from run-as for engine maintenance instead of native desktop activation')
    parser.add_argument('--timeout', type=float, default=60)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        parser.error('supply a shell command')
    try:
        session = Session.connect(args.serial, args.package)
        if args.root or args.adb_context:
            return session.run(' '.join(command), root=args.root, timeout=args.timeout).returncode
        return session.run_desktop(' '.join(command), timeout=args.timeout).returncode
    except (RuntimeError, subprocess.SubprocessError) as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
