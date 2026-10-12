/*
 * Managed by Portal: Steam's tray icon.
 *
 * Steam's Linux client shows its tray icon only through libappindicator.so.1
 * (the GTK2 build), which it dlopens next to libgtk-x11-2.0. Debian trixie
 * ships only the GTK3 libayatana-appindicator3, which cannot load into a GTK2
 * process, so Steam silently runs without a tray icon.
 *
 * This library provides the four calls Steam makes with a GTK2 GtkStatusIcon
 * (an XEmbed tray icon); Plasma's xembedsniproxy shows it in the system tray.
 * The indicator handed back is the GtkStatusIcon itself, so a g_object_unref
 * from the caller stays valid. Both mouse buttons open the indicator's menu,
 * as they do for a real AppIndicator.
 *
 * Build (guest-native ARM64 glibc, no libc at link time):
 *   aarch64-linux-gnu-gcc -shared -fPIC -nostdlib -O2 -ffreestanding \
 *       -fno-stack-protector -Wl,-soname,libappindicator.so.1 \
 *       -Wl,-z,max-page-size=4096 -Wl,-s \
 *       -o portal-appindicator.so portal-appindicator.c
 */
#include <stddef.h>

#define RTLD_NOW 2
#define RTLD_GLOBAL 0x100

extern void *dlopen(const char *file, int mode);
extern void *dlsym(void *handle, const char *symbol);
extern int access(const char *path, int mode);

typedef void *gpointer;
typedef int gboolean;
typedef unsigned int guint;
typedef void (*GCallback)(void);

/* libappindicator AppIndicatorStatus */
enum { APP_INDICATOR_STATUS_PASSIVE, APP_INDICATOR_STATUS_ACTIVE, APP_INDICATOR_STATUS_ATTENTION };

static struct {
    int loaded;
    gpointer (*status_icon_new_from_file)(const char *);
    gpointer (*status_icon_new_from_icon_name)(const char *);
    void (*status_icon_set_visible)(gpointer, gboolean);
    void (*status_icon_set_tooltip_text)(gpointer, const char *);
    void (*status_icon_set_title)(gpointer, const char *);
    void (*status_icon_set_name)(gpointer, const char *);
    void (*status_icon_position_menu)(gpointer, int *, int *, gboolean *, gpointer);
    void (*menu_popup)(gpointer, gpointer, gpointer, gpointer, gpointer, guint, unsigned int);
    void (*widget_show_all)(gpointer);
    unsigned int (*get_current_event_time)(void);
    unsigned long (*signal_connect_data)(gpointer, const char *, GCallback, gpointer, gpointer, int);
    void (*object_set_data)(gpointer, const char *, gpointer);
    gpointer (*object_get_data)(gpointer, const char *);
    gpointer (*object_ref)(gpointer);
} g;

static int load(void) {
    if (g.loaded) return g.loaded > 0;
    void *gtk = dlopen("libgtk-x11-2.0.so.0", RTLD_NOW | RTLD_GLOBAL);
    void *gobj = dlopen("libgobject-2.0.so.0", RTLD_NOW | RTLD_GLOBAL);
    g.loaded = -1;
    if (!gtk || !gobj) return 0;
#define SYM(field, lib, name) if (!(*(void **)&g.field = dlsym(lib, name))) return 0
    SYM(status_icon_new_from_file, gtk, "gtk_status_icon_new_from_file");
    SYM(status_icon_new_from_icon_name, gtk, "gtk_status_icon_new_from_icon_name");
    SYM(status_icon_set_visible, gtk, "gtk_status_icon_set_visible");
    SYM(status_icon_set_tooltip_text, gtk, "gtk_status_icon_set_tooltip_text");
    SYM(status_icon_set_title, gtk, "gtk_status_icon_set_title");
    SYM(status_icon_set_name, gtk, "gtk_status_icon_set_name");
    SYM(status_icon_position_menu, gtk, "gtk_status_icon_position_menu");
    SYM(menu_popup, gtk, "gtk_menu_popup");
    SYM(widget_show_all, gtk, "gtk_widget_show_all");
    SYM(get_current_event_time, gtk, "gtk_get_current_event_time");
    SYM(signal_connect_data, gobj, "g_signal_connect_data");
    SYM(object_set_data, gobj, "g_object_set_data");
    SYM(object_get_data, gobj, "g_object_get_data");
    SYM(object_ref, gobj, "g_object_ref");
#undef SYM
    g.loaded = 1;
    return 1;
}

static void show_menu(gpointer icon, guint button, unsigned int time) {
    gpointer menu = g.object_get_data(icon, "portal-menu");
    if (!menu) return;
    g.widget_show_all(menu);
    g.menu_popup(menu, NULL, NULL, (gpointer)g.status_icon_position_menu, icon, button, time);
}

static void on_activate(gpointer icon, gpointer data) {
    (void)data;
    show_menu(icon, 1, g.get_current_event_time());
}

static void on_popup_menu(gpointer icon, guint button, unsigned int time, gpointer data) {
    (void)data;
    show_menu(icon, button, time);
}

static size_t length(const char *s) {
    size_t n = 0;
    while (s[n]) n++;
    return n;
}

gpointer app_indicator_new_with_path(const char *id, const char *icon_name, int category,
                                     const char *icon_theme_path) {
    (void)category;
    if (!load()) return NULL;

    gpointer icon = NULL;
    char file[1024];
    if (icon_name && icon_name[0] == '/' && access(icon_name, 4) == 0) {
        icon = g.status_icon_new_from_file(icon_name);
    } else if (icon_name && icon_theme_path) {
        size_t a = length(icon_theme_path), b = length(icon_name);
        if (a + b + 6 < sizeof file) {
            char *p = file;
            for (size_t i = 0; i < a; i++) *p++ = icon_theme_path[i];
            *p++ = '/';
            for (size_t i = 0; i < b; i++) *p++ = icon_name[i];
            const char *ext = ".png";
            int has_ext = b > 4 && icon_name[b - 4] == '.';
            if (!has_ext) for (size_t i = 0; i < 4; i++) *p++ = ext[i];
            *p = 0;
            if (access(file, 4) == 0) icon = g.status_icon_new_from_file(file);
        }
    }
    if (!icon) icon = g.status_icon_new_from_icon_name(icon_name ? icon_name : "steam");
    if (!icon) return NULL;

    if (id) g.status_icon_set_name(icon, id);
    g.status_icon_set_visible(icon, 0);
    g.signal_connect_data(icon, "activate", (GCallback)on_activate, NULL, NULL, 0);
    g.signal_connect_data(icon, "popup-menu", (GCallback)on_popup_menu, NULL, NULL, 0);
    return icon;
}

gpointer app_indicator_new(const char *id, const char *icon_name, int category) {
    return app_indicator_new_with_path(id, icon_name, category, NULL);
}

void app_indicator_set_status(gpointer icon, int status) {
    if (icon && load()) g.status_icon_set_visible(icon, status != APP_INDICATOR_STATUS_PASSIVE);
}

void app_indicator_set_title(gpointer icon, const char *title) {
    if (!icon || !load() || !title) return;
    g.status_icon_set_title(icon, title);
    g.status_icon_set_tooltip_text(icon, title);
}

void app_indicator_set_menu(gpointer icon, gpointer menu) {
    if (!icon || !load()) return;
    if (menu) g.object_ref(menu);
    g.object_set_data(icon, "portal-menu", menu);
}
