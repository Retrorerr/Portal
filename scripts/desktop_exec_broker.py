#!/usr/bin/python3 -I
"""One-use Debug test command, activated by the live desktop session bus.

Android's run-as domain cannot delegate an AF_UNIX listener to the app's
domain. Session-bus activation inherits the actual desktop's engine and
SELinux context, including for Flatpak's security-context Wayland listener.
"""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


def run_job(directory, request):
    directory = Path(directory)
    result = dict(returncode=126, timed_out=False)
    try:
        result['selinux_context'] = Path('/proc/self/attr/current').read_text().strip('\0\n')
    except OSError:
        result['selinux_context'] = 'unavailable'
    with (directory / 'stdout').open('wb') as stdout, (directory / 'stderr').open('wb') as stderr:
        process = subprocess.Popen(['/bin/sh', '-c', request['command']],
                                   cwd=request['environment']['HOME'], env=request['environment'],
                                   stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                   start_new_session=True)
        result['pid'] = process.pid
        deadline = time.monotonic() + request['timeout'] if request['timeout'] is not None else None
        while process.poll() is None:
            canceled = (directory / 'cancel').exists()
            timed_out = deadline is not None and time.monotonic() >= deadline
            if not canceled and not timed_out:
                time.sleep(0.1)
                continue
            result.update(returncode=124, timed_out=timed_out, canceled=canceled)
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            # The shell may exit on TERM while a descendant ignores it.
            # Reap it, but check the entire owned group before ending cleanup.
            grace = time.monotonic() + 3
            while time.monotonic() < grace:
                process.poll()
                try:
                    os.killpg(process.pid, 0)
                except ProcessLookupError:
                    break
                time.sleep(0.1)
            else:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            process.wait(timeout=3)
            break
        else:
            result['returncode'] = process.returncode
    return result


def main():
    import dbus
    import dbus.service
    name, directory = sys.argv[1], Path(sys.argv[2])
    bus = dbus.SessionBus()
    owner = dbus.service.BusName(name, bus=bus, do_not_queue=True)
    # A leftover activation file must never replay a completed command.
    try:
        (directory / 'started').open('x').close()
    except FileExistsError:
        return 0
    try:
        request = json.loads((directory / 'request.json').read_text())
        result = run_job(directory, request)
    except Exception as error:
        result = dict(returncode=126, error=str(error))
    temporary = directory / 'status.tmp'
    temporary.write_text(json.dumps(result))
    temporary.replace(directory / 'status.json')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
