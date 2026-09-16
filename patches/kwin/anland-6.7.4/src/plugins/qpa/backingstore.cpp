/*
    KWin - the KDE window manager
    This file is part of the KDE project.

    SPDX-FileCopyrightText: 2015 Martin Gräßlin <mgraesslin@kde.org>
    SPDX-FileCopyrightText: 2019 Vlad Zahorodnii <vlad.zahorodnii@kde.org>

    SPDX-License-Identifier: GPL-2.0-or-later
*/
#include "backingstore.h"
#include "core/graphicsbuffer.h"
#include "core/graphicsbufferview.h"
#include "internalwindow.h"
#include "logging.h"
#include "swapchain.h"
#include "window.h"

#include <QPainter>
#include <libdrm/drm_fourcc.h>

namespace KWin
{
namespace QPA
{

BackingStore::BackingStore(QWindow *window)
    : QPlatformBackingStore(window)
    , m_fallbackImage(1, 1, QImage::Format_ARGB32_Premultiplied)
{
}

QPaintDevice *BackingStore::paintDevice()
{
    if (m_usingFallback || !m_bufferView || m_bufferView->isNull()) {
        return &m_fallbackImage;
    }
    return m_bufferView->image();
}

void BackingStore::resize(const QSize &size, const QRegion &staticContents)
{
    m_bufferView.reset();
    m_buffer.clear();
    if (QPlatformWindow *platformWindow = window()->handle()) {
        platformWindow->invalidateSurface();
    }
    m_usingFallback = false;
}

void BackingStore::beginPaint(const QRegion &region)
{
    Window *platformWindow = static_cast<Window *>(window()->handle());
    m_bufferView.reset();
    m_usingFallback = false;
    if (!platformWindow) {
        qCCritical(KWIN_QPA) << "Cannot begin painting without a platform window";
        m_buffer.clear();
        m_usingFallback = true;
        return;
    }

    Swapchain *swapchain = platformWindow->swapchain(nullptr, {{DRM_FORMAT_ARGB8888, {DRM_FORMAT_MOD_LINEAR}}});
    if (!swapchain) {
        qCCritical(KWIN_QPA, "Failed to create a swapchain for the backing store!");
        m_buffer.clear();
        m_usingFallback = true;
        return;
    }

    const auto oldBuffer = m_buffer;
    if (auto buffer = swapchain->acquire()) {
        m_buffer = buffer;
    } else {
        qCCritical(KWIN_QPA, "Failed to acquire a graphics buffer for the backing store");
        // Keep presenting the last good buffer (if any) instead of going
        // blank: painting falls back to the 1x1 image and this frame is
        // dropped in flush(), while the next beginPaint retries acquisition.
        m_buffer = oldBuffer;
        m_usingFallback = true;
        return;
    }

    m_bufferView = std::make_unique<GraphicsBufferView>(m_buffer, GraphicsBuffer::Read | GraphicsBuffer::Write);
    if (m_bufferView->isNull()) {
        qCCritical(KWIN_QPA) << "Failed to map a graphics buffer for the backing store";
        m_bufferView.reset();
        m_buffer.clear();
        m_usingFallback = true;
        return;
    }

    if (oldBuffer && oldBuffer != m_buffer && oldBuffer->size() == m_buffer->size()) {
        const GraphicsBufferView oldView(oldBuffer, GraphicsBuffer::Read);
        std::memcpy(m_bufferView->image()->bits(), oldView.image()->constBits(), oldView.image()->sizeInBytes());
    }

    QImage *image = m_bufferView->image();
    image->setDevicePixelRatio(platformWindow->devicePixelRatio());

    if (image->hasAlphaChannel()) {
        QPainter p(image);
        p.setCompositionMode(QPainter::CompositionMode_Source);
        const QColor blank = Qt::transparent;
        for (const QRect &rect : region) {
            p.fillRect(rect, blank);
        }
    }
}

void BackingStore::endPaint()
{
    m_bufferView.reset();
}

void BackingStore::flush(QWindow *window, const QRegion &region, const QPoint &offset)
{
    Window *platformWindow = static_cast<Window *>(window->handle());
    if (!platformWindow) {
        return;
    }
    InternalWindow *internalWindow = platformWindow->internalWindow();
    if (!internalWindow) {
        return;
    }
    // Allocation/mapping may have failed in beginPaint(): painting went to
    // the 1x1 fallback image, which cannot be presented. Drop this frame and
    // keep the last successfully presented buffer on screen; the next
    // beginPaint() retries. Never dereference or present a null buffer here.
    if (m_usingFallback || !m_buffer) {
        return;
    }

    const qreal scale = platformWindow->devicePixelRatio();
    const QRect bufferRect(QPoint(0, 0), m_buffer->size());
    Region bufferDamage;
    for (const QRect &rect : region) {
        bufferDamage += Rect(rect)
                            .scaled(scale)
                            .roundedOut()
                            .intersected(bufferRect);
    }

    internalWindow->present(InternalWindowFrame{
        .buffer = m_buffer,
        .bufferDamage = bufferDamage,
    });
}

}
}
