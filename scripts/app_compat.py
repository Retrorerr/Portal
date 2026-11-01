#!/usr/bin/env python3
"""Repeatable physical-device app compatibility checks for Portal Debug.

Reports distinguish tested GUI probes, launch-only checks and unavailable
runtimes. SurfaceFlinger measures the desktop SurfaceView, not an individual
client: guest Wayland commits and probe counters are recorded separately.
"""
import argparse
import concurrent.futures
import contextlib
import fcntl
import hashlib
import tempfile
import json
import math
from pathlib import Path
import re
import shlex
import statistics
import subprocess
import time
import uuid
import xml.etree.ElementTree as ET
from guest_exec import Session, PACKAGE

REPO = Path(__file__).resolve().parents[1]
SUITES = {
    'qt': ['qt5', 'qt6', 'quick5', 'quick6', 'pyside6', 'quickpyside6'],
    'gtk': ['gtk2', 'gtk3', 'gtk4', 'tk', 'wx'],
    'graphics': ['glx', 'egl', 'vulkan', 'sdl', 'vkwayland', 'vkxcb'],
    'office': ['writer', 'calc', 'impress'],
    'apps': ['vlc', 'chromium', 'electron', 'flatpak', 'translated'],
    'media': ['h264', 'hevc', 'vp9', 'av1'],
    'portable': ['linux', 'windows', 'appimage'],
    'java': ['swing'],
}
PROBE_PACKAGES = ['python3-pyqt5', 'python3-pyqt5.qtquick', 'python3-pyqt6',
                  'python3-pyqt6.qtquick', 'python3-gi', 'gir1.2-gtk-3.0',
                  'gir1.2-gtk-4.0', 'python3-tk', 'python3-wxgtk4.0',
                  'mesa-utils', 'vulkan-tools', 'python3-uno', 'time',
                  'python3-pyside6.qtwidgets', 'python3-pyside6.qtquick',
                  'libgtk2.0-0t64', 'libsdl2-2.0-0', 'default-jre', 'x11-utils', 'gcc', 'libc6-dev', 'libvulkan-dev']


def desktop_visibility(raw):
    """A live SurfaceView alone is insufficient: the veil can cover it."""
    start, end = raw.find('<?xml'), raw.find('</hierarchy>')
    if start < 0 or end < 0:
        raise RuntimeError('Android UI inspection failed; desktop visibility cannot be verified')
    tree = ET.fromstring(raw[start:end + len('</hierarchy>')])
    surface, veil, entry = None, False, None
    for node in tree.iter('node'):
        if node.get('package') != PACKAGE:
            continue
        text = node.get('text', '')
        bounds = tuple(map(int, re.findall(r'\d+', node.get('bounds', ''))))
        if node.get('class') == 'android.view.SurfaceView' and len(bounds) == 4:
            surface = bounds
        if 'Swipe up to enter Portal' in text or text == 'Returning to Plasma':
            veil = True
        if 'Swipe up to enter Portal' in text and len(bounds) == 4:
            entry = bounds
    return dict(visible=surface is not None and not veil, veil=veil,
                surface_bounds=surface, entry_bounds=entry)


def visible_desktop(session, dismiss_veil=False):
    def inspect():
        result = session.adb('exec-out', 'uiautomator', 'dump', '/dev/tty',
                             capture_output=True, text=True, timeout=20)
        return desktop_visibility(result.stdout)
    state = inspect()
    if dismiss_veil and state['veil'] and state['entry_bounds'] and state['surface_bounds']:
        x0, y0, x1, y1 = state['entry_bounds']
        sx0, sy0, sx1, sy1 = state['surface_bounds']
        x, y = (x0 + x1) // 2, (y0 + y1) // 2
        # Bounds come from Android's UI tree. A tap does not prove that
        # the veil was swiped away; perform the real desktop-entry gesture.
        target = sy0 + (sy1 - sy0) * 2 // 5
        session.adb('shell', f'input swipe {x} {y} {x} {target} 450', check=True)
        for _ in range(3):
            state = inspect()
            if state['visible']:
                state['veil_swiped'] = True
                return state
    if not state['visible']:
        raise RuntimeError('Portal Debug desktop is not visible. Open it and swipe up the veil before measuring performance.')
    state['veil_swiped'] = False
    return state


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
    rows, period, frame_periods = set(), None, {}
    if layer:
        session.adb('shell', 'dumpsys SurfaceFlinger --latency-clear ' + shlex.quote(layer),
                    capture_output=True)
    before_cpu, before_pss = cpu_snapshot(session), memory(session)
    started = time.monotonic()
    with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
        # The desktop broker owns the remote process group and its timeout.
        # A nested GNU timeout would create another group outside that
        # cancellation boundary.
        timed = ("if [ -x /usr/bin/time ]; then exec /usr/bin/time -f "
                 "'PORTAL_RESOURCES %U %S %M %x' /bin/sh -c " + shlex.quote(command)
                 + "; else exec /bin/sh -c " + shlex.quote(command) + "; fi")
        future = pool.submit(session.run_desktop, timed, environment=environment, timeout=timeout,
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
                            row = tuple(map(int, fields))
                            rows.add(row)
                            frame_periods.setdefault(row[1], period)
            time.sleep(0.25)
        result = future.result()
    duration = time.monotonic() - started
    after_cpu = cpu_snapshot(session)
    # Exited processes cannot be recovered from /proc. GNU time separately
    # records the command's CPU and maximum process RSS when available.
    ticks = sum(max(0, after_cpu[p]-value) for p, value in before_cpu.items() if p in after_cpu)
    resources = re.search(r'^PORTAL_RESOURCES ([\d.]+) ([\d.]+) (\d+) (\d+)$', result.stderr, re.M)
    resources = (dict(user_seconds=float(resources[1]), system_seconds=float(resources[2]),
                      max_process_rss_kib=int(resources[3]), exit_code=int(resources[4]))
                 if resources else dict(status='unavailable', reason='No completed GNU time record; tool absent or command interrupted'))
    return result, dict(duration_seconds=duration, existing_process_cpu_ticks=ticks,
                        desktop_launch_context=getattr(result, 'portal_launch_context', 'unavailable'),
                        command_resources=resources, _surface_rows=rows, _frame_periods=frame_periods,
                        pss_before_kib=before_pss, pss_after_kib=memory(session),
                        desktop_presentation=frame_stats(rows, period),
                        wayland_commits=len(re.findall(r'wl_surface[@#]\d+\.commit\(', result.stderr)))


def presentation_for_flow(rows, periods, observations):
    ranges = [(o['flow_start_ns'], o['flow_end_ns']) for o in observations
              if o.get('flow_start_ns') and o.get('flow_end_ns')]
    active = [row for row in rows if any(a <= row[1] <= b for a, b in ranges)] if ranges else list(rows)
    sampled = [periods[row[1]] for row in active if periods.get(row[1])]
    result = frame_stats(active, statistics.mode(sampled) if sampled else None)
    result['scope'] = 'active_flow' if ranges else 'process_lifetime'
    result['refresh_rates_hz'] = sorted({round(1e9 / value, 3) for value in sampled})
    # Static windows naturally stop presenting. Their gaps are not jank.
    result['continuous_flow'] = bool(ranges)
    result['pacing_status'] = ('not_evaluated' if not ranges else
                               'insufficient_samples' if result.get('status') == 'insufficient_samples' else
                               'gaps_over_100ms_observed' if result['gaps_over_100ms'] else 'no_gaps_over_100ms')
    return result


def push(session, local, destination):
    remote = '/data/local/tmp/portal-compat-' + uuid.uuid4().hex
    session.adb('push', str(local), remote, check=True, capture_output=True)
    try:
        result = session.adb('shell', shlex.join(['run-as', PACKAGE, 'cp', remote,
                            session.record['rootfs'] + destination]), capture_output=True, text=True)
        result.check_returncode()
    finally:
        session.adb('shell', shlex.join(['rm', '-f', remote]), capture_output=True)


def fixture_files(directory):
    if directory is None:
        return {}
    manifest = json.loads((directory / 'manifest.json').read_text())
    files = {}
    for kind, item in manifest.items():
        name = item['file']
        if Path(name).name != name or name in ('', '.', '..'):
            raise ValueError('Fixture names must be filenames within their manifest directory')
        path = directory / name
        if hashlib.sha256(path.read_bytes()).hexdigest() != item['sha256']:
            raise ValueError('Fixture SHA256 mismatch: ' + name)
        files[kind] = path
    return files


@contextlib.contextmanager
def perfetto_capture(session, destination, duration):
    """Optional scheduler/graphics evidence for the exact measured flow."""
    metadata = dict(status='disabled')
    if destination is None:
        yield metadata
        return
    remote = '/data/misc/perfetto-traces/portal-compat-' + uuid.uuid4().hex + '.pftrace'
    command = shlex.join(['perfetto', '--background-wait', '-o', remote, '-t', str(math.ceil(duration)) + 's',
                          '--app', PACKAGE, 'sched', 'freq', 'idle', 'wm', 'gfx', 'view', 'binder_driver'])
    result = session.adb('shell', command, capture_output=True, text=True, timeout=20)
    pids = re.findall(r'^\s*(\d+)\s*$', result.stdout, re.M)
    if result.returncode or not pids:
        metadata.update(status='unavailable', reason=result.stderr or result.stdout)
        yield metadata
        return
    try:
        yield metadata
    finally:
        session.adb('shell', 'kill -INT ' + pids[-1], capture_output=True, timeout=10)
        # Wait for the writer to flush, with a small, bounded stability check.
        previous = None
        for _ in range(12):
            size = session.adb('shell', 'stat -c %s ' + shlex.quote(remote),
                               capture_output=True, text=True, timeout=10).stdout.strip()
            if size.isdigit() and int(size) > 0 and size == previous:
                break
            previous = size
            time.sleep(0.25)
        pulled = session.adb('pull', remote, str(destination), capture_output=True, text=True, timeout=30)
        if pulled.returncode == 0:
            metadata.update(status='captured', file=destination.name)
            session.adb('shell', 'rm -f ' + shlex.quote(remote), capture_output=True)
        else:
            metadata.update(status='unavailable', reason=pulled.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial')
    parser.add_argument('--package', default=PACKAGE)
    parser.add_argument('--suite', choices=['all', 'inventory', *SUITES], default='all')
    parser.add_argument('--case', action='append', choices=sum(SUITES.values(), []), help='Limit the selected suite to these probes')
    parser.add_argument('--repeat', type=int, default=3)
    parser.add_argument('--seconds', type=float, default=4)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--library-path', help='Optional isolated source-fix comparison; never changes the session')
    parser.add_argument('--install-probes', action='store_true', help='Install optional Debian GUI test dependencies')
    parser.add_argument('--fixtures', type=Path, help='Directory from build_compat_fixtures.py; verifies its SHA256 manifest')
    parser.add_argument('--launcher', type=Path, help='Stage a candidate portable launcher instead of testing installed portal-run')
    parser.add_argument('--perfetto', action='store_true', help='Record a scheduler/graphics trace for each probe')
    parser.add_argument('--env', action='append', default=[], metavar='KEY=VALUE', help='Explicit per-probe environment for fallback/renderer comparisons')
    parser.add_argument('--require-frame-pacing', action='store_true', help='Fail if an active GUI flow has presentation gaps over 100 ms or insufficient samples')
    parser.add_argument('--flatpak-installation', choices=['user', 'system'], default='user')
    args = parser.parse_args()
    if args.repeat < 1 or not 1 <= args.seconds <= 30:
        parser.error('repeat must be positive and seconds must be between 1 and 30')
    if any(not re.fullmatch(r'[A-Za-z_][A-Za-z_0-9]*=.*', value, re.S) for value in args.env):
        parser.error('--env must be KEY=VALUE')
    overrides = dict(value.split('=', 1) for value in args.env)
    selected = sum(SUITES.values(), []) if args.suite == 'all' else SUITES.get(args.suite, [])
    if args.case:
        if any(case not in selected for case in args.case):
            parser.error('--case must belong to the selected suite')
        selected = [case for case in selected if case in args.case]
    session = Session.connect(args.serial, args.package)
    lock_name = hashlib.sha256(session.serial.encode()).hexdigest()[:20]
    lock = open(Path(tempfile.gettempdir()) / ('portal-app-compat-' + lock_name + '.lock'), 'a')
    fcntl.flock(lock, fcntl.LOCK_EX)
    args.output.mkdir(parents=True, exist_ok=True)
    visibility = visible_desktop(session, dismiss_veil=True) if args.suite != 'inventory' else None
    if visibility:
        screenshot = session.adb('exec-out', 'screencap', '-p', capture_output=True, check=True)
        (args.output / 'desktop-before.png').write_bytes(screenshot.stdout)
    inventory = session.run("dpkg-query -W -f '${Package} ${Version} ${Architecture} ${db:Status-Status}\\n' | grep -E '^(libqt[56]|qtwayland|qt[56]-|libgtk|libreoffice|flatpak |box64 |wine|openjdk|chromium|ffmpeg |libavcodec|libva|mesa-|vulkan-|libegl|libgl|libsdl|libpyside|python3-(pyqt|pyside|gi|tk|wx))'; printf '\\n--- graphics/session ---\\n'; kscreen-doctor -o; printf '\\n--- installed Flatpaks ---\\n'; flatpak list --columns=application,arch,branch,runtime", capture_output=True, text=True)
    (args.output / 'inventory.txt').write_text(inventory.stdout + inventory.stderr)
    report = dict(version=2, serial=session.serial, package=PACKAGE, engine=session.record['engine'],
                  rootfs=session.record['rootfs'], user=session.environment['USER'],
                  session={k: v for k, v in session.environment.items() if k.startswith(('QT_', 'GTK_', 'GDK_', 'QSG_', 'LIBVA_', 'TERMUX_VA_', 'MESA_', 'GALLIUM_', 'VK_', 'EGL_', 'LIBGL_', 'SDL_', 'BOX64_', 'FEX_', 'FD_', 'TURNIP_', 'FLATPAK_'))
                           or k in ('DISPLAY', 'WAYLAND_DISPLAY', 'XDG_RUNTIME_DIR', 'LD_PRELOAD', 'LD_LIBRARY_PATH', 'JAVA_TOOL_OPTIONS', '_JAVA_OPTIONS', 'JDK_JAVA_OPTIONS')},
                  library_path_override=args.library_path,
                  launcher_override=str(args.launcher) if args.launcher else None,
                  environment_overrides=overrides,
                  desktop_visibility=visibility, results=[])
    report['selected_runtime'] = {}
    for name in ('renderer-mode', 'guest-engine', 'hw-video-decode'):
        value = session.adb('shell', '-T', shlex.join(['run-as', PACKAGE, 'cat', 'files/' + name]),
                            capture_output=True, text=True)
        report['selected_runtime'][name] = value.stdout.strip() if value.returncode == 0 else 'default'
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
    report['probe_sources'] = {}
    for probe in ('toolkit_probe.py', 'libreoffice_probe.py', 'media_probe.py', 'sdl_probe.py', 'vulkan_probe.py', 'gtk2_probe.py', 'windows_probe.py', 'vlc_probe.py', 'chromium_probe.py', 'JavaProbe.java', 'flatpak_probe.py', 'flatpak_sdl_probe.c'):
        source = REPO / 'tests/compat' / probe
        report['probe_sources'][probe] = hashlib.sha256(source.read_bytes()).hexdigest()
        push(session, source, directory + '/' + probe)
    fixtures = fixture_files(args.fixtures)
    report['fixtures'] = {kind: dict(file=path.name, sha256=hashlib.sha256(path.read_bytes()).hexdigest())
                          for kind, path in fixtures.items()}
    remote_fixtures = {}
    for kind, local in fixtures.items():
        destination = directory + '/' + local.name
        push(session, local, destination)
        if kind in ('linux', 'appimage'):
            session.run('chmod 700 ' + shlex.quote(destination), check=True)
        remote_fixtures[kind] = destination
    launcher = '/usr/local/bin/portal-run'
    if args.launcher:
        launcher = directory + '/portal-run.py'
        push(session, args.launcher, launcher)
    launcher_command = shlex.join(['python3', launcher])
    for case in selected:
        for run in range(args.repeat + 1):
            label = f'{case}-' + ('warmup' if run == 0 else str(run))
            env = {}
            timeout = args.seconds + 15
            if case in ('gtk2', 'sdl'):
                command = f'python3 {directory}/{case}_probe.py --seconds {args.seconds}'
                level = 'gui_probe'
            elif case in SUITES['qt'] + SUITES['gtk']:
                command = f'python3 {directory}/toolkit_probe.py {case} --seconds {args.seconds}'
                level = 'gui_probe'
            elif case in ('vkwayland', 'vkxcb'):
                backend = 'wayland' if case == 'vkwayland' else 'xcb'
                command = f'python3 {directory}/vulkan_probe.py {backend} --seconds {args.seconds}'
                level, timeout = 'graphics_gui_probe', args.seconds + 30
            elif case in SUITES['graphics']:
                command = {'glx': 'glxinfo -B', 'egl': 'eglinfo -B -p wayland', 'vulkan': 'vulkaninfo --summary'}[case]
                level = 'graphics_probe'
            elif case in SUITES['office']:
                command = f'python3 {directory}/libreoffice_probe.py {case} --seconds {args.seconds}'
                level = 'document_probe'
                timeout = args.seconds + 100
            elif case == 'swing':
                command = f'java {directory}/JavaProbe.java {args.seconds}'
                level, timeout = 'gui_probe', args.seconds + 35
            elif case in SUITES['media'] + SUITES['portable']:
                if case not in remote_fixtures:
                    report['results'].append(dict(case=case, status='untested', reason='Provide an owned fixture with --fixtures; see build_compat_fixtures.py'))
                    break
                fixture = shlex.quote(remote_fixtures[case])
                if case in SUITES['media']:
                    command = f'python3 {directory}/media_probe.py {fixture}'
                    level, timeout = 'hardware_decode', 130
                else:
                    command = f'{launcher_command} {fixture}'
                    if case == 'windows':
                        command = f'python3 {directory}/windows_probe.py {shlex.quote(launcher)} {fixture}'
                    if case in ('linux', 'appimage'):
                        command += ' ' + shlex.join(['a b', '日本語'])
                    # Prefixes and extraction caches belong to this run.
                    env.update(STEAM_COMPAT_DATA_PATH=directory + '/windows-prefix',
                               XDG_CACHE_HOME=directory + '/cache')
                    level, timeout = 'execution_probe', 180
            elif case == 'vlc':
                command = f'python3 {directory}/vlc_probe.py --seconds {args.seconds}'
                level = 'launch_only'
                env.update(XDG_CONFIG_HOME=directory + '/vlc-config', XDG_CACHE_HOME=directory + '/vlc-cache')
            elif case == 'chromium':
                command = f'python3 {directory}/chromium_probe.py --seconds {args.seconds}'
                level, timeout = 'gui_probe', args.seconds + 40
            elif case == 'electron':
                report['results'].append(dict(case=case, status='untested', reason='Requires an application-specific disposable profile and functional flow'))
                break
            elif case == 'flatpak':
                command = f'python3 {directory}/flatpak_probe.py --directory {directory}/flatpak --installation {args.flatpak_installation} --seconds {args.seconds}'
                level, timeout = 'packaged_graphics_probe', args.seconds + 120
            else:
                report['results'].append(dict(case=case, status='untested', reason='Requires a translated GUI application fixture; the portable suite tests console execution'))
                break
            print(label, flush=True)
            if level not in ('graphics_probe', 'execution_probe', 'hardware_decode'):
                env['WAYLAND_DEBUG'] = 'client'
            if args.library_path:
                env['LD_LIBRARY_PATH'] = args.library_path
            env.update(overrides)
            try:
                trace = args.output / (label + '.pftrace') if args.perfetto else None
                with perfetto_capture(session, trace, timeout + 10) as trace_metadata:
                    result, measurements = capture(session, command, env, timeout)
                measurements['perfetto'] = trace_metadata
                (args.output / (label + '.stdout')).write_text(result.stdout)
                (args.output / (label + '.stderr')).write_text(result.stderr)
                observations = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{') and line.endswith('}')]
                rows = measurements.pop('_surface_rows')
                periods = measurements.pop('_frame_periods')
                measurements['desktop_presentation'] = presentation_for_flow(rows, periods, observations)
                (args.output / (label + '.frames.csv')).write_text(
                    'desired_present_ns,actual_present_ns,frame_ready_ns\n' +
                    ''.join(','.join(map(str, row)) + '\n' for row in sorted(rows)))
                unavailable = any(x in result.stderr for x in (
                    'ModuleNotFoundError', 'Namespace Gtk not available', 'not found',
                    'runtime missing', 'install Box64', 'install FEX', 'cannot open shared object file'))
                success = result.returncode == 0 and (level == 'graphics_probe' or bool(observations))
                if level == 'launch_only':
                    success = result.returncode == 0 and any(o.get('qt_interface_window') for o in observations)
                if level == 'packaged_graphics_probe':
                    success = result.returncode == 0 and any(o.get('hardware_gl') and o.get('external_canary_denied') for o in observations)
                if any(o.get('dpr_consistent') is False or o.get('dialog_geometry_consistent') is False or
                       o.get('resize_consistent') is False for o in observations):
                    success = False
                status = ('available_launch' if level == 'launch_only' else 'passed') if success else ('unavailable' if unavailable else 'failed')
                try:
                    visible_desktop(session)
                    visible_after = True
                except RuntimeError:
                    visible_after = False
                    status = 'invalid'
                report['results'].append(dict(case=case, run=run, warmup=run == 0, status=status,
                                              desktop_visible_after=visible_after,
                                              coverage=level, animated=measurements['desktop_presentation']['continuous_flow'], exit_code=result.returncode,
                                              observations=observations, **measurements))
            except subprocess.TimeoutExpired:
                report['results'].append(dict(case=case, run=run, warmup=run == 0, status='failed', reason='ADB/guest command timed out'))
            save()
        if case == 'flatpak':
            session.run_desktop(f'python3 {directory}/flatpak_probe.py --directory {directory}/flatpak --installation {args.flatpak_installation} --cleanup',
                                timeout=45, capture_output=True)
    # Deliberately keep disposable profiles and logs in the named /tmp folder
    # for diagnosis. They never replace the user's application profiles.
    report['guest_artifacts'] = directory
    save()
    return int(any(r['status'] in ('failed', 'invalid') or
                   (args.require_frame_pacing and not r.get('warmup', False) and r.get('desktop_presentation', {}).get('pacing_status')
                    in ('gaps_over_100ms_observed', 'insufficient_samples')) for r in report['results']))


if __name__ == '__main__':
    raise SystemExit(main())
