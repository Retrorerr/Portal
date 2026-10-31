#!/usr/bin/env python3
"""Repeatable physical-device app compatibility checks for Portal Debug.

Reports distinguish tested GUI probes, launch-only checks and unavailable
runtimes. SurfaceFlinger measures the desktop SurfaceView, not an individual
client: guest Wayland commits and probe counters are recorded separately.
"""
import argparse
import concurrent.futures
import fcntl
import hashlib
import tempfile
import json
from pathlib import Path
import re
import shlex
import statistics
import subprocess
import time
import uuid
from guest_exec import Session, PACKAGE

REPO = Path(__file__).resolve().parents[1]
SUITES = {
    'qt': ['qt5', 'qt6', 'quick5', 'quick6'],
    'gtk': ['gtk3', 'gtk4', 'tk', 'wx'],
    'graphics': ['glx', 'egl', 'vulkan'],
    'office': ['writer', 'calc', 'impress'],
    'apps': ['vlc', 'electron', 'flatpak', 'translated'],
}
PROBE_PACKAGES = ['python3-pyqt5', 'python3-pyqt5.qtquick', 'python3-pyqt6',
                  'python3-pyqt6.qtquick', 'python3-gi', 'gir1.2-gtk-3.0',
                  'gir1.2-gtk-4.0', 'python3-tk', 'python3-wxgtk4.0',
                  'mesa-utils', 'vulkan-tools', 'python3-uno']


def frame_stats(rows, period):
    presents = sorted({r[1] for r in rows if len(r) == 3 and 0 < r[1] < 9e18})
    intervals = [(b-a)/1e6 for a, b in zip(presents, presents[1:])]
    if not intervals:
        return dict(presented_frames=len(presents), status='insufficient_samples')
    ordered = sorted(intervals)
    return dict(presented_frames=len(presents), display_hz=1e9/period if period else None,
                median_interval_ms=statistics.median(intervals),
                p95_interval_ms=ordered[min(len(ordered)-1, int(len(ordered)*0.95))],
                max_interval_ms=max(intervals), gaps_over_100ms=sum(x > 100 for x in intervals))


def surface_layer(session):
    result = session.adb('shell', 'dumpsys SurfaceFlinger --list', capture_output=True, text=True)
    candidates = re.findall(
        r'[0-9a-f]+ SurfaceView\[app\.polarbear/app\.polarbear\.PortalActivity\]\(BLAST\)#[0-9]+', result.stdout)
    return candidates[-1] if candidates else None


def memory(session):
    result = session.adb('shell', 'dumpsys meminfo ' + PACKAGE, capture_output=True, text=True)
    match = re.search(r'TOTAL PSS:\s+(\d+)', result.stdout)
    return int(match[1]) if match else None


def cpu_snapshot(session):
    # Read real Android procfs, not the guest's virtual /proc/stat overlay.
    script = 'for p in /proc/[0-9]*/stat; do cat "$p" 2>/dev/null; done'
    result = session.adb('shell', shlex.join(['run-as', PACKAGE, 'sh', '-c', script]),
                         capture_output=True, text=True)
    ticks = {}
    for line in result.stdout.splitlines():
        match = re.match(r'(\d+) \(.*\) (.*)', line)
        if match:
            fields = match[2].split()
            if len(fields) > 12:
                ticks[match[1]] = int(fields[11]) + int(fields[12])
    return ticks


def capture(session, command, environment, timeout):
    layer = surface_layer(session)
    rows, period = set(), None
    if layer:
        session.adb('shell', 'dumpsys SurfaceFlinger --latency-clear ' + shlex.quote(layer),
                    capture_output=True)
    before_cpu, before_pss = cpu_snapshot(session), memory(session)
    started = time.monotonic()
    with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
        future = pool.submit(session.run, command, environment=environment, timeout=timeout,
                             capture_output=True, text=True)
        while not future.done():
            if layer:
                raw = session.adb('shell', 'dumpsys SurfaceFlinger --latency ' + shlex.quote(layer),
                                  capture_output=True, text=True).stdout.splitlines()
                if raw and raw[0].isdigit():
                    period = int(raw[0])
                    for line in raw[1:]:
                        fields = line.split()
                        if len(fields) == 3 and all(x.isdigit() for x in fields):
                            rows.add(tuple(map(int, fields)))
            time.sleep(0.25)
        result = future.result()
    duration = time.monotonic() - started
    after_cpu = cpu_snapshot(session)
    # Existing processes only. Exited probe CPU is also available through
    # /usr/bin/time in the per-run output; do not infer it from vanished PIDs.
    ticks = sum(max(0, after_cpu[p]-value) for p, value in before_cpu.items() if p in after_cpu)
    return result, dict(duration_seconds=duration, existing_process_cpu_ticks=ticks,
                        pss_before_kib=before_pss, pss_after_kib=memory(session),
                        desktop_presentation=frame_stats(rows, period),
                        wayland_commits=len(re.findall(r'wl_surface[@#]\d+\.commit\(', result.stderr)))


def push(session, local, destination):
    remote = '/data/local/tmp/portal-compat-' + uuid.uuid4().hex
    session.adb('push', str(local), remote, check=True, capture_output=True)
    try:
        result = session.adb('shell', shlex.join(['run-as', PACKAGE, 'cp', remote,
                            session.record['rootfs'] + destination]), capture_output=True, text=True)
        result.check_returncode()
    finally:
        session.adb('shell', shlex.join(['rm', '-f', remote]), capture_output=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial')
    parser.add_argument('--package', default=PACKAGE)
    parser.add_argument('--suite', choices=['all', 'inventory', *SUITES], default='all')
    parser.add_argument('--repeat', type=int, default=3)
    parser.add_argument('--seconds', type=float, default=4)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--library-path', help='Optional isolated source-fix comparison; never changes the session')
    parser.add_argument('--install-probes', action='store_true', help='Install optional Debian GUI test dependencies')
    args = parser.parse_args()
    if args.repeat < 1 or not 1 <= args.seconds <= 30:
        parser.error('repeat must be positive and seconds must be between 1 and 30')
    session = Session.connect(args.serial, args.package)
    lock_name = hashlib.sha256(session.serial.encode()).hexdigest()[:20]
    lock = open(Path(tempfile.gettempdir()) / ('portal-app-compat-' + lock_name + '.lock'), 'a')
    fcntl.flock(lock, fcntl.LOCK_EX)
    args.output.mkdir(parents=True, exist_ok=True)
    inventory = session.run("dpkg-query -W -f '${Package} ${Version} ${Architecture} ${db:Status-Status}\\n' | grep -E '^(libqt[56]|qtwayland|qt[56]-|libgtk|libreoffice|flatpak |box64 |wine|openjdk|python3-(pyqt|gi|tk|wx))'; printf '\\n--- graphics/session ---\\n'; kscreen-doctor -o; printf '\\n--- installed Flatpaks ---\\n'; flatpak list --columns=application,arch,branch,runtime", capture_output=True, text=True)
    (args.output / 'inventory.txt').write_text(inventory.stdout + inventory.stderr)
    report = dict(version=1, serial=session.serial, package=PACKAGE, engine=session.record['engine'],
                  rootfs=session.record['rootfs'], user=session.environment['USER'],
                  session={k: v for k, v in session.environment.items() if k.startswith(('QT_', 'GTK_', 'GDK_', 'QSG_'))
                           or k in ('WAYLAND_DISPLAY', 'XDG_RUNTIME_DIR', 'LD_PRELOAD', 'LD_LIBRARY_PATH')},
                  results=[])
    def save():
        (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    save()
    if args.suite == 'inventory':
        return 0
    if args.install_probes:
        result = session.run('apt-get update && apt-get install -y --no-remove --no-install-recommends ' + ' '.join(PROBE_PACKAGES),
                             root=True, timeout=1800, capture_output=True, text=True,
                             environment={'DEBIAN_FRONTEND': 'noninteractive'})
        (args.output / 'install.log').write_text(result.stdout + result.stderr)
        if result.returncode:
            raise RuntimeError('Probe dependencies could not be installed; see install.log')
    directory = '/tmp/portal-compat-' + uuid.uuid4().hex
    session.run('mkdir -m 700 ' + shlex.quote(directory), check=True)
    for probe in ('toolkit_probe.py', 'libreoffice_probe.py'):
        push(session, REPO / 'tests/compat' / probe, directory + '/' + probe)
    selected = sum(SUITES.values(), []) if args.suite == 'all' else SUITES[args.suite]
    for case in selected:
        for run in range(args.repeat + 1):
            label = f'{case}-' + ('warmup' if run == 0 else str(run))
            if case in SUITES['qt'] + SUITES['gtk']:
                command = f'python3 {directory}/toolkit_probe.py {case} --seconds {args.seconds}'
                level = 'gui_probe'
            elif case in SUITES['graphics']:
                command = {'glx': 'glxinfo -B', 'egl': 'eglinfo -B -p wayland', 'vulkan': 'vulkaninfo --summary'}[case]
                level = 'graphics_probe'
            elif case in SUITES['office']:
                command = f'python3 {directory}/libreoffice_probe.py {case} --seconds {args.seconds}'
                level = 'document_probe'
            elif case == 'vlc':
                command = f'timeout --signal=TERM --kill-after=3 {args.seconds+4}s vlc --no-one-instance --ignore-config --no-media-library'
                level = 'launch_only'
            elif case == 'electron':
                report['results'].append(dict(case=case, status='untested', reason='Requires an application-specific disposable profile and functional flow'))
                break
            else:
                report['results'].append(dict(case=case, status='untested', reason='Requires an installed application fixture; inventory records available runtimes'))
                break
            print(label, flush=True)
            env = {'WAYLAND_DEBUG': 'client'} if level != 'graphics_probe' else {}
            if args.library_path:
                env['LD_LIBRARY_PATH'] = args.library_path
            try:
                result, measurements = capture(session, command, env, args.seconds + (90 if level == 'document_probe' else 15))
                (args.output / (label + '.stdout')).write_text(result.stdout)
                (args.output / (label + '.stderr')).write_text(result.stderr)
                observations = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{') and line.endswith('}')]
                unavailable = any(x in result.stderr for x in ('ModuleNotFoundError', 'Namespace Gtk not available', 'not found'))
                success = result.returncode == 0 and (level == 'graphics_probe' or bool(observations))
                if level == 'launch_only':
                    success = result.returncode == 124 and measurements['wayland_commits'] > 0
                if any(o.get('dpr_consistent') is False for o in observations):
                    success = False
                status = ('available_launch' if level == 'launch_only' else 'passed') if success else ('unavailable' if unavailable else 'failed')
                report['results'].append(dict(case=case, run=run, warmup=run == 0, status=status,
                                              coverage=level, animated=case in SUITES['qt'], exit_code=result.returncode,
                                              observations=observations, **measurements))
            except subprocess.TimeoutExpired:
                report['results'].append(dict(case=case, run=run, status='failed', reason='ADB/guest command timed out'))
            save()
    # Deliberately keep disposable profiles and logs in the named /tmp folder
    # for diagnosis. They never replace the user's application profiles.
    report['guest_artifacts'] = directory
    save()
    return int(any(r['status'] == 'failed' for r in report['results']))


if __name__ == '__main__':
    raise SystemExit(main())
