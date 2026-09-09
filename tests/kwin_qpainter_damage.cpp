// SPDX-License-Identifier: GPL-2.0-or-later
// Regression for fractional ghosting: every changed QImage pixel must be in damage.
// Compile: c++ -std=c++23 -O2 -fPIC tests/kwin_qpainter_damage.cpp -I patches/kwin/debian-6.3.6 -I src/backends/wayland $(pkg-config --cflags --libs Qt6Gui) -o /tmp/qpainter-damage-test && /tmp/qpainter-damage-test
#include "qpainter_buffer_damage.h"
#include <QImage>
#include <QPainter>
#include <QRect>
#include <cstdio>
#include <cmath>
#include <vector>

// Old implementation (size-only window, no +1, no origin subtraction) for comparison.
static QRegion oldRoundedDamage(const QRegion &logicalDamage, const QSize &window, const QSize &buffer)
{
    const QRect bounds(QPoint(0, 0), buffer);
    if (window.isEmpty() || buffer.isEmpty())
        return bounds;
    const qreal sx = qreal(buffer.width()) / window.width();
    const qreal sy = qreal(buffer.height()) / window.height();
    QRegion result;
    for (const QRect &rect : logicalDamage) {
        result += QRectF(rect.x() * sx, rect.y() * sy, rect.width() * sx, rect.height() * sy)
                      .toAlignedRect()
                      .intersected(bounds);
    }
    return result;
}

static int qroundQt(double v) { return int(std::floor(v + 0.5)); }

int main()
{
    const QSize host(3392, 2400);
    const std::vector<double> scales = {1.0, 1.1, 1.25, 1.33, 1.5, 1.75, 2.0, 2.25, 2.5};
    int cases = 0;
    int newUncovered = 0;
    int oldFractionalMisses = 0;
    int oldOriginMisses = 0;
    int integerPassOld = 0;
    int integerCases = 0;

    for (double scale : scales) {
        const int cfgW = int(std::round(host.width() / scale));
        const int cfgH = int(std::round(host.height() / scale));
        const int bufW = qroundQt(cfgW * scale);
        const int bufH = qroundQt(cfgH * scale);
        const QSize buffer(bufW, bufH);
        const double geomWf = bufW / scale;
        const double geomHf = bufH / scale;
        const QSize winSize(qroundQt(geomWf), qroundQt(geomHf));
        const bool isInteger = (scale == 1.0 || scale == 2.0);

        // Damage set: origin, centre, far bottom-right (worst shift), thin 1px far edge,
        // moving 10px steps (5 frames), scroll strip bottom.
        std::vector<QRect> damages;
        damages.emplace_back(0, 0, 23, 19);
        damages.emplace_back(100, 100, 23, 19);
        damages.emplace_back(std::max(0, winSize.width() - 23), std::max(0, winSize.height() - 19), 23, 19);
        damages.emplace_back(std::max(0, winSize.width() - 1), 0, 1, winSize.height()); // thin far vertical
        damages.emplace_back(0, std::max(0, winSize.height() - 1), winSize.width(), 1); // thin bottom
        damages.emplace_back(0, std::max(0, winSize.height() - 20), winSize.width(), 20); // scroll strip
        for (int step = 0; step < 5; ++step)
            damages.emplace_back(50 + step * 10, 60, 200, 150); // moving rect
        // Negative / clipping.
        damages.emplace_back(-5, -5, 20, 20);

        for (const QRect &dirtyRaw : damages) {
            // Clip dirty to integer window (KWin clips to superLayer aligned, viewport integer).
            // Keep raw for negative test (do not clamp negative away, let mapping clip).
            const QRect dirty = dirtyRaw;
            const QRect windowRect(QPoint(0, 0), winSize);
            const QRegion repaired = KWin::Wayland::qpainterBufferDamage(QRegion(dirty), windowRect, buffer);
            const QRegion oldRepaired = oldRoundedDamage(QRegion(dirty), winSize, buffer);

            // Real Qt raster truth with exact QPainter window/viewport (rounded window, full image viewport).
            QImage image(buffer, QImage::Format_RGB32);
            image.fill(0xff000000);
            {
                QPainter painter(&image);
                painter.setWindow(windowRect);
                painter.setClipRegion(QRegion(dirty));
                painter.fillRect(windowRect, Qt::white);
                painter.end();
            }
            // Invariant: every white pixel must be in new damage.
            for (int y = 0; y < image.height(); ++y) {
                const auto row = reinterpret_cast<const QRgb *>(image.constScanLine(y));
                for (int x = 0; x < image.width(); ++x) {
                    if (row[x] != 0xff000000 && !repaired.contains(QPoint(x, y))) {
                        std::fprintf(stderr, "NEW uncovered %d,%d scale=%g win=%dx%d buf=%dx%d dirty=(%d,%d %dx%d)\n",
                                     x, y, scale, winSize.width(), winSize.height(), buffer.width(), buffer.height(),
                                     dirty.x(), dirty.y(), dirty.width(), dirty.height());
                        ++newUncovered;
                        goto next_dirty;
                    }
                }
            }
        next_dirty:;
            // Old vs float-ideal truth (exact scale, no rounding): systematic shift check.
            // Float-ideal damage: (x*scale, y*scale, w*scale, h*scale) outward-aligned.
            {
                const QRectF f(dirty.x() * scale, dirty.y() * scale, dirty.width() * scale, dirty.height() * scale);
                const QRect ideal = f.toAlignedRect().intersected(QRect(QPoint(0, 0), buffer));
                // Old covers ideal? At integer exact, yes; at fractional far-edge thin, no (1px miss).
                bool oldCoversIdeal = true;
                // Sample ideal rect coverage: check ideal corners/edges inside old.
                // Conservative: old must contain entire ideal (bounding containment).
                if (!ideal.isEmpty()) {
                    // Check all four edges via containment of ideal bounding rect.
                    // QRegion containment of rect: old must contain ideal (old ⊇ ideal).
                    // Approximate by checking ideal extras: ideal - old must be empty.
                    QRegion diff = QRegion(ideal) - oldRepaired;
                    if (!diff.isEmpty())
                        oldCoversIdeal = false;
                }
                const bool farEdge = (dirty.x() + dirty.width() >= winSize.width() - 1) ||
                                     (dirty.y() + dirty.height() >= winSize.height() - 1);
                const bool thin = (dirty.width() == 1 || dirty.height() == 1);
                if (!oldCoversIdeal && (farEdge || thin)) {
                    if (!isInteger)
                        ++oldFractionalMisses;
                }
                if (isInteger && oldCoversIdeal)
                    ++integerPassOld;
                if (isInteger)
                    ++integerCases;
            }
            ++cases;
        }

        // Non-zero origin: window at (100,50), damage global (110,60,23,19).
        {
            const QRect windowRect(QPoint(100, 50), winSize);
            const QRect dirtyGlobal(110, 60, 23, 19);
            const QRegion repaired = KWin::Wayland::qpainterBufferDamage(QRegion(dirtyGlobal), windowRect, buffer);
            const QRegion oldRepaired = oldRoundedDamage(QRegion(dirtyGlobal), winSize, buffer);
            QImage image(buffer, QImage::Format_RGB32);
            image.fill(0xff000000);
            {
                QPainter painter(&image);
                painter.setWindow(windowRect);
                painter.setClipRegion(QRegion(dirtyGlobal));
                painter.fillRect(windowRect, Qt::white);
                painter.end();
            }
            for (int y = 0; y < image.height(); ++y) {
                const auto row = reinterpret_cast<const QRgb *>(image.constScanLine(y));
                for (int x = 0; x < image.width(); ++x) {
                    if (row[x] != 0xff000000 && !repaired.contains(QPoint(x, y))) {
                        std::fprintf(stderr, "NEW origin uncovered %d,%d scale=%g\n", x, y, scale);
                        ++newUncovered;
                        goto next_origin;
                    }
                }
            }
        next_origin:;
            // Old ignores origin (off by 100*sx, severe) -> must miss at all scales.
            bool oldMisses = false;
            for (int y = 0; y < image.height(); ++y) {
                const auto row = reinterpret_cast<const QRgb *>(image.constScanLine(y));
                for (int x = 0; x < image.width(); ++x) {
                    if (row[x] != 0xff000000 && !oldRepaired.contains(QPoint(x, y))) {
                        oldMisses = true;
                        break;
                    }
                }
                if (oldMisses)
                    break;
            }
            if (oldMisses)
                ++oldOriginMisses;
            ++cases;
        }
    }

    std::printf("QPainter damage: %d cases newUncovered=%d oldFractionalMisses=%d oldOriginMisses=%d integerOldPass=%d/%d\n",
                cases, newUncovered, oldFractionalMisses, oldOriginMisses, integerPassOld, integerCases);
    if (newUncovered != 0) {
        std::fprintf(stderr, "FAIL: new damage under-covers QImage\n");
        return 1;
    }
    // Regression must expose old bug at fractional far-edge/thin and at non-zero origins,
    // while integer 1.0/2.0 centre passes on old (no systematic shift when exact).
    if (oldFractionalMisses == 0) {
        std::fprintf(stderr, "FAIL: test does not expose fractional old bug (expected >0)\n");
        return 2;
    }
    if (oldOriginMisses == 0) {
        std::fprintf(stderr, "FAIL: test does not expose origin old bug\n");
        return 2;
    }
    return 0;
}
