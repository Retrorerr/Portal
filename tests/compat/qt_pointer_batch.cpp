// Real-QPA regression: no application-specific preload or synthetic paint loop.
#include <QtCore/QCoreApplication>
#include <QtCore/QTimer>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtGui/QGuiApplication>
#include <QtGui/QWindow>
#include <QtGui/QMouseEvent>
#include <QtGui/QWheelEvent>
#include <QtGui/QTouchEvent>
#include <QtGui/private/qwindow_p.h>
#include <QtWaylandClient/private/qwaylandwindow_p.h>
#include <QtWaylandClient/private/qwaylandinputdevice_p.h>
#include <QtWaylandClient/private/qwaylanddisplay_p.h>
#include <cstdio>
#include <cstdlib>

namespace QtWaylandClient {
void beginPointerEventBatch();
void endPointerEventBatch();
void flushPointerEventBatch();
}
using namespace QtWaylandClient;

struct SeatAccess : QWaylandInputDevice {
    static Touch *touch(QWaylandInputDevice *seat) { return (seat->*(&SeatAccess::mTouch)).data(); }
};

class Probe : public QWindow {
public:
    QStringList events;
    QList<QEventPoint> points;
    QPoint wheel;
    QList<int> wheelPhases;
    QPointF position;
    bool event(QEvent *e) override {
        switch (e->type()) {
        case QEvent::TouchBegin:
        case QEvent::TouchUpdate:
        case QEvent::TouchEnd: {
            auto *touch = static_cast<QTouchEvent *>(e);
            events << (e->type() == QEvent::TouchBegin ? "touch-begin"
                : e->type() == QEvent::TouchUpdate ? "touch-update" : "touch-end");
            points = touch->points();
            e->accept();
            return true;
        }
        case QEvent::MouseMove:
            events << "move";
            position = static_cast<QMouseEvent *>(e)->position();
            break;
        case QEvent::MouseButtonPress: events << "press"; break;
        case QEvent::MouseButtonRelease: events << "release"; break;
        case QEvent::Wheel:
            events << "wheel";
            wheel += static_cast<QWheelEvent *>(e)->angleDelta();
            wheelPhases << int(static_cast<QWheelEvent *>(e)->phase());
            break;
        default: break;
        }
        return QWindow::event(e);
    }
};

int main(int argc, char **argv) {
    QGuiApplication app(argc, argv);
    Probe window;
    window.setFlags(Qt::Window | Qt::FramelessWindowHint);
    window.setTitle("Portal pointer ordering regression");
    window.resize(640, 480);
    window.show();
    QTimer::singleShot(800, [&] {
        auto *native = dynamic_cast<QWaylandWindow *>(QWindowPrivate::get(&window)->platformWindow);
        if (!native || !native->display()->currentInputDevice()) std::exit(2);
        auto *seat = native->display()->currentInputDevice();
        auto drain = [&] { QWindowSystemInterface::sendWindowSystemEvents(QEventLoop::AllEvents); };
        auto move = [&](int x, Qt::MouseButtons buttons = Qt::NoButton) {
            native->handleMouse(seat, QWaylandPointerEvent(QEvent::MouseMove, Qt::NoScrollPhase,
                native, x, QPointF(x, 20), QPointF(x, 20), buttons, Qt::NoButton, Qt::NoModifier));
        };
        auto button = [&](bool down) {
            const QPointF position(down ? 100 : 200, 20);
            native->handleMouse(seat, QWaylandPointerEvent(down ? QEvent::MouseButtonPress : QEvent::MouseButtonRelease,
                Qt::NoScrollPhase, native, 100, position, position,
                down ? Qt::LeftButton : Qt::NoButton, Qt::LeftButton, Qt::NoModifier));
        };
        auto wheel = [&](int amount, Qt::ScrollPhase phase = Qt::NoScrollPhase) {
            native->handleMouse(seat, QWaylandPointerEvent(QEvent::Wheel, phase, native, 200,
                QPointF(100, 100), QPointF(100, 100), QPoint(), QPoint(0, amount),
                Qt::MouseEventNotSynthesized, Qt::NoModifier, false));
        };
        auto require = [&](bool value, const char *message) {
            if (!value) { std::fprintf(stderr, "FAIL: %s; events=%s\n", message,
                qPrintable(window.events.join(','))); std::exit(1); }
        };
        drain(); window.events.clear();
        require(QCoreApplication::testAttribute(Qt::AA_CompressHighFrequencyEvents), "compression on by default, as on xcb");
        beginPointerEventBatch();
        for (int i = 1; i <= 100; ++i) move(i);
        button(true);
        for (int i = 101; i <= 200; ++i) move(i, Qt::LeftButton);
        button(false);
        endPointerEventBatch(); drain();
        require(window.events == QStringList({"move", "press", "move", "release"}), "pointer edge ordering / bounded move delivery");
        require(window.position == QPointF(200, 20), "final pointer position");
        window.events.clear(); window.wheel = {};
        beginPointerEventBatch();
        for (int i = 0; i < 100; ++i) wheel(12);
        wheel(-120);
        endPointerEventBatch(); drain();
        require(window.events == QStringList({"wheel", "wheel"}), "wheel direction boundary");
        require(window.wheel == QPoint(0, 1080), "wheel distance conserved");
        window.events.clear(); window.wheelPhases.clear();
        beginPointerEventBatch();
        wheel(0, Qt::ScrollBegin); wheel(12, Qt::ScrollUpdate); wheel(12, Qt::ScrollUpdate); wheel(0, Qt::ScrollEnd);
        endPointerEventBatch(); drain();
        // Qt's QPA emits vertical + horizontal compatibility events for
        // zero-delta begin/end frames. Preserve those existing semantics.
        require(window.wheelPhases == QList<int>({1, 1, 2, 3, 3}), "scroll begin/update/end boundaries");
        auto *touch = SeatAccess::touch(seat);
        require(touch, "seat has touch");
        const auto fixed = [](int value) { return wl_fixed_from_int(value); };
        auto touchDown = [&](int id, int x) {
            touch->touch_down(1, 1, native->wlSurface(), id, fixed(x), fixed(20));
            touch->touch_frame();
        };
        auto touchMove = [&](int id, int x) { touch->touch_motion(2, id, fixed(x), fixed(20)); touch->touch_frame(); };
        auto touchUp = [&](int id) { touch->touch_up(3, 3, id); touch->touch_frame(); };
        window.events.clear();
        beginPointerEventBatch();
        touchDown(0, 10);
        for (int i = 11; i <= 110; ++i) touchMove(0, i);
        touchUp(0);
        endPointerEventBatch(); drain();
        require(window.events == QStringList({"touch-begin", "touch-update", "touch-end"}), "touch edge ordering / bounded update delivery");
        require(window.points.first().position() == QPointF(110, 20), "final touch position");
        // A point that moved earlier in the burst must still report moving.
        window.events.clear();
        touchDown(0, 10); touchDown(1, 50); drain(); window.events.clear();
        beginPointerEventBatch();
        touchMove(0, 20);
        touchMove(1, 60);
        endPointerEventBatch(); drain();
        require(window.events == QStringList({"touch-update"}), "multi-touch frames merge");
        require(window.points.size() == 2 && window.points.at(0).state() == QEventPoint::Updated
                && window.points.at(1).state() == QEventPoint::Updated, "both moved points stay updated");
        touchUp(0); touchUp(1); drain();
        // Drawing apps can opt out using Qt's standard public attribute.
        window.events.clear();
        QCoreApplication::setAttribute(Qt::AA_CompressHighFrequencyEvents, false);
        beginPointerEventBatch();
        for (int i = 0; i < 10; ++i) move(i);
        endPointerEventBatch(); drain();
        require(window.events.size() == 10, "compression opt-out preserves samples");
        window.events.clear();
        touchDown(0, 10); drain(); window.events.clear();
        beginPointerEventBatch();
        for (int i = 11; i <= 20; ++i) touchMove(0, i);
        endPointerEventBatch(); drain();
        require(window.events.size() == 10, "compression opt-out preserves touch samples");
        touchUp(0); drain();
        std::puts("{\"pointer_ordering\":true,\"wheel_distance\":true,\"scroll_phases\":true,\"touch_ordering\":true,\"multi_touch\":true,\"compression_opt_out\":true}");
        app.quit();
    });
    return app.exec();
}
