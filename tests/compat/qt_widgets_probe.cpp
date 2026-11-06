// Compile against a bundle's own Qt 5/6 headers and libraries; no moc needed.
#include <QApplication>
#include <QLineEdit>
#include <QPainter>
#include <QScreen>
#include <QTimer>
#include <QVBoxLayout>
#include <QWindow>
#include <cstdio>
#include <cstdlib>
#include <time.h>

static long long now_ns()
{
    timespec value;
    clock_gettime(CLOCK_MONOTONIC, &value);
    return static_cast<long long>(value.tv_sec) * 1000000000 + value.tv_nsec;
}

class Canvas : public QWidget {
public:
    unsigned frames = 0;
    long long first_frame = 0;
    qreal first_resize_dpr = 0;
    void resizeEvent(QResizeEvent *event) override
    {
        // Toolkits that size their own buffers from this event keep this ratio.
        if (first_resize_dpr == 0) first_resize_dpr = devicePixelRatioF();
        QWidget::resizeEvent(event);
    }
    void paintEvent(QPaintEvent *) override
    {
        if (!first_frame) first_frame = now_ns();
        QPainter painter(this);
        painter.fillRect(rect(), QColor("#192533"));
        painter.fillRect((frames++ * 6) % 600, 80, 100, 100, QColor("#e8794a"));
    }
};

int main(int argc, char **argv)
{
    const int duration = argc > 1 ? static_cast<int>(std::atof(argv[1]) * 1000) : 4000;
    QApplication app(argc, argv);
    QWidget window;
    window.setWindowTitle("Portal compatibility: bundled Qt Widgets");
    QVBoxLayout layout(&window);
    QLineEdit edit(QString::fromUtf8("Native bundled Qt / Unicode: 日本語 café"));
    Canvas canvas;
    layout.addWidget(&edit);
    layout.addWidget(&canvas);
    window.resize(760, 480);
    window.show();
    QTimer timer;
    QObject::connect(&timer, &QTimer::timeout, &canvas, [&] { canvas.update(); });
    timer.start(16);
    QTimer::singleShot(duration * 65 / 100, &window, [&] { window.resize(800, 540); });
    QTimer::singleShot(duration, &window, [&] {
        const QWindow *handle = window.windowHandle();
        const bool resized = window.width() == 800 && window.height() == 540
            && handle->width() == 800 && handle->height() == 540;
        std::printf("{\"toolkit\":\"bundled Qt Widgets\",\"version\":\"%s\","
                    "\"backend\":\"%s\",\"frames\":%u,\"window_dpr\":%.6f,\"first_resize_dpr\":%.6f,"
                    "\"resize_consistent\":%s,\"flow_start_ns\":%lld,\"flow_end_ns\":%lld}\n",
                    qVersion(), qPrintable(QApplication::platformName()), canvas.frames,
                    handle->devicePixelRatio(), canvas.first_resize_dpr, resized ? "true" : "false",
                    canvas.first_frame, now_ns());
        app.exit(resized && canvas.frames ? 0 : 1);
    });
    return app.exec();
}
