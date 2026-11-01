#!/usr/bin/python3
"""Exercise real LibreOffice documents using an isolated UNO profile."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import uuid
import sys
import re
import uno
from com.sun.star.beans import PropertyValue

parser = argparse.ArgumentParser()
parser.add_argument('component', choices=['writer', 'calc', 'impress'])
parser.add_argument('--seconds', type=float, default=4)
args = parser.parse_args()


def phase(name):
    print(f'PORTAL_OFFICE_PHASE {time.monotonic():.3f} {name}', file=sys.stderr, flush=True)


def prop(name, value):
    p = PropertyValue()
    p.Name, p.Value = name, value
    return p


def backend_modules(pid):
    pending, seen, modules = [pid], set(), set()
    children = {}
    try:
        # Android kernels can omit /proc/PID/task/PID/children.
        for line in subprocess.check_output(['ps', '-eo', 'pid=,ppid='], text=True, timeout=5).splitlines():
            child, parent = map(int, line.split())
            children.setdefault(parent, []).append(child)
    except (OSError, ValueError, subprocess.SubprocessError):
        pass
    while pending and len(seen) < 32:
        pid = pending.pop()
        if pid in seen:
            continue
        seen.add(pid)
        pending.extend(children.get(pid, []))
        try:
            maps = Path(f'/proc/{pid}/maps').read_text()
            modules.update(re.findall(r'/(libvclplug_[^/\s]+\.so|libQt[56]Core\.so\.[^/\s]+)', maps))
        except (OSError, ValueError):
            pass
    return sorted(modules)


with tempfile.TemporaryDirectory(prefix='portal-office-probe-') as directory:
    directory = Path(directory)
    pipe = 'portal-qa-' + uuid.uuid4().hex
    log = (directory / 'office.log').open('w+')
    process = subprocess.Popen(['/usr/bin/libreoffice', '-env:UserInstallation=' + directory.as_uri(),
                                '--norestore', '--nofirststartwizard', '--nodefault',
                                '--accept=pipe,name=' + pipe + ';urp;StarOffice.ComponentContext'],
                               stdout=log, stderr=log)
    doc = None
    try:
        phase('connect')
        local = uno.getComponentContext()
        resolver = local.ServiceManager.createInstanceWithContext('com.sun.star.bridge.UnoUrlResolver', local)
        context = None
        deadline = time.monotonic() + 25
        while time.monotonic() < deadline:
            try:
                context = resolver.resolve('uno:pipe,name=' + pipe + ';urp;StarOffice.ComponentContext')
                break
            except Exception:
                if process.poll() is not None:
                    raise RuntimeError('LibreOffice exited before UNO became ready')
                time.sleep(0.1)
        if context is None:
            raise RuntimeError('LibreOffice UNO startup timed out')
        manager = context.ServiceManager
        phase('create_document')
        desktop = manager.createInstanceWithContext('com.sun.star.frame.Desktop', context)
        factory = {'writer': 'swriter', 'calc': 'scalc', 'impress': 'simpress'}[args.component]
        doc = desktop.loadComponentFromURL('private:factory/' + factory, '_blank', 0, (prop('Hidden', False),))
        controller = doc.getCurrentController()
        frame = controller.getFrame()
        window = frame.getContainerWindow()
        if args.component == 'writer':
            phase('populate_writer')
            doc.getText().setString('\n'.join(f'Paragraph {i}: Unicode 日本語 café. ' + 'Portable desktop applications. ' * 12 for i in range(300)))
        elif args.component == 'calc':
            phase('populate_calc')
            sheet = doc.getSheets().getByIndex(0)
            data = tuple(tuple(f'{r}:{c}' for c in range(50)) for r in range(1000))
            sheet.getCellRangeByPosition(0, 0, 49, 999).setDataArray(data)
        else:
            phase('populate_impress')
            pages = doc.getDrawPages()
            for i in range(1, 10):
                page = pages.insertNewByIndex(i)
                shape = doc.createInstance('com.sun.star.drawing.TextShape')
                page.add(shape)
                shape.setString(f'Portal compatibility slide {i}: 日本語 café')
        # Use normal format handling and verify the content survives reopen.
        suffix, format_name = {'writer': ('odt', 'writer8'), 'calc': ('ods', 'calc8'),
                               'impress': ('odp', 'impress8')}[args.component]
        file = directory / ('fixture.' + suffix)
        phase('save')
        doc.storeAsURL(file.as_uri(), (prop('FilterName', format_name), prop('Overwrite', True)))
        assert file.stat().st_size > 0
        phase('reopen')
        doc.close(True)
        doc = desktop.loadComponentFromURL(file.as_uri(), '_blank', 0, (prop('Hidden', False),))
        if args.component == 'writer':
            assert 'Paragraph 299: Unicode 日本語 café.' in doc.getText().getString()
        elif args.component == 'calc':
            assert doc.getSheets().getByIndex(0).getCellByPosition(49, 999).getString() == '999:49'
        else:
            assert doc.getDrawPages().getCount() == 10
        controller = doc.getCurrentController()
        window = controller.getFrame().getContainerWindow()
        time.sleep(0.5)
        def move_view(index):
            if args.component == 'writer':
                controller.getViewCursor().jumpToPage(1 + (index % 8))
            elif args.component == 'calc':
                controller.setFirstVisibleRow((index * 8) % 900)
            else:
                controller.setCurrentPage(doc.getDrawPages().getByIndex(index % 10))
        # Warm layout and glyph/shader caches before the measured interval.
        phase('warm_views')
        for index in range(10):
            move_view(index)
        time.sleep(0.25)
        start, count = time.monotonic(), 0
        flow_start_ns = time.monotonic_ns()
        phase('active_flow')
        while time.monotonic() - start < args.seconds:
            move_view(count)
            count += 1
            time.sleep(0.016)
        rect = window.getPosSize()
        info = window.getInfo()
        flow_end_ns = time.monotonic_ns()
        print(json.dumps(dict(component=args.component, document_saved=True, viewport_updates=count,
                              elapsed=time.monotonic()-start,
                              flow_start_ns=flow_start_ns, flow_end_ns=flow_end_ns,
                              backend_modules=backend_modules(process.pid),
                              window=[rect.X, rect.Y, rect.Width, rect.Height],
                              dpi_x=info.PixelPerMeterX * 0.0254, dpi_y=info.PixelPerMeterY * 0.0254)), flush=True)
        phase('terminate')
        # Terminate with the saved document still attached. Closing its last
        # frame first can leave an in-flight Start Center activation racing
        # this separate remote termination request.
        assert desktop.terminate()
        doc = None
        process.wait(timeout=10)
    finally:
        if doc is not None:
            try:
                doc.close(True)
            except Exception:
                pass
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        log.seek(0)
        print(log.read(), file=sys.stderr)
        log.close()
