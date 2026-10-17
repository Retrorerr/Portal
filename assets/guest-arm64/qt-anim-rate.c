/*
 * Portal Qt animation rate — guest-native ARM64 glibc preload for Qt 6.
 *
 * Qt drives QPropertyAnimation/QVariantAnimation (every Qt Widgets smooth
 * scroll, KItemListView kinetic scrolling in Dolphin, Breeze widget
 * transitions) from QUnifiedTimer's fixed 16 ms timer, i.e. 60 Hz whatever
 * the display runs at. On a 144 Hz output a 60 Hz animation lands on 2 or 3
 * vsyncs per frame in turn, which reads as judder (device-measured: Dolphin
 * fling scrolling committed ~65 frames/s on a 144 Hz session). Qt Quick
 * scenes are unaffected: their render loop drives animations from vsync.
 *
 * Every Qt executable enters its event loop through Q*Application::exec(),
 * a cross-library call that goes through the PLT, so this shim interposes
 * the three exec() entry points and, once, before the loop starts, sets the
 * main thread's QUnifiedTimer interval to 6 ms (166 Hz) when the primary
 * screen runs above 60 Hz; 60 Hz outputs keep Qt's 16 ms default. One value
 * for every high-refresh rate on purpose: the output rate can change after
 * an app started (per-app refresh overrides land just after Portal resumes,
 * e.g. 120 -> 144 Hz, and Portal then re-advertises the rate), and a tick
 * derived from the startup rate would then judder. Animation values are
 * computed from elapsed time, so ticking faster than vsync only means each
 * vsync sees a fresh frame; Qt's update() coalescing and Wayland frame
 * callbacks keep paints and commits at the display rate.
 *
 * The session's LD_PRELOAD is inherited by every process, so the shim has
 * no Qt DT_NEEDED: Qt symbols are looked up with dlsym only inside a hook,
 * which only runs inside a Qt process. PORTAL_QT_ANIMATION_INTERVAL_MS
 * overrides the interval (0 disables the shim).
 *
 * Build (in the guest, Debian trixie gcc):
 *   gcc -O2 -fPIC -shared -Wl,-z,lazy -s qt-anim-rate.c -o qt-anim-rate.so -ldl
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdlib.h>

#define SYM_CORE_EXEC "_ZN16QCoreApplication4execEv"
#define SYM_GUI_EXEC "_ZN15QGuiApplication4execEv"
#define SYM_WIDGETS_EXEC "_ZN12QApplication4execEv"
#define SYM_TIMER_INSTANCE "_ZN13QUnifiedTimer8instanceEb"
#define SYM_TIMER_SET_INTERVAL "_ZN13QUnifiedTimer17setTimingIntervalEi"
#define SYM_PRIMARY_SCREEN "_ZN15QGuiApplication13primaryScreenEv"
#define SYM_REFRESH_RATE "_ZNK7QScreen11refreshRateEv"

typedef int (*exec_fn)(void);
typedef void *(*timer_instance_fn)(int create);
typedef void (*timer_set_interval_fn)(void *timer, int interval);
typedef void *(*primary_screen_fn)(void);
typedef double (*refresh_rate_fn)(const void *screen);

static int tuned;

static int interval_from_env(int *out)
{
    const char *value = getenv("PORTAL_QT_ANIMATION_INTERVAL_MS");
    if (!value || !*value)
        return 0;
    *out = atoi(value);
    return 1;
}

static int interval_from_screen(void)
{
    primary_screen_fn primary = (primary_screen_fn)dlsym(RTLD_DEFAULT, SYM_PRIMARY_SCREEN);
    refresh_rate_fn refresh = (refresh_rate_fn)dlsym(RTLD_DEFAULT, SYM_REFRESH_RATE);
    if (!primary || !refresh)
        return 0; /* QtCore-only process: no animations worth retiming. */
    void *screen = primary();
    if (!screen)
        return 0;
    double hz = refresh(screen);
    if (!(hz > 1.0) || hz > 1000.0)
        return 0;
    return hz > 61.0 ? 6 : 0;
}

static void tune_animation_timer(void)
{
    if (tuned)
        return;
    tuned = 1;
    int interval = 0;
    if (!interval_from_env(&interval))
        interval = interval_from_screen();
    if (interval <= 0 || interval >= 16)
        return;
    timer_instance_fn instance = (timer_instance_fn)dlsym(RTLD_DEFAULT, SYM_TIMER_INSTANCE);
    timer_set_interval_fn set_interval =
        (timer_set_interval_fn)dlsym(RTLD_DEFAULT, SYM_TIMER_SET_INTERVAL);
    if (!instance || !set_interval)
        return;
    void *timer = instance(1);
    if (timer)
        set_interval(timer, interval);
}

static int call_next(const char *symbol)
{
    exec_fn next = (exec_fn)dlsym(RTLD_NEXT, symbol);
    if (!next)
        abort(); /* Only reachable from a Qt caller, which linked it. */
    return next();
}

int _ZN16QCoreApplication4execEv(void)
{
    tune_animation_timer();
    return call_next(SYM_CORE_EXEC);
}

int _ZN15QGuiApplication4execEv(void)
{
    tune_animation_timer();
    return call_next(SYM_GUI_EXEC);
}

int _ZN12QApplication4execEv(void)
{
    tune_animation_timer();
    return call_next(SYM_WIDGETS_EXEC);
}
