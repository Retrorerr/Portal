#!/usr/bin/python3
"""Exercise SDL's selected video backend and its real OpenGL context."""
import argparse
import ctypes as C
import json
import time

parser = argparse.ArgumentParser()
parser.add_argument('--seconds', type=float, default=4)
args = parser.parse_args()
sdl = C.CDLL('libSDL2-2.0.so.0')


def api(name, result, *arguments):
    function = getattr(sdl, name)
    function.restype, function.argtypes = result, list(arguments)
    return function


error = api('SDL_GetError', C.c_char_p)
init = api('SDL_Init', C.c_int, C.c_uint)
create = api('SDL_CreateWindow', C.c_void_p, C.c_char_p, C.c_int, C.c_int, C.c_int, C.c_int, C.c_uint)
context = api('SDL_GL_CreateContext', C.c_void_p, C.c_void_p)
interval = api('SDL_GL_SetSwapInterval', C.c_int, C.c_int)
proc = api('SDL_GL_GetProcAddress', C.c_void_p, C.c_char_p)
swap = api('SDL_GL_SwapWindow', None, C.c_void_p)
driver = api('SDL_GetCurrentVideoDriver', C.c_char_p)
poll = api('SDL_PollEvent', C.c_int, C.c_void_p)
delete = api('SDL_GL_DeleteContext', None, C.c_void_p)
destroy = api('SDL_DestroyWindow', None, C.c_void_p)
quit_sdl = api('SDL_Quit', None)
window = gl = None
try:
    if init(0x20):
        raise RuntimeError(error().decode())
    window = create(b'Portal compatibility: SDL/OpenGL', 0x1FFF0000, 0x1FFF0000, 720, 480, 0x26)
    if not window:
        raise RuntimeError(error().decode())
    gl = context(window)
    if not gl:
        raise RuntimeError(error().decode())
    interval(1)
    get_string = C.CFUNCTYPE(C.c_char_p, C.c_uint)(proc(b'glGetString'))
    clear_color = C.CFUNCTYPE(None, C.c_float, C.c_float, C.c_float, C.c_float)(proc(b'glClearColor'))
    clear = C.CFUNCTYPE(None, C.c_uint)(proc(b'glClear'))
    event = C.create_string_buffer(64)
    start, start_ns, frames = time.monotonic(), time.monotonic_ns(), 0
    while time.monotonic() - start < args.seconds:
        while poll(event):
            pass
        clear_color((frames % 120) / 120, 0.2, 0.4, 1)
        clear(0x4000)
        swap(window)
        frames += 1
    print(json.dumps(dict(toolkit='SDL2', backend=driver().decode(), frames=frames,
                          renderer=get_string(0x1F01).decode(), version=get_string(0x1F02).decode(),
                          flow_start_ns=start_ns, flow_end_ns=time.monotonic_ns())), flush=True)
finally:
    if gl:
        delete(gl)
    if window:
        destroy(window)
    quit_sdl()
