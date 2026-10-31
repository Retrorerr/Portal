// Real-QPA regression: no application-specific preload or synthetic paint loop.
#include <QtCore/QCoreApplication>
#include <QtCore/QTimer>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtGui/QGuiApplication>
#include <QtGui/QWindow>
#include <QtGui/QMouseEvent>
#include <QtGui/QWheelEvent>
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

class Probe : public QWindow {
public:
    QStringList events;
    QPoint wheel;
    QList<int> wheelPhases;
    QPointF position;
    bool event(QEvent *e) override {
        switch (e->type()) {
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
        QCoreApplication::setAttribute(Qt::AA_CompressHighFrequencyEvents, true);
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
        // Drawing apps can opt out using Qt's standard public attribute.
        window.events.clear();
        QCoreApplication::setAttribute(Qt::AA_CompressHighFrequencyEvents, false);
        beginPointerEventBatch();
        for (int i = 0; i < 10; ++i) move(i);
        endPointerEventBatch(); drain();
        require(window.events.size() == 10, "compression opt-out preserves samples");
        std::puts("{\"pointer_ordering\":true,\"wheel_distance\":true,\"scroll_phases\":true,\"compression_opt_out\":true}");
        app.quit();
    });
    return app.exec();
}
