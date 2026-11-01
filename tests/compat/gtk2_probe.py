#!/usr/bin/python3
"""A GTK 2 window using its normal runtime without obsolete Python bindings."""
import argparse
import ctypes as C
import json

parser = argparse.ArgumentParser()
parser.add_argument('--seconds', type=float, default=4)
args = parser.parse_args()
gtk = C.CDLL('libgtk-x11-2.0.so.0')
glib = C.CDLL('libglib-2.0.so.0')


def api(library, name, result, *arguments):
    function = getattr(library, name)
    function.restype, function.argtypes = result, list(arguments)
    return function


init = api(gtk, 'gtk_init_check', C.c_int, C.c_void_p, C.c_void_p)
window_new = api(gtk, 'gtk_window_new', C.c_void_p, C.c_int)
label_new = api(gtk, 'gtk_label_new', C.c_void_p, C.c_char_p)
add = api(gtk, 'gtk_container_add', None, C.c_void_p, C.c_void_p)
resize = api(gtk, 'gtk_window_set_default_size', None, C.c_void_p, C.c_int, C.c_int)
show = api(gtk, 'gtk_widget_show_all', None, C.c_void_p)
destroy = api(gtk, 'gtk_widget_destroy', None, C.c_void_p)
main = api(gtk, 'gtk_main', None)
quit_gtk = api(gtk, 'gtk_main_quit', None)
callback_type = C.CFUNCTYPE(C.c_int, C.c_void_p)
timeout = api(glib, 'g_timeout_add', C.c_uint, C.c_uint, callback_type, C.c_void_p)
if not init(None, None):
    raise RuntimeError('GTK 2 could not open the selected display')
window = window_new(0)
resize(window, 720, 480)
add(window, label_new('Native GTK 2 / Unicode: 日本語 café'.encode()))
show(window)


@callback_type
def finish(_):
    print(json.dumps(dict(toolkit='GTK2', backend='X11', window_shown=True)), flush=True)
    quit_gtk()
    return 0


timeout(round(args.seconds * 1000), finish, None)
try:
    main()
finally:
    destroy(window)
