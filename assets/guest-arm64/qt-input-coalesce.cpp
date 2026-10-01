/*
 * Portal Qt input coalescing — guest-native ARM64 glibc preload for Qt 6
 * Wayland clients that paint synchronously per input event (LibreOffice).
 *
 * LibreOffice's Qt VCL plugin repaints inside every scroll/drag event
 * (Writer: ScrollHdl -> PaintImmediately) and only presents through
 * QWidget::update(), i.e. after Qt's window-system event queue drains
 * (QtFrame::Flush() is a no-op). Qt's Wayland QPA, unlike xcb, never
 * compresses high-frequency input, so while a touchpad scroll or a scrollbar
 * drag streams events LibreOffice repaints the whole window ~60 times a
 * second yet commits 1-4 frames (device-measured: 265 full-window copies/s,
 * 3.6 commits/s).
 *
 * This shim interposes the QWindowSystemInterface entry points that
 * libQt6WaylandClient calls (cross-library, so through the PLT) and holds
 * back mouse moves, touch updates and wheel steps. Everything that arrives in
 * the same Wayland dispatch merges into one event (moves/touch: latest
 * position; wheel: summed deltas), delivered from a high-priority GLib idle
 * on the next main-loop iteration — before Qt processes its queue. Each burst
 * costs one repaint and is followed by a present. Any other pointer/touch
 * event flushes the held one first, so ordering is preserved. Off the main
 * thread, events pass through untouched.
 *
 * The launcher's LD_PRELOAD is inherited by every child (sh, oosplash,
 * xdg-open, ...), so the library has no Qt or GLib DT_NEEDED: Qt functions
 * bind lazily through the PLT and only resolve once a hook runs, which only
 * happens inside a Qt process.
 *
 * Build (Debian trixie, g++-aarch64-linux-gnu + qt6-base-private-dev:arm64):
 *   Q=/usr/include/aarch64-linux-gnu/qt6
 *   aarch64-linux-gnu-g++ -O2 -fPIC -shared -std=c++17 -DQT_NO_DEBUG \
 *     -DQT_NO_VERSION_TAGGING -I$Q -I$Q/QtCore -I$Q/QtGui -I$Q/QtGui/6.8.2 \
 *     -I$Q/QtGui/6.8.2/QtGui -I$Q/QtCore/6.8.2 -I$Q/QtCore/6.8.2/QtCore \
 *     -Wl,-z,lazy -s qt-input-coalesce.cpp -o qt-input-coalesce.so -ldl
 */
#include <QtCore/QCoreApplication>
#include <QtCore/QPointer>
#include <QtCore/QThread>
#include <QtGui/QWindow>
#include <qpa/qwindowsysteminterface.h>

#include <dlfcn.h>

using WSI = QWindowSystemInterface;

#define SYM_MOUSE_DEV "_ZN22QWindowSystemInterface16handleMouseEventINS_15DefaultDeliveryEEEbP7QWindowmPK15QPointingDeviceRK7QPointFS9_6QFlagsIN2Qt11MouseButtonEESC_N6QEvent4TypeESA_INSB_16KeyboardModifierEENSB_16MouseEventSourceE"
#define SYM_MOUSE "_ZN22QWindowSystemInterface16handleMouseEventINS_15DefaultDeliveryEEEbP7QWindowmRK7QPointFS6_6QFlagsIN2Qt11MouseButtonEES9_N6QEvent4TypeES7_INS8_16KeyboardModifierEENS8_16MouseEventSourceE"
#define SYM_TOUCH "_ZN22QWindowSystemInterface16handleTouchEventINS_15DefaultDeliveryEEEbP7QWindowmPK15QPointingDeviceRK5QListINS_10TouchPointEE6QFlagsIN2Qt16KeyboardModifierEE"
#define SYM_WHEEL "_ZN22QWindowSystemInterface16handleWheelEventEP7QWindowmPK15QPointingDeviceRK7QPointFS7_6QPointS8_6QFlagsIN2Qt16KeyboardModifierEENSA_11ScrollPhaseENSA_16MouseEventSourceEb"

using MouseDevFn = bool (*)(QWindow *, ulong, const QPointingDevice *, const QPointF &, const QPointF &,
                            Qt::MouseButtons, Qt::MouseButton, QEvent::Type, Qt::KeyboardModifiers,
                            Qt::MouseEventSource);
using MouseFn = bool (*)(QWindow *, ulong, const QPointF &, const QPointF &, Qt::MouseButtons, Qt::MouseButton,
                         QEvent::Type, Qt::KeyboardModifiers, Qt::MouseEventSource);
using TouchFn = bool (*)(QWindow *, ulong, const QPointingDevice *, const QList<WSI::TouchPoint> &,
                         Qt::KeyboardModifiers);
using WheelFn = bool (*)(QWindow *, ulong, const QPointingDevice *, const QPointF &, const QPointF &, QPoint, QPoint,
                         Qt::KeyboardModifiers, Qt::ScrollPhase, Qt::MouseEventSource, bool);

extern "C" {
typedef int gboolean;
typedef gboolean (*GSourceFunc)(void *);
unsigned g_idle_add_full(int priority, GSourceFunc fn, void *data, void (*notify)(void *));
}
// Above G_PRIORITY_DEFAULT (0), where Qt's event sources run.
static constexpr int kFlushPriority = -50;

namespace {

template <typename Fn> Fn real(const char *name)
{
    void *sym = dlvsym(RTLD_NEXT, name, "Qt_6_PRIVATE_API");
    if (!sym)
        sym = dlsym(RTLD_NEXT, name);
    return reinterpret_cast<Fn>(sym);
}

MouseDevFn realMouseDev() { static MouseDevFn fn = real<MouseDevFn>(SYM_MOUSE_DEV); return fn; }
MouseFn realMouse() { static MouseFn fn = real<MouseFn>(SYM_MOUSE); return fn; }
TouchFn realTouch() { static TouchFn fn = real<TouchFn>(SYM_TOUCH); return fn; }
WheelFn realWheel() { static WheelFn fn = real<WheelFn>(SYM_WHEEL); return fn; }

enum class Kind { None, MoveDev, Move, Touch, Wheel };

struct Held {
    Kind kind = Kind::None;
    QPointer<QWindow> window;
    ulong timestamp = 0;
    const QPointingDevice *device = nullptr;
    QPointF local, global;
    Qt::MouseButtons buttons;
    Qt::MouseButton button = Qt::NoButton;
    QEvent::Type type = QEvent::None;
    Qt::KeyboardModifiers mods;
    Qt::MouseEventSource source = Qt::MouseEventNotSynthesized;
    QList<WSI::TouchPoint> points;
    QPoint pixel, angle;
    Qt::ScrollPhase phase = Qt::NoScrollPhase;
    bool inverted = false;
};

Held held;
bool idleQueued = false;

bool onMainThread()
{
    // QCoreApplication::self by lookup: a data relocation against QtCore
    // would fail to load in the non-Qt processes that inherit LD_PRELOAD.
    static auto self = reinterpret_cast<QCoreApplication **>(dlsym(RTLD_DEFAULT, "_ZN16QCoreApplication4selfE"));
    QCoreApplication *app = self ? *self : nullptr;
    return app && QThread::currentThread() == app->thread();
}

void flush()
{
    if (held.kind == Kind::None)
        return;
    Held h = std::move(held);
    held = Held{};
    if (!h.window)
        return;
    switch (h.kind) {
    case Kind::MoveDev:
        realMouseDev()(h.window, h.timestamp, h.device, h.local, h.global, h.buttons, h.button, h.type, h.mods,
                       h.source);
        break;
    case Kind::Move:
        realMouse()(h.window, h.timestamp, h.local, h.global, h.buttons, h.button, h.type, h.mods, h.source);
        break;
    case Kind::Touch:
        realTouch()(h.window, h.timestamp, h.device, h.points, h.mods);
        break;
    case Kind::Wheel:
        realWheel()(h.window, h.timestamp, h.device, h.local, h.global, h.pixel, h.angle, h.mods, h.phase, h.source,
                    h.inverted);
        break;
    case Kind::None:
        break;
    }
}

gboolean idleFlush(void *)
{
    idleQueued = false;
    flush();
    return 0;
}

void hold()
{
    if (!idleQueued) {
        idleQueued = true;
        g_idle_add_full(kFlushPriority, idleFlush, nullptr, nullptr);
    }
}

bool allUpdated(const QList<WSI::TouchPoint> &points)
{
    for (const auto &p : points)
        if (p.state != QEventPoint::State::Updated && p.state != QEventPoint::State::Stationary)
            return false;
    return !points.isEmpty();
}

bool sameIds(const QList<WSI::TouchPoint> &a, const QList<WSI::TouchPoint> &b)
{
    if (a.size() != b.size())
        return false;
    for (qsizetype i = 0; i < a.size(); ++i)
        if (a[i].id != b[i].id)
            return false;
    return true;
}

} // namespace

extern "C" {

bool portal_mouse_dev(QWindow *window, ulong timestamp, const QPointingDevice *device, const QPointF &local,
                      const QPointF &global, Qt::MouseButtons buttons, Qt::MouseButton button, QEvent::Type type,
                      Qt::KeyboardModifiers mods, Qt::MouseEventSource source) __asm__(SYM_MOUSE_DEV);
bool portal_mouse_dev(QWindow *window, ulong timestamp, const QPointingDevice *device, const QPointF &local,
                      const QPointF &global, Qt::MouseButtons buttons, Qt::MouseButton button, QEvent::Type type,
                      Qt::KeyboardModifiers mods, Qt::MouseEventSource source)
{
    if (!realMouseDev())
        return false;
    if (!onMainThread())
        return realMouseDev()(window, timestamp, device, local, global, buttons, button, type, mods, source);
    if (type == QEvent::MouseMove && held.kind == Kind::MoveDev && held.window == window && held.device == device
        && held.buttons == buttons && held.mods == mods) {
        held.timestamp = timestamp;
        held.local = local;
        held.global = global;
        return true;
    }
    flush();
    if (type != QEvent::MouseMove)
        return realMouseDev()(window, timestamp, device, local, global, buttons, button, type, mods, source);
    held = Held{Kind::MoveDev, window, timestamp, device, local, global, buttons, button, type, mods, source};
    hold();
    return true;
}

bool portal_mouse(QWindow *window, ulong timestamp, const QPointF &local, const QPointF &global,
                  Qt::MouseButtons buttons, Qt::MouseButton button, QEvent::Type type, Qt::KeyboardModifiers mods,
                  Qt::MouseEventSource source) __asm__(SYM_MOUSE);
bool portal_mouse(QWindow *window, ulong timestamp, const QPointF &local, const QPointF &global,
                  Qt::MouseButtons buttons, Qt::MouseButton button, QEvent::Type type, Qt::KeyboardModifiers mods,
                  Qt::MouseEventSource source)
{
    if (!realMouse())
        return false;
    if (!onMainThread())
        return realMouse()(window, timestamp, local, global, buttons, button, type, mods, source);
    if (type == QEvent::MouseMove && held.kind == Kind::Move && held.window == window && held.buttons == buttons
        && held.mods == mods) {
        held.timestamp = timestamp;
        held.local = local;
        held.global = global;
        return true;
    }
    flush();
    if (type != QEvent::MouseMove)
        return realMouse()(window, timestamp, local, global, buttons, button, type, mods, source);
    held = Held{Kind::Move, window, timestamp, nullptr, local, global, buttons, button, type, mods, source};
    hold();
    return true;
}

bool portal_touch(QWindow *window, ulong timestamp, const QPointingDevice *device,
                  const QList<WSI::TouchPoint> &points, Qt::KeyboardModifiers mods) __asm__(SYM_TOUCH);
bool portal_touch(QWindow *window, ulong timestamp, const QPointingDevice *device,
                  const QList<WSI::TouchPoint> &points, Qt::KeyboardModifiers mods)
{
    if (!realTouch())
        return false;
    if (!onMainThread())
        return realTouch()(window, timestamp, device, points, mods);
    const bool update = allUpdated(points);
    if (update && held.kind == Kind::Touch && held.window == window && held.device == device && held.mods == mods
        && sameIds(held.points, points)) {
        held.timestamp = timestamp;
        held.points = points;
        return true;
    }
    flush();
    if (!update)
        return realTouch()(window, timestamp, device, points, mods);
    held = Held{};
    held.kind = Kind::Touch;
    held.window = window;
    held.timestamp = timestamp;
    held.device = device;
    held.mods = mods;
    held.points = points;
    hold();
    return true;
}

bool portal_wheel(QWindow *window, ulong timestamp, const QPointingDevice *device, const QPointF &local,
                  const QPointF &global, QPoint pixel, QPoint angle, Qt::KeyboardModifiers mods,
                  Qt::ScrollPhase phase, Qt::MouseEventSource source, bool inverted) __asm__(SYM_WHEEL);
bool portal_wheel(QWindow *window, ulong timestamp, const QPointingDevice *device, const QPointF &local,
                  const QPointF &global, QPoint pixel, QPoint angle, Qt::KeyboardModifiers mods,
                  Qt::ScrollPhase phase, Qt::MouseEventSource source, bool inverted)
{
    if (!realWheel())
        return false;
    if (!onMainThread())
        return realWheel()(window, timestamp, device, local, global, pixel, angle, mods, phase, source, inverted);
    // Begin/End carry gesture state; only steps inside a phase merge.
    const bool step = phase == Qt::NoScrollPhase || phase == Qt::ScrollUpdate || phase == Qt::ScrollMomentum;
    if (step && held.kind == Kind::Wheel && held.window == window && held.device == device && held.mods == mods
        && held.phase == phase && held.source == source && held.inverted == inverted) {
        held.timestamp = timestamp;
        held.local = local;
        held.global = global;
        held.pixel += pixel;
        held.angle += angle;
        return true;
    }
    flush();
    if (!step)
        return realWheel()(window, timestamp, device, local, global, pixel, angle, mods, phase, source, inverted);
    held = Held{};
    held.kind = Kind::Wheel;
    held.window = window;
    held.timestamp = timestamp;
    held.device = device;
    held.local = local;
    held.global = global;
    held.mods = mods;
    held.source = source;
    held.pixel = pixel;
    held.angle = angle;
    held.phase = phase;
    held.inverted = inverted;
    hold();
    return true;
}

} // extern "C"
