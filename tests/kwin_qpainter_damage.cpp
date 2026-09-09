// SPDX-License-Identifier: GPL-2.0-or-later
// Compile against the patched KWin header and the guest's actual Qt rasterizer.
#include "qpainter_buffer_damage.h"
#include <QImage>
#include <QPainter>
#include <cstdio>
#include <cmath>

int main()
{
    int cases = 0;
    int oldIdentityMisses = 0;
    int oldIntegerScaleMisses = 0;
    for (const QSize buffer : {QSize(2714, 1920), QSize(3392, 2400), QSize(1920, 2714), QSize(1001, 701)}) {
        for (const double scale : {1.0, 1.25, 1.5, 2.0, 2.25, 3.0}) {
            const QSize window = (QSizeF(buffer) / scale).toSize();
            for (const QPoint fraction : {QPoint(0, 0), QPoint(1, 0), QPoint(0, 1), QPoint(1, 1), QPoint(2, 2)}) {
                const QPoint origin((window.width() - 23) * fraction.x() / 2,
                                    (window.height() - 19) * fraction.y() / 2);
                const QRect dirty(origin, QSize(23, 19));
                const QRegion repaired = KWin::Wayland::qpainterBufferDamage(dirty, window, buffer);
                // Emulate the actual KWin QPainter clip/window/viewport mapping.
                QImage image(buffer, QImage::Format_RGB32);
                image.fill(0xff000000);
                QPainter painter(&image);
                painter.setWindow(QRect(QPoint(0, 0), window));
                painter.setClipRegion(dirty);
                painter.fillRect(QRect(QPoint(0, 0), window), Qt::white);
                painter.end();
                const QRegion identity(dirty);
                const double integerScale = std::ceil(scale);
                const QRegion integerDamage(QRectF(dirty.x() * integerScale, dirty.y() * integerScale,
                                                   dirty.width() * integerScale, dirty.height() * integerScale).toAlignedRect());
                bool identityMiss = false;
                bool integerMiss = false;
                for (int y = 0; y < image.height(); ++y) {
                    const auto row = reinterpret_cast<const QRgb *>(image.constScanLine(y));
                    for (int x = 0; x < image.width(); ++x) {
                        if (row[x] != 0xff000000) {
                            if (!repaired.contains(QPoint(x, y))) {
                                std::fprintf(stderr, "uncovered pixel %d,%d scale=%g buffer=%dx%d\n", x, y, scale, buffer.width(), buffer.height());
                                return 1;
                            }
                            identityMiss |= !identity.contains(QPoint(x, y));
                            integerMiss |= !integerDamage.contains(QPoint(x, y));
                        }
                    }
                }
                oldIdentityMisses += identityMiss;
                oldIntegerScaleMisses += integerMiss;
                ++cases;
            }
        }
    }
    std::printf("QPainter damage: %d cases passed; old buffer-identity missed %d, old integer-scale missed %d\n", cases, oldIdentityMisses, oldIntegerScaleMisses);
    return oldIdentityMisses > 0 && oldIntegerScaleMisses > 0 ? 0 : 2;
}
