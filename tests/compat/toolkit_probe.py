#!/usr/bin/python3
"""Short-lived real GUI probes. Emit observations, not inferred feature claims."""
import argparse
import json
import time

parser = argparse.ArgumentParser()
parser.add_argument('toolkit', choices=['qt5', 'qt6', 'quick5', 'quick6', 'pyside6', 'quickpyside6', 'gtk3', 'gtk4', 'tk', 'wx'])
parser.add_argument('--seconds', type=float, default=4)
args = parser.parse_args()
start = time.monotonic()
frames = 0
active_start_ns = None

def report(**values):
    print(json.dumps(dict(toolkit=args.toolkit, elapsed=time.monotonic()-start, frames=frames,
                          flow_start_ns=active_start_ns, flow_end_ns=time.monotonic_ns(), **values)), flush=True)


if args.toolkit in ('qt5', 'qt6', 'quick5', 'quick6', 'pyside6', 'quickpyside6'):
    if args.toolkit.endswith('5'):
        from PyQt5 import QtCore, QtGui, QtWidgets
        qt_version = QtCore.QT_VERSION_STR
    elif 'pyside' in args.toolkit:
        from PySide6 import QtCore, QtGui, QtWidgets
        qt_version = QtCore.qVersion()
    else:
        from PyQt6 import QtCore, QtGui, QtWidgets
        qt_version = QtCore.QT_VERSION_STR
    app = QtWidgets.QApplication([])
    startup_screen_dpr = app.primaryScreen().devicePixelRatio()
    dialog_geometry = None
    if args.toolkit.startswith('quick'):
        if args.toolkit.endswith('5'):
            from PyQt5.QtQuick import QQuickView
            from PyQt5.QtQml import QQmlComponent
        elif 'pyside' in args.toolkit:
            from PySide6.QtQuick import QQuickView
            from PySide6.QtQml import QQmlComponent
        else:
            from PyQt6.QtQuick import QQuickView
            from PyQt6.QtQml import QQmlComponent
        window = QQuickView()
        window.setResizeMode(getattr(QQuickView, 'ResizeMode', QQuickView).SizeRootObjectToView)
        component = QQmlComponent(window.engine())
        component.setData(b'import QtQuick 2.15\nRectangle { width: 720; height: 480; color: "#192533"; Rectangle { width: 100; height: 100; color: "#e8794a"; y: 190; SequentialAnimation on x { loops: Animation.Infinite; NumberAnimation { to: 620; duration: 700 } NumberAnimation { to: 0; duration: 700 } } } }', QtCore.QUrl())
        if component.isError():
            raise RuntimeError(str(component.errors()))
        item = component.create()
        window.setContent(QtCore.QUrl(), component, item)
        window.resize(720, 480)
        window.setTitle('Portal compatibility: ' + args.toolkit)
        def swapped():
            global frames, active_start_ns
            if active_start_ns is None:
                active_start_ns = time.monotonic_ns()
            frames += 1
        window.frameSwapped.connect(swapped)
        window.show()
    else:
        class Canvas(QtWidgets.QWidget):
            def paintEvent(self, event):
                global frames, active_start_ns
                if active_start_ns is None:
                    active_start_ns = time.monotonic_ns()
                frames += 1
                painter = QtGui.QPainter(self)
                painter.fillRect(self.rect(), QtGui.QColor('#192533'))
                painter.fillRect((frames * 6) % max(1, self.width()-100), 80, 100, 100, QtGui.QColor('#e8794a'))
        window = QtWidgets.QWidget()
        window.setWindowTitle('Portal compatibility: ' + args.toolkit)
        layout = QtWidgets.QVBoxLayout(window)
        layout.addWidget(QtWidgets.QLabel('Native toolkit / fractional scaling / Unicode: 日本語 café'))
        edit = QtWidgets.QLineEdit('editable Unicode: 日本語 café')
        layout.addWidget(edit)
        canvas = Canvas()
        canvas.setMinimumSize(720, 300)
        layout.addWidget(canvas)
        layout.addWidget(QtWidgets.QPushButton('Dialog geometry test'))
        window.resize(760, 480)
        window.show()
        dialog = QtWidgets.QDialog(window)
        dialog.setWindowTitle('Portal compatibility: native dialog geometry')
        dialog_layout = QtWidgets.QVBoxLayout(dialog)
        dialog_layout.addWidget(QtWidgets.QLabel('Dialog / Unicode: 日本語 café'))
        dialog_layout.addWidget(QtWidgets.QLineEdit('Editable text'))
        dialog_layout.addWidget(QtWidgets.QPushButton('Close'))
        dialog.resize(520, 240)
        def check_dialog():
            global dialog_geometry
            available = dialog.windowHandle().screen().availableGeometry()
            dialog_geometry = (dialog.width() <= available.width() and dialog.height() <= available.height()
                               and all(dialog.rect().contains(dialog_layout.itemAt(i).widget().geometry())
                                       for i in range(dialog_layout.count())))
            dialog.close()
        QtCore.QTimer.singleShot(round(args.seconds * 250), dialog.show)
        QtCore.QTimer.singleShot(round(args.seconds * 500), check_dialog)
        timer = QtCore.QTimer()
        timer.timeout.connect(canvas.update)
        timer.start(16)
    def finish():
        handle = window if args.toolkit.startswith('quick') else window.windowHandle()
        values = dict(version=qt_version, backend=app.platformName(), startup_screen_dpr=startup_screen_dpr,
                      window_dpr=handle.devicePixelRatio(), screen_dpr=handle.screen().devicePixelRatio(),
                      refresh_hz=handle.screen().refreshRate(), geometry=[window.width(), window.height()])
        values['resize_consistent'] = (window.width() == 800 and window.height() == 540
                                       and handle.width() == window.width() and handle.height() == window.height())
        if not args.toolkit.startswith('quick'):
            values['widget_dpr'] = window.devicePixelRatioF()
            values['dpr_consistent'] = abs(window.devicePixelRatioF()-handle.devicePixelRatio()) < 0.01
            values['dialog_geometry_consistent'] = dialog_geometry
        report(**values)
        app.quit()
    QtCore.QTimer.singleShot(round(args.seconds * 650), lambda: window.resize(800, 540))
    QtCore.QTimer.singleShot(round(args.seconds*1000), finish)
    (getattr(app, 'exec', None) or app.exec_)()
elif args.toolkit in ('gtk3', 'gtk4'):
    import gi
    version = '3.0' if args.toolkit == 'gtk3' else '4.0'
    gi.require_version('Gtk', version)
    from gi.repository import Gtk, GLib, Gdk
    app = Gtk.Application(application_id='org.portal.CompatibilityProbe')
    def activate(app):
        window = Gtk.ApplicationWindow(application=app, title='Portal compatibility: ' + args.toolkit)
        window.set_default_size(720, 480)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=16)
        label = Gtk.Label(label='Native toolkit / Unicode: 日本語 café')
        entry = Gtk.Entry()
        entry.set_text('editable Unicode: 日本語 café')
        if args.toolkit == 'gtk3':
            box.pack_start(label, False, False, 0)
            box.pack_start(entry, False, False, 0)
            window.add(box)
            window.show_all()
        else:
            box.append(label)
            box.append(entry)
            window.set_child(box)
            window.present()
        def finish():
            report(version=version, backend=Gdk.Display.get_default().__class__.__name__, scale=window.get_scale_factor())
            app.quit()
            return False
        GLib.timeout_add(round(args.seconds*1000), finish)
    app.connect('activate', activate)
    app.run([])
elif args.toolkit == 'tk':
    import tkinter as tk
    window = tk.Tk()
    window.title('Portal compatibility: Tk')
    window.geometry('720x480')
    tk.Label(window, text='Native Tk / Unicode: 日本語 café').pack()
    tk.Entry(window).pack()
    def finish():
        report(version=tk.TkVersion, backend=window.tk.call('tk', 'windowingsystem'))
        window.destroy()
    window.after(round(args.seconds*1000), finish)
    window.mainloop()
else:
    import wx
    app = wx.App(False)
    window = wx.Frame(None, title='Portal compatibility: wxWidgets', size=(720, 480))
    panel = wx.Panel(window)
    wx.TextCtrl(panel, value='Native wxWidgets / Unicode: 日本語 café', pos=(20, 40), size=(650, 40))
    window.Show()
    def finish():
        report(version=wx.version(), backend=wx.PlatformInfo)
        window.Destroy()
    wx.CallLater(round(args.seconds*1000), finish)
    app.MainLoop()
