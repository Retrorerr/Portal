#!/usr/bin/python3
"""Exercise SDL's selected video backend and its real OpenGL context."""
import argparse
import ctypes as C
import json
import time

parser = argparse.ArgumentParser()
parser.add_argument('--seconds', type=float, default=4)
parser.add_argument('--visual-pattern', type=json.loads, help='Draw the capture reference in fullscreen')
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
drawable_size = api('SDL_GL_GetDrawableSize', None, C.c_void_p, C.POINTER(C.c_int), C.POINTER(C.c_int))
show_cursor = api('SDL_ShowCursor', C.c_int, C.c_int)
stop_text_input = api('SDL_StopTextInput', None)
delete = api('SDL_GL_DeleteContext', None, C.c_void_p)
destroy = api('SDL_DestroyWindow', None, C.c_void_p)
quit_sdl = api('SDL_Quit', None)


def render_pattern(window, clear_color, clear, scissor, phase):
    width, height = C.c_int(), C.c_int()
    drawable_size(window, C.byref(width), C.byref(height))
    for tile, index in enumerate(args.visual_pattern['tiles']):
        if tile == args.visual_pattern['phase_tile']:
            index = phase
        x, y = tile % 4, 3 - tile // 4  # OpenGL's origin is at the bottom.
        x0, x1 = width.value * x // 4, width.value * (x + 1) // 4
        y0, y1 = height.value * y // 4, height.value * (y + 1) // 4
        scissor(x0, y0, x1 - x0, y1 - y0)
        clear_color(*(component / 255 for component in args.visual_pattern['palette'][index]), 1)
        clear(0x4000)  # GL_COLOR_BUFFER_BIT


window = gl = None
try:
    if init(0x20):
        raise RuntimeError(error().decode())
    flags = 0x26
    if args.visual_pattern:
        flags |= 0x1001 | 0x2000  # SDL_WINDOW_FULLSCREEN_DESKTOP | SDL_WINDOW_ALLOW_HIGHDPI
    window = create(b'Portal compatibility: SDL/OpenGL', 0x1FFF0000, 0x1FFF0000, 720, 480, flags)
    if not window:
        raise RuntimeError(error().decode())
    gl = context(window)
    if not gl:
        raise RuntimeError(error().decode())
    interval(1)
    get_string = C.CFUNCTYPE(C.c_char_p, C.c_uint)(proc(b'glGetString'))
    clear_color = C.CFUNCTYPE(None, C.c_float, C.c_float, C.c_float, C.c_float)(proc(b'glClearColor'))
    clear = C.CFUNCTYPE(None, C.c_uint)(proc(b'glClear'))
    backend = driver().decode()
    event = C.create_string_buffer(64)
    if args.visual_pattern:
        show_cursor(0)
        # The reference scene has no text fields. SDL starts text input by
        # default, which can put Portal's keyboard over the sampled tiles.
        stop_text_input()
        if backend == 'x11':
            # KWin may animate the initially mapped X11 window into
            # fullscreen. Give that transition a neutral frame before
            # showing the pixel-check markers.
            deadline = time.monotonic() + 0.6
            clear_color(0.1, 0.1, 0.1, 1)
            while time.monotonic() < deadline:
                while poll(event):
                    pass
                clear(0x4000)
                swap(window)
        scissor = C.CFUNCTYPE(None, C.c_int, C.c_int, C.c_int, C.c_int)(proc(b'glScissor'))
        enable = C.CFUNCTYPE(None, C.c_uint)(proc(b'glEnable'))
        enable(0x0C11)  # GL_SCISSOR_TEST
        phases = args.visual_pattern['phases']
        marker_seconds = 0.4
    start, start_ns, frames = time.monotonic(), time.monotonic_ns(), 0
    while (elapsed := time.monotonic() - start) < args.seconds:
        while poll(event):
            pass
        if args.visual_pattern:
            if elapsed < marker_seconds:
                phase = args.visual_pattern['start']
            elif elapsed >= args.seconds - marker_seconds:
                phase = args.visual_pattern['end']
            else:
                phase = phases[int((elapsed - marker_seconds) * 4) % len(phases)]
            render_pattern(window, clear_color, clear, scissor, phase)
        else:
            clear_color((frames % 120) / 120, 0.2, 0.4, 1)
            clear(0x4000)
        swap(window)
        frames += 1
    print(json.dumps(dict(toolkit='SDL2', backend=backend, frames=frames,
                          renderer=get_string(0x1F01).decode(), version=get_string(0x1F02).decode(),
                          flow_start_ns=start_ns, flow_end_ns=time.monotonic_ns())), flush=True)
finally:
    if gl:
        delete(gl)
    if window:
        destroy(window)
    quit_sdl()
