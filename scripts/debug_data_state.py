#!/usr/bin/env python3
"""Temporarily isolate Portal Debug data for a clean first-run test.

Renames retain the original inodes, permissions and guest hardlink store.
The original and fresh states are kept inside Debug's private directory.
Android permission grants are not reset. No data is deleted.
"""
import argparse
import fcntl
import hashlib
import json
from pathlib import Path
import shlex
import subprocess
import tempfile
import time
import uuid

PACKAGE = 'app.polarbear'
DATA_NAMES = {'files', 'shared_prefs', 'cache', 'code_cache', 'databases',
              'no_backup', 'app_webview', 'app_textures'}


def same(actual, expected):
    return actual is not None and actual == expected


def validate(state):
    if state.get('version') != 1 or state.get('package') != PACKAGE:
        raise RuntimeError('Unsupported Debug data record')
    store = state['store']
    if not store.startswith('.portal-e2e-') or len(store) != len('.portal-e2e-') + 32:
        raise RuntimeError('Invalid private state directory')
    if any(c not in '0123456789abcdef' for c in store[len('.portal-e2e-'):]):
        raise RuntimeError('Invalid private state token')
    if not set(state['original']).issubset(DATA_NAMES) or 'files' not in state['original']:
        raise RuntimeError('Unexpected application data paths')
    if not set(state.get('fresh', {})).issubset(DATA_NAMES):
        raise RuntimeError('Unexpected fresh application paths')


def save(fs, state, record):
    validate(state)
    text = json.dumps(state, indent=2) + '\n'
    fs.write(state['store'] + '/journal.json', text)
    temporary = record.with_suffix('.tmp')
    temporary.write_text(text)
    temporary.chmod(0o600)
    temporary.replace(record)


def prepare(fs, serial, record):
    if record.exists():
        raise RuntimeError('A state record already exists; resume or restore it')
    names = fs.entries()
    if not set(names).issubset(DATA_NAMES) or 'files' not in names:
        raise RuntimeError('Unexpected private directories; no data was moved')
    state = dict(version=1, package=PACKAGE, serial=serial,
                 store='.portal-e2e-' + uuid.uuid4().hex, phase='prepared',
                 original={name: fs.info(name) for name in names})
    fs.mkdir(state['store'] + '/original')
    fs.mkdir(state['store'] + '/fresh')
    save(fs, state, record)
    return state


def isolate(fs, state, record):
    validate(state)
    if state['phase'] == 'isolated':
        return
    if state['phase'] not in ('prepared', 'isolating'):
        raise RuntimeError('This record cannot start another isolation')
    state['phase'] = 'isolating'
    save(fs, state, record)
    for name, identity in state['original'].items():
        destination = state['store'] + '/original/' + name
        source, saved = fs.info(name), fs.info(destination)
        if same(source, identity) and saved is None:
            fs.rename(name, destination)
        elif source is None and same(saved, identity):
            continue
        else:
            raise RuntimeError('Data changed during isolation: ' + name)
    state['phase'] = 'isolated'
    save(fs, state, record)


def restore(fs, state, record):
    validate(state)
    if state['phase'] == 'restored':
        for name, identity in state['original'].items():
            if not same(fs.info(name), identity):
                raise RuntimeError('Restored data identity changed: ' + name)
        return
    if state['phase'] not in ('prepared', 'isolating', 'isolated', 'restoring'):
        raise RuntimeError('Unsupported restore phase')
    if state['phase'] != 'restoring':
        names = set(fs.entries()) - {state['store']}
        if not names.issubset(DATA_NAMES):
            raise RuntimeError('Unexpected private paths; data preserved')
        state['fresh'] = {name: fs.info(name) for name in sorted(names)
                          if not same(fs.info(name), state['original'].get(name))}
        state['phase'] = 'restoring'
        save(fs, state, record)
    active_names = set(fs.entries()) - {state['store']}
    if not active_names.issubset(set(state['original']) | set(state['fresh'])):
        raise RuntimeError('Application data paths changed during restore')
    for name, identity in state['fresh'].items():
        destination = state['store'] + '/fresh/' + name
        source, saved = fs.info(name), fs.info(destination)
        if same(source, identity) and saved is None:
            fs.rename(name, destination)
        elif same(saved, identity) and (source is None or same(source, state['original'].get(name))):
            continue
        else:
            raise RuntimeError('Fresh data changed during restore: ' + name)
    for name, identity in state['original'].items():
        source = state['store'] + '/original/' + name
        saved, active = fs.info(source), fs.info(name)
        if same(saved, identity) and active is None:
            fs.rename(source, name)
        elif saved is None and same(active, identity):
            continue
        else:
            raise RuntimeError('Original data changed during restore: ' + name)
    state['phase'] = 'restored'
    save(fs, state, record)


class AndroidFiles:
    def __init__(self, serial):
        self.serial = serial
        details = self.adb('shell', 'dumpsys package ' + PACKAGE).stdout
        if 'DEBUGGABLE' not in details:
            raise RuntimeError('Portal Debug is not installed and debuggable')
        root = self.run('pwd').stdout.strip()
        if root not in ('/data/user/0/' + PACKAGE, '/data/data/' + PACKAGE):
            raise RuntimeError('Unexpected Android profile or private data root')
        self.uid = int(self.run('id -u').stdout)

    def adb(self, *args, **kwargs):
        return subprocess.run(['adb', '-s', self.serial, *args],
                              capture_output=True, text=True, timeout=30, **kwargs)

    def run(self, script, check=True):
        return self.adb('shell', '-T', shlex.join(['run-as', PACKAGE, 'sh', '-c', script]), check=check)

    def stop(self):
        self.adb('shell', 'am force-stop ' + PACKAGE, check=True)
        for _ in range(20):
            processes = self.adb('shell', 'ps -A -o UID,PID,NAME', check=True).stdout
            identities = {str(self.uid), 'u0_a' + str(self.uid - 10000)}
            remaining = [line for line in processes.splitlines() if line.split() and line.split()[0] in identities]
            if not remaining:
                return
            time.sleep(0.25)
        raise RuntimeError('Debug process did not stop; data preserved')

    def entries(self):
        return self.run('ls -1A').stdout.splitlines()

    def info(self, path):
        result = self.run('stat -c ' + shlex.quote('%d %i %f %u %g') + ' ' + shlex.quote(path), check=False)
        if result.returncode:
            # Distinguish an absent path from other stat failures.
            absent = self.run('test ! -e ' + shlex.quote(path) + ' && test ! -L ' + shlex.quote(path), check=False)
            if absent.returncode == 0:
                return None
            raise RuntimeError('Could not inspect private data: ' + path)
        device, inode, mode, uid, gid = result.stdout.split()
        return dict(device=int(device), inode=int(inode), mode=int(mode, 16), uid=int(uid), gid=int(gid))

    def mkdir(self, path):
        self.run('mkdir -p -m 700 ' + shlex.quote(path))

    def rename(self, source, destination):
        self.run('test ! -e ' + shlex.quote(destination) + ' && test ! -L ' + shlex.quote(destination)
                 + ' && mv ' + shlex.quote(source) + ' ' + shlex.quote(destination))

    def write(self, path, text):
        temporary = path + '.new'
        self.run('umask 077; printf %s ' + shlex.quote(text) + ' >' + shlex.quote(temporary)
                 + ' && mv -f ' + shlex.quote(temporary) + ' ' + shlex.quote(path))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--record', type=Path, required=True)
    parser.add_argument('action', choices=['isolate', 'restore', 'status'])
    args = parser.parse_args()
    args.record.parent.mkdir(parents=True, exist_ok=True)
    lock_name = hashlib.sha256(args.serial.encode()).hexdigest()[:20]
    with open(Path(tempfile.gettempdir()) / ('portal-app-compat-' + lock_name + '.lock'), 'a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        fs = AndroidFiles(args.serial)
        state = json.loads(args.record.read_text()) if args.record.exists() else None
        if state:
            validate(state)
            if state['serial'] != args.serial:
                raise RuntimeError('Record belongs to another device')
        if args.action != 'status':
            fs.stop()
            if state is None:
                if args.action != 'isolate':
                    raise RuntimeError('No original state record; nothing moved')
                state = prepare(fs, args.serial, args.record)
            (isolate if args.action == 'isolate' else restore)(fs, state, args.record)
        print(json.dumps(state, indent=2))


if __name__ == '__main__':
    main()
