/*
 * Qt Widgets animation timing for the Debian guest. No Qt DT_NEEDED.
 *
 * Versioned entry points keep Qt 5 and Qt 6 separate even when a host
 * loads one globally and another via a RTLD_LOCAL plugin (VLC's shape).
 * Unknown Qt releases keep their native timer; private APIs are used only
 * for the tested 5.15 and 6.8 series. Qt 4/7 and non-Qt processes are inert.
 *
 * Reproduce with scripts/build_qt_animation.py. The symbol-version map is
 * required. Both versions are non-default and version index 2 is empty:
 * unversioned Qt 4/bundled builds must resolve their native entry points.
 */
#define RTLD_LAZY 1
#define RTLD_NOLOAD 4
#define RTLD_NEXT ((void *)-1l)
extern void *dlvsym(void *, const char *, const char *);
extern void *dlopen(const char *, int);
extern int dlclose(void *);
extern int strncmp(const char *, const char *, unsigned long);
extern const char *strrchr(const char *, int);
extern char *getenv(const char *);
extern int atoi(const char *);

#define CORE_EXEC "_ZN16QCoreApplication4execEv"
#define GUI_EXEC "_ZN15QGuiApplication4execEv"
#define WIDGETS_EXEC "_ZN12QApplication4execEv"
#define TIMER_INSTANCE "_ZN13QUnifiedTimer8instanceEb"
#define TIMER_INTERVAL "_ZN13QUnifiedTimer17setTimingIntervalEi"
#define PRIMARY_SCREEN "_ZN15QGuiApplication13primaryScreenEv"
#define REFRESH_RATE "_ZNK7QScreen11refreshRateEv"

/* qVersion() is C++-mangled in Qt 5 and extern "C" since Qt 6. */
struct runtime {
    const char *tag, *private_tag, *version;
    const char *libraries[3];
    int major, minor, tuned;
};
static struct runtime qt5 = {"Qt_5", "Qt_5_PRIVATE_API", "_Z8qVersionv",
    {"libQt5Core.so.5", "libQt5Gui.so.5", "libQt5Widgets.so.5"}, 5, 15, 0};
static struct runtime qt6 = {"Qt_6", "Qt_6_PRIVATE_API", "qVersion",
    {"libQt6Core.so.6", "libQt6Gui.so.6", "libQt6Widgets.so.6"}, 6, 8, 0};

/* Stable public prefix of glibc's dl_phdr_info on the shipped 64-bit ABIs.
 * No Qt headers or DT_NEEDED libraries enter the preload. */
struct loaded_image { unsigned long address; const char *name; };
extern int dl_iterate_phdr(int (*)(struct loaded_image *, unsigned long, void *), void *);
struct lookup {
    const char *prefix;
    unsigned long length, skip, seen;
    char path[4096];
};

static int bundled_symbol(struct loaded_image *image, unsigned long size, void *data)
{
    struct lookup *lookup = data;
    if (size < sizeof(*image) || !image->name || !*image->name)
        return 0;
    const char *base = strrchr(image->name, '/');
    base = base ? base + 1 : image->name;
    if (strncmp(base, lookup->prefix, lookup->length))
        return 0;
    if (lookup->seen++ != lookup->skip)
        return 0;
    unsigned long index = 0;
    while (image->name[index] && index + 1 < sizeof(lookup->path)) {
        lookup->path[index] = image->name[index];
        index++;
    }
    if (image->name[index])
        return 0;
    lookup->path[index] = 0;
    return 1;
}

static void *symbol(struct runtime *rt, int library, const char *name, const char *tag)
{
    void *result = dlvsym(RTLD_NEXT, name, tag);
    if (!result) {
        void *handle = dlopen(rt->libraries[library], RTLD_LAZY | RTLD_NOLOAD);
        if (handle) {
            result = dlvsym(handle, name, tag);
            dlclose(handle);
        }
    }
    if (!result) {
        /* Wheels and portable bundles may rename Qt's SONAME. Their
         * RTLD_LOCAL dependencies are invisible to RTLD_NEXT; resolve in
         * the already-loaded matching component without loading a second Qt. */
        struct lookup lookup = {rt->libraries[library], 0, 0, 0, {0}};
        while (lookup.prefix[lookup.length] && lookup.prefix[lookup.length] != '.')
            lookup.length++;
        do {
            lookup.seen = 0;
            lookup.path[0] = 0;
            dl_iterate_phdr(bundled_symbol, &lookup);
            if (!lookup.path[0])
                break;
            /* Loader operations run after the iteration lock is released. */
            void *handle = dlopen(lookup.path, RTLD_LAZY | RTLD_NOLOAD);
            if (handle) {
                result = dlvsym(handle, name, tag);
                dlclose(handle);
            }
            lookup.skip++;
        } while (!result);
    }
    return result;
}

static int tested_version(struct runtime *rt)
{
    const char *(*version)(void) = symbol(rt, 0, rt->version, rt->tag);
    if (!version)
        return 0;
    const char *p = version();
    if (!p)
        return 0;
    int major = atoi(p);
    while (*p >= '0' && *p <= '9') ++p;
    return *p == '.' && major == rt->major && atoi(p + 1) == rt->minor;
}

static void tune(struct runtime *rt)
{
    if (rt->tuned || !tested_version(rt))
        return;
    rt->tuned = 1;
    int interval = 0;
    const char *override = getenv("PORTAL_QT_ANIMATION_INTERVAL_MS");
    if (override) {
        interval = atoi(override);
    } else {
        void *(*primary)(void) = symbol(rt, 1, PRIMARY_SCREEN, rt->tag);
        double (*refresh)(const void *) = symbol(rt, 1, REFRESH_RATE, rt->tag);
        void *screen = primary ? primary() : (void *)0;
        if (screen && refresh) {
            double hz = refresh(screen);
            if (hz > 61.0 && hz < 1000.0)
                interval = 6;
        }
    }
    if (interval <= 0 || interval >= 16)
        return;
    void *(*instance)(int) = symbol(rt, 0, TIMER_INSTANCE, rt->private_tag);
    void (*set_interval)(void *, int) = symbol(rt, 0, TIMER_INTERVAL, rt->private_tag);
    if (instance && set_interval) {
        void *timer = instance(1);
        if (timer)
            set_interval(timer, interval);
    }
}

static int execute(struct runtime *rt, int library, const char *name)
{
    int (*next)(void) = symbol(rt, library, name, rt->tag);
    if (!next)
        return 127;
    tune(rt);
    return next();
}

#define ENTRY(name, rt, library, target) \
    int name(void) { return execute(&rt, library, target); }
ENTRY(portal_qt5_core_exec, qt5, 0, CORE_EXEC)
ENTRY(portal_qt5_gui_exec, qt5, 1, GUI_EXEC)
ENTRY(portal_qt5_widgets_exec, qt5, 2, WIDGETS_EXEC)
ENTRY(portal_qt6_core_exec, qt6, 0, CORE_EXEC)
ENTRY(portal_qt6_gui_exec, qt6, 1, GUI_EXEC)
ENTRY(portal_qt6_widgets_exec, qt6, 2, WIDGETS_EXEC)

__asm__(".symver portal_qt5_core_exec," CORE_EXEC "@Qt_5");
__asm__(".symver portal_qt5_gui_exec," GUI_EXEC "@Qt_5");
__asm__(".symver portal_qt5_widgets_exec," WIDGETS_EXEC "@Qt_5");
__asm__(".symver portal_qt6_core_exec," CORE_EXEC "@Qt_6");
__asm__(".symver portal_qt6_gui_exec," GUI_EXEC "@Qt_6");
__asm__(".symver portal_qt6_widgets_exec," WIDGETS_EXEC "@Qt_6");
