/*
    KWin - the KDE window manager
    This file is part of the KDE project.

    SPDX-FileCopyrightText: 2006 Lubos Lunak <l.lunak@kde.org>
    SPDX-FileCopyrightText: 2009, 2010, 2011 Martin Gräßlin <mgraesslin@kde.org>

    SPDX-License-Identifier: GPL-2.0-or-later
*/
#include "opengl/eglbackend.h"
#include "compositor.h"
#include "core/drm_formats.h"
#include "core/drmdevice.h"
#include "core/graphicsbuffer.h"
#include "core/outputbackend.h"
#include "core/renderdevice.h"
#include "main.h"
#include "opengl/eglimagetexture.h"
#include "opengl/eglutils_p.h"
#include "utils/common.h"
#include "wayland/linux_drm_syncobj_v1.h"
#include "wayland_server.h"

#include <QElapsedTimer>

#include <drm_fourcc.h>
#include <unistd.h>
#include <xf86drm.h>

namespace KWin
{

static std::shared_ptr<EglContext> s_globalShareContext;

EglBackend::EglBackend()
{
}

CompositingType EglBackend::compositingType() const
{
    return OpenGLCompositing;
}

bool EglBackend::checkGraphicsReset()
{
    const auto context = openglContext();
    if (context != EglContext::currentContext()) {
        const bool success = context->makeCurrent();
        if (!success) {
            // not necessarily a graphics reset, but we can't really know
            // and need to re-create everything either way
            return true;
        }
    }
    const GLenum status = context->checkGraphicsResetStatus();
    if (Q_LIKELY(status == GL_NO_ERROR)) {
        return false;
    }

    switch (status) {
    case GL_GUILTY_CONTEXT_RESET:
        qCWarning(KWIN_OPENGL) << "A graphics reset attributable to the current GL context occurred.";
        break;
    case GL_INNOCENT_CONTEXT_RESET:
        qCWarning(KWIN_OPENGL) << "A graphics reset not attributable to the current GL context occurred.";
        break;
    case GL_UNKNOWN_CONTEXT_RESET:
        qCWarning(KWIN_OPENGL) << "A graphics reset of an unknown cause occurred.";
        break;
    default:
        break;
    }

    QElapsedTimer timer;
    timer.start();

    // Wait until the reset is completed or max one second
    while (timer.elapsed() < 10000 && context->checkGraphicsResetStatus() != GL_NO_ERROR) {
        usleep(50);
    }
    if (timer.elapsed() >= 10000) {
        qCWarning(KWIN_OPENGL) << "Waiting for glGetGraphicsResetStatus to return GL_NO_ERROR timed out!";
    }

    return true;
}

bool EglBackend::ensureGlobalShareContext()
{
    EglDisplay *display = m_renderDevice ? m_renderDevice->eglDisplay() : m_eglDisplay.get();
    if (!display) {
        return false;
    }
    if (s_globalShareContext && s_globalShareContext->displayObject() != display) {
        s_globalShareContext.reset();
        kwinApp()->outputBackend()->setSceneEglGlobalShareContext(nullptr);
    }
    if (!s_globalShareContext) {
        s_globalShareContext = EglContext::create(display, EGL_NO_CONFIG_KHR, nullptr);
        connect(Compositor::self(), &Compositor::aboutToDestroy, qApp, []() {
            if (!s_globalShareContext) {
                return;
            }
            s_globalShareContext.reset();
            kwinApp()->outputBackend()->setSceneEglGlobalShareContext(nullptr);
        });
    }
    if (s_globalShareContext) {
        kwinApp()->outputBackend()->setSceneEglGlobalShareContext(s_globalShareContext.get());
        return true;
    } else {
        return false;
    }
}

void EglBackend::cleanup()
{
    m_context.reset();
    if (m_eglDisplay && s_globalShareContext && s_globalShareContext->displayObject() == m_eglDisplay.get()) {
        s_globalShareContext.reset();
        if (kwinApp()->outputBackend()) {
            kwinApp()->outputBackend()->setSceneEglGlobalShareContext(nullptr);
        }
    }
    m_eglDisplay.reset();
}

void EglBackend::setRenderDevice(RenderDevice *device)
{
    m_renderDevice = device;
}

bool EglBackend::setEglDisplay(::EGLDisplay display)
{
    m_eglDisplay = EglDisplay::create(display, nullptr);
    return m_eglDisplay != nullptr;
}

void EglBackend::initWayland()
{
    // Anland owns the Android dmabuf transport.  It deliberately has no
    // KWin DRM render device, so the generic Linux dmabuf feedback path is
    // neither available nor needed for this backend.
    if (!m_renderDevice || !WaylandServer::self()) {
        return;
    }
    DrmDevice *scanoutDevice = drmDevice();
    Q_ASSERT(scanoutDevice);

    auto filterFormats = [this](std::optional<uint32_t> bpc, bool withExternalOnlyYUV) {
        FormatModifierMap set;
        const auto &allFormats = m_renderDevice->eglDisplay()->allSupportedDrmFormats();
        const auto &nonExternalOnly = m_renderDevice->eglDisplay()->nonExternalOnlySupportedDrmFormats();
        for (auto it = allFormats.constBegin(); it != allFormats.constEnd(); it++) {
            const auto info = FormatInfo::get(it.key());
            if (bpc && (!info || bpc != info->bitsPerColor)) {
                continue;
            }

            const bool externalOnlySupported = withExternalOnlyYUV && info && info->yuvConversion();
            ModifierList modifiers = externalOnlySupported ? *it : nonExternalOnly[it.key()];

            if (externalOnlySupported && !modifiers.empty()) {
                if (auto yuv = info->yuvConversion()) {
                    for (auto plane : std::as_const(yuv->plane)) {
                        const auto planeModifiers = allFormats.value(plane.format);
                        erase_if(modifiers, [&planeModifiers](uint64_t mod) {
                            return !planeModifiers.contains(mod);
                        });
                    }
                }
            }
            for (const auto &tranche : std::as_const(m_tranches)) {
                if (modifiers.empty()) {
                    break;
                }
                const auto trancheModifiers = tranche.formatTable.value(it.key());
                for (auto trancheModifier : trancheModifiers) {
                    modifiers.erase(trancheModifier);
                }
            }
            if (modifiers.empty()) {
                continue;
            }
            set.insert(it.key(), modifiers);
        }
        return set;
    };

    auto includeShaderConversions = [](FormatModifierMap &&formats) -> FormatModifierMap {
        for (auto format : FormatInfo::s_drmConversions.keys()) {
            auto &modifiers = formats[format];
            if (modifiers.empty()) {
                modifiers = {DRM_FORMAT_MOD_LINEAR};
            }
        }
        return formats;
    };

    m_tranches.append({
        .device = m_renderDevice->eglDisplay()->renderDevNode().value_or(scanoutDevice->deviceId()),
        .flags = LinuxDmaBufV1Feedback::TrancheFlag::Sampling,
        .formatTable = filterFormats(10, false),
    });
    m_tranches.append({
        .device = m_renderDevice->eglDisplay()->renderDevNode().value_or(scanoutDevice->deviceId()),
        .flags = LinuxDmaBufV1Feedback::TrancheFlag::Sampling,
        .formatTable = filterFormats(8, false),
    });
    m_tranches.append({
        .device = m_renderDevice->eglDisplay()->renderDevNode().value_or(scanoutDevice->deviceId()),
        .flags = LinuxDmaBufV1Feedback::TrancheFlag::Sampling,
        .formatTable = includeShaderConversions(filterFormats(std::nullopt, true)),
    });

    LinuxDmaBufV1ClientBufferIntegration *dmabuf = waylandServer()->linuxDmabuf();
    dmabuf->setRenderBackend(this);
    dmabuf->setSupportedFormatsWithModifiers(m_tranches);

    waylandServer()->setRenderBackend(this);
}

bool EglBackend::initClientExtensions(bool requireGbm)
{
    // Get the list of client extensions
    const char *clientExtensionsCString = eglQueryString(EGL_NO_DISPLAY, EGL_EXTENSIONS);
    const QByteArray clientExtensionsString = QByteArray::fromRawData(clientExtensionsCString, qstrlen(clientExtensionsCString));
    if (clientExtensionsString.isEmpty()) {
        // If eglQueryString() returned NULL, the implementation doesn't support
        // EGL_EXT_client_extensions. Expect an EGL_BAD_DISPLAY error.
        EGLint error = eglGetError();
        if (error != EGL_SUCCESS) {
            qCWarning(KWIN_OPENGL) << "Error during eglQueryString " << getEglErrorString(error);
        }
    }

    m_clientExtensions = clientExtensionsString.split(' ');

    QList<QByteArray> requiredExtensions = {QByteArrayLiteral("EGL_EXT_platform_base")};
    if (requireGbm) {
        requiredExtensions.append(QByteArrayLiteral("EGL_KHR_platform_gbm"));
    }
    for (const QByteArray &extension : std::as_const(requiredExtensions)) {
        if (!hasClientExtension(extension)) {
            qCWarning(KWIN_OPENGL, "Required client extension %s is not supported", qPrintable(extension));
            return false;
        }
    }
    return true;
}

bool EglBackend::hasClientExtension(const QByteArray &ext) const
{
    return m_clientExtensions.contains(ext);
}

bool EglBackend::isOpenGLES() const
{
    return EglDisplay::shouldUseOpenGLES();
}

bool EglBackend::createContext()
{
    if (!ensureGlobalShareContext()) {
        return false;
    }
    if (m_renderDevice) {
        m_context = m_renderDevice->eglContext(s_globalShareContext.get());
    } else {
        EglDisplay *display = eglDisplayObject();
        if (!display) {
            return false;
        }
        m_context = EglContext::create(display, EGL_NO_CONFIG_KHR, s_globalShareContext.get());
    }
    return m_context != nullptr;
}

QList<LinuxDmaBufV1Feedback::Tranche> EglBackend::tranches() const
{
    return m_tranches;
}

EGLImageKHR EglBackend::importBufferAsImage(GraphicsBuffer *buffer)
{
    EglDisplay *display = eglDisplayObject();
    if (!display) {
        return EGL_NO_IMAGE_KHR;
    }
    return display->importBufferAsImage(buffer);
}

EGLImageKHR EglBackend::importBufferAsImage(GraphicsBuffer *buffer, int plane, int format, const QSize &size)
{
    EglDisplay *display = eglDisplayObject();
    if (!display) {
        return EGL_NO_IMAGE_KHR;
    }
    return display->importBufferAsImage(buffer, plane, format, size);
}

std::shared_ptr<GLTexture> EglBackend::importDmaBufAsTexture(const DmaBufAttributes &attributes) const
{
    return m_context->importDmaBufAsTexture(attributes);
}

bool EglBackend::testImportBuffer(GraphicsBuffer *buffer)
{
    EglDisplay *display = eglDisplayObject();
    if (!display) {
        return false;
    }
    const auto nonExternalOnly = display->nonExternalOnlySupportedDrmFormats();
    if (auto it = nonExternalOnly.find(buffer->dmabufAttributes()->format); it != nonExternalOnly.end() && it->contains(buffer->dmabufAttributes()->modifier)) {
        return importBufferAsImage(buffer) != EGL_NO_IMAGE_KHR;
    }
    // external_only buffers aren't used as a single EGLImage, import them separately
    const auto info = FormatInfo::get(buffer->dmabufAttributes()->format);
    if (!info || !info->yuvConversion()) {
        return false;
    }
    const auto planes = info->yuvConversion()->plane;
    if (buffer->dmabufAttributes()->planeCount != planes.size()) {
        return false;
    }
    for (int i = 0; i < planes.size(); i++) {
        if (!importBufferAsImage(buffer, i, planes[i].format, QSize(buffer->size().width() / planes[i].widthDivisor, buffer->size().height() / planes[i].heightDivisor))) {
            return false;
        }
    }
    return true;
}

FormatModifierMap EglBackend::supportedFormats() const
{
    EglDisplay *display = eglDisplayObject();
    if (!display) {
        return {};
    }
    return display->nonExternalOnlySupportedDrmFormats();
}

EglDisplay *EglBackend::eglDisplayObject() const
{
    if (m_renderDevice) {
        return m_renderDevice->eglDisplay();
    }
    return m_eglDisplay.get();
}

EglContext *EglBackend::openglContext() const
{
    return m_context.get();
}

std::shared_ptr<EglContext> EglBackend::openglContextRef() const
{
    return m_context;
}

} // namespace KWin

#include "moc_eglbackend.cpp"
