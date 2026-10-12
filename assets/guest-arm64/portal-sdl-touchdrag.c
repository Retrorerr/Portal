/*
 * Managed by Portal: lets a mouse or a finger drag Steam's window by its own
 * title bar.
 *
 * Steam draws its own title bar. CEF reports the page's draggable regions
 * (-webkit-app-region) to steamwebhelper, which answers SDL's hit test from
 * them (SDL_SetWindowHitTest) and is meant to cut the same regions out of the
 * input shape of CEF's X window, so presses there fall through to the SDL
 * window underneath and SDL starts the window move. The shaping step is gated
 * on a per-window flag that this webhelper build never sets, so CEF takes
 * every press and the title bar does nothing, for a mouse as much as a finger.
 *
 * This preload, inside steamwebhelper only:
 *
 *   - finds the shaping function next to the hit-test callback by its
 *     instruction pattern, reads the flag's offset from it and sets the flag
 *     on every window that registers a hit test. If the pattern is not found
 *     (a Steam update changed the code) it leaves Steam alone;
 *   - watches SDL's events: SDL applies hit tests to mouse presses only, so a
 *     finger landing in a draggable (or resize) area gets the same
 *     _NET_WM_MOVERESIZE request SDL sends for the mouse. KWin then moves the
 *     window with the finger that is down.
 *
 * Build (guest-native ARM64 glibc, no libc at link time):
 *   clang --target=aarch64-linux-gnu -shared -fPIC -nostdlib -O2 \
 *       -fuse-ld=lld -ffreestanding -fno-stack-protector \
 *       -o portal-sdl-touchdrag.so portal-sdl-touchdrag.c
 */
#include <stdbool.h>
#include <stdint.h>

#define RTLD_NOW 2
#define RTLD_NOLOAD 4
#define RTLD_NEXT ((void *)-1l)

extern void *dlsym(void *handle, const char *symbol);
extern void *dlopen(const char *file, int mode);
extern long readlink(const char *path, char *buf, unsigned long size);

/* elf.h / link.h (64-bit) */
#define PT_LOAD 1
#define PF_X 1
#define PF_R 4

typedef struct {
    uint32_t p_type, p_flags;
    uint64_t p_offset, p_vaddr, p_paddr, p_filesz, p_memsz, p_align;
} Elf64_Phdr;

struct dl_phdr_info {
    uint64_t dlpi_addr;
    const char *dlpi_name;
    const Elf64_Phdr *dlpi_phdr;
    uint16_t dlpi_phnum;
};

extern int dl_iterate_phdr(int (*callback)(struct dl_phdr_info *, unsigned long, void *),
                           void *data);

/* SDL3 (SDL_video.h, SDL_events.h). */
typedef struct SDL_Window SDL_Window;
typedef struct { int x, y; } SDL_Point;
typedef int (*SDL_HitTest)(SDL_Window *, const SDL_Point *, void *);

#define SDL_HITTEST_DRAGGABLE 1
#define SDL_HITTEST_RESIZE_TOPLEFT 2
#define SDL_HITTEST_RESIZE_LEFT 9
#define SDL_EVENT_FINGER_DOWN 0x700

typedef struct {
    uint32_t type;
    uint32_t reserved;
    uint64_t timestamp;
    uint64_t touch_id;
    uint64_t finger_id;
    float x, y; /* normalized to the window */
    float dx, dy;
    float pressure;
    uint32_t window_id;
} SDL_TouchFingerEvent;

/* ClientMessage, padded to sizeof(XEvent). */
typedef struct {
    int type;
    unsigned long serial;
    int send_event;
    void *display;
    unsigned long window;
    unsigned long message_type;
    int format;
    long l[5];
    long pad[12];
} XClientMessageEvent;

#define CLIENT_MESSAGE 33
#define SUBSTRUCTURE_NOTIFY_MASK (1l << 19)
#define SUBSTRUCTURE_REDIRECT_MASK (1l << 20)
#define NET_WM_MOVERESIZE_MOVE 8
#define SOURCE_APPLICATION 1

#define MAX_WINDOWS 64

static struct {
    SDL_Window *window;
    SDL_HitTest callback;
    void *data;
} windows[MAX_WINDOWS];

/* The next definition of a symbol. A library opened with RTLD_LOCAL is not
 * in the global scope, so fall back to the already loaded library by name. */
static void *next(const char *name, const char *library) {
    void *symbol = dlsym(RTLD_NEXT, name);
    if (!symbol) {
        void *handle = dlopen(library, RTLD_NOW | RTLD_NOLOAD);
        if (handle)
            symbol = dlsym(handle, name);
    }
    return symbol;
}

#define SDL3 "libSDL3.so.0"
#define X11 "libX11.so.6"

static int active = -1;

static int is_webhelper(void) {
    if (active < 0) {
        static const char name[] = "/steamwebhelper";
        const long len = sizeof name - 1;
        char exe[512];
        long n = readlink("/proc/self/exe", exe, sizeof exe);
        active = n >= len;
        for (long i = 0; active && i < len; i++)
            if (exe[n - len + i] != name[i])
                active = 0;
    }
    return active;
}

/* The readable code segment around an address. */
struct segment {
    uintptr_t addr, start, end;
};

static int find_segment(struct dl_phdr_info *info, unsigned long size, void *data) {
    (void)size;
    struct segment *segment = data;
    for (unsigned i = 0; i < info->dlpi_phnum; i++) {
        const Elf64_Phdr *phdr = &info->dlpi_phdr[i];
        uintptr_t start = info->dlpi_addr + phdr->p_vaddr;
        if (phdr->p_type != PT_LOAD || segment->addr < start ||
            segment->addr >= start + phdr->p_memsz)
            continue;
        if ((phdr->p_flags & (PF_X | PF_R)) != (PF_X | PF_R))
            return -1;
        segment->start = start;
        segment->end = start + phdr->p_memsz;
        return 1;
    }
    return 0;
}

/*
 * The shaping function opens with
 *     ldrb w1, [x0, #flag]
 *     cbz  w1, <return>
 *     sub  sp, sp, #0x100
 * and sits next to the hit-test callback in the webhelper's text. Returns the
 * flag's offset in the window object, or 0 when the pattern is not there.
 * The scan stays inside the callback's own code segment, so a Steam build
 * that moved the code cannot make it read unmapped memory.
 */
static unsigned long shape_flag_offset(SDL_HitTest callback) {
    static long offset = -1;
    if (offset >= 0)
        return offset;
    offset = 0;
    struct segment segment = {(uintptr_t)callback & ~(uintptr_t)3, 0, 0};
    if (dl_iterate_phdr(find_segment, &segment) != 1)
        return offset;
    const uint32_t *code = (const uint32_t *)segment.addr;
    long first = -0x4000, last = 0x4000;
    long before = (long)((segment.addr - segment.start) / 4);
    long after = (long)((segment.end - segment.addr) / 4) - 2; /* room for code[i + 2] */
    if (first < -before)
        first = -before;
    if (last > after)
        last = after;
    for (long i = first; i < last; i++) {
        if ((code[i] & 0xffc003ff) == 0x39400001 &&      /* ldrb w1, [x0, #imm] */
            (code[i + 1] & 0xff00001f) == 0x34000001 &&  /* cbz w1 */
            code[i + 2] == 0xd10403ff) {                 /* sub sp, sp, #0x100 */
            offset = (code[i] >> 10) & 0xfff;
            break;
        }
    }
    return offset;
}

static void enable_shaping(unsigned slot) {
    unsigned long offset = shape_flag_offset(windows[slot].callback);
    if (offset && windows[slot].data)
        ((volatile unsigned char *)windows[slot].data)[offset] = 1;
}

static void request_move(void *display, unsigned long window, int x, int y, long direction) {
    unsigned long (*root_window)(void *) = next("XDefaultRootWindow", X11);
    unsigned long (*intern_atom)(void *, const char *, int) = next("XInternAtom", X11);
    int (*ungrab)(void *, unsigned long) = next("XUngrabPointer", X11);
    int (*send)(void *, unsigned long, int, long, void *) = next("XSendEvent", X11);
    int (*flush)(void *) = next("XFlush", X11);
    if (!root_window || !intern_atom || !ungrab || !send || !flush)
        return;

    XClientMessageEvent event = {0};
    event.type = CLIENT_MESSAGE;
    event.window = window;
    event.message_type = intern_atom(display, "_NET_WM_MOVERESIZE", 0);
    event.format = 32;
    event.l[0] = x;
    event.l[1] = y;
    event.l[2] = direction;
    event.l[3] = 1; /* Button1, as SDL sends for the mouse */
    event.l[4] = SOURCE_APPLICATION;
    ungrab(display, 0);
    send(display, root_window(display), 0,
         SUBSTRUCTURE_REDIRECT_MASK | SUBSTRUCTURE_NOTIFY_MASK, &event);
    flush(display);
}

static void finger_down(const SDL_TouchFingerEvent *finger) {
    static SDL_Window *(*from_id)(uint32_t);
    static bool (*get_size)(SDL_Window *, int *, int *);
    static bool (*get_position)(SDL_Window *, int *, int *);
    static uint32_t (*get_props)(SDL_Window *);
    static void *(*get_pointer)(uint32_t, const char *, void *);
    static int64_t (*get_number)(uint32_t, const char *, int64_t);
    if (!from_id) {
        from_id = next("SDL_GetWindowFromID", SDL3);
        get_size = next("SDL_GetWindowSize", SDL3);
        get_position = next("SDL_GetWindowPosition", SDL3);
        get_props = next("SDL_GetWindowProperties", SDL3);
        get_pointer = next("SDL_GetPointerProperty", SDL3);
        get_number = next("SDL_GetNumberProperty", SDL3);
    }
    if (!from_id || !get_size || !get_position || !get_props || !get_pointer || !get_number)
        return;

    SDL_Window *window = from_id(finger->window_id);
    for (unsigned i = 0; window && i < MAX_WINDOWS; i++) {
        if (windows[i].window != window)
            continue;
        int w, h, x, y;
        if (!get_size(window, &w, &h) || !get_position(window, &x, &y))
            return;
        SDL_Point point = {(int)(finger->x * w), (int)(finger->y * h)};
        int area = windows[i].callback(window, &point, windows[i].data);
        long direction = -1;
        if (area == SDL_HITTEST_DRAGGABLE)
            direction = NET_WM_MOVERESIZE_MOVE;
        else if (area >= SDL_HITTEST_RESIZE_TOPLEFT && area <= SDL_HITTEST_RESIZE_LEFT)
            /* SDL and EWMH list the eight edges in the same order. */
            direction = area - SDL_HITTEST_RESIZE_TOPLEFT;
        if (direction < 0)
            return;
        uint32_t props = get_props(window);
        void *display = get_pointer(props, "SDL.window.x11.display", 0);
        unsigned long xwindow = (unsigned long)get_number(props, "SDL.window.x11.window", 0);
        if (display && xwindow)
            request_move(display, xwindow, x + point.x, y + point.y, direction);
        return;
    }
}

/* Runs on SDL's event thread, where the window objects are used. */
static bool watch(void *userdata, void *event) {
    (void)userdata;
    if (*(uint32_t *)event == SDL_EVENT_FINGER_DOWN)
        finger_down(event);
    /* Steam clears the flag again when it rebuilds a window's browser. */
    for (unsigned i = 0; i < MAX_WINDOWS; i++)
        if (windows[i].window)
            enable_shaping(i);
    return true;
}

/* A destroyed window's address can come back for a new window. */
void SDL_DestroyWindow(SDL_Window *window) {
    void (*real)(SDL_Window *) = next("SDL_DestroyWindow", SDL3);
    for (unsigned i = 0; i < MAX_WINDOWS; i++)
        if (windows[i].window == window)
            windows[i].window = 0;
    if (real)
        real(window);
}

/* SDL3 returns bool, SDL2 int: both come back in w0 unchanged. */
int SDL_SetWindowHitTest(SDL_Window *window, SDL_HitTest callback, void *data) {
    int (*real)(SDL_Window *, SDL_HitTest, void *) = next("SDL_SetWindowHitTest", SDL3);
    if (!real)
        return 0;
    int result = real(window, callback, data);
    if (!is_webhelper())
        return result;

    static bool watching;
    if (!watching) {
        bool (*add_watch)(bool (*)(void *, void *), void *) = next("SDL_AddEventWatch", SDL3);
        watching = add_watch && add_watch(watch, 0);
    }

    unsigned slot = MAX_WINDOWS;
    for (unsigned i = 0; i < MAX_WINDOWS; i++) {
        if (windows[i].window == window) {
            slot = i;
            break;
        }
        if (!windows[i].window && slot == MAX_WINDOWS)
            slot = i;
    }
    if (slot < MAX_WINDOWS) {
        windows[slot].window = callback ? window : 0;
        windows[slot].callback = callback;
        windows[slot].data = data;
        if (callback)
            enable_shaping(slot);
    }
    return result;
}
