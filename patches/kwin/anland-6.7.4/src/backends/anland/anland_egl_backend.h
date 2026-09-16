/*
    KWin - the KDE window manager
    This file is part of the KDE project.

    SPDX-License-Identifier: GPL-2.0-or-later

    OpenGL/EGL render backend for the anland backend. It renders the KWin scene
    directly into the dmabuf buffers provided by the display daemon (imported by
    fd) and tells the daemon to present them. Modeled on VirtualEglBackend, with
    the render target being one of the daemon's dmabufs instead of an internal
    FBO. The consumer rotates the buffer index externally (shared memory), so the
    layer keeps per-buffer accumulated damage (buffer-age equivalent), exactly
    like weston's backend-anland.
*/
#pragma once

#include "core/drm_formats.h"
#include "core/outputlayer.h"
#include "opengl/eglbackend.h"
#include "utils/damagejournal.h"

#include <QSize>

#include <array>
#include <map>
#include <memory>
#include <optional>
#include <sys/types.h>

extern "C" {
#include "display_producer.h"
#include "protocol.h"
}

namespace KWin
{
class GLFramebuffer;
class GLTexture;
class DrmDevice;
class OutputFrame;
class AnlandBackend;
class AnlandEglBackend;
class AnlandOutput;

class AnlandEglLayer : public OutputLayer
{
public:
    AnlandEglLayer(BackendOutput *output, AnlandEglBackend *backend);
    ~AnlandEglLayer() override;

    std::optional<OutputLayerBeginFrameInfo> doBeginFrame() override;
    bool doEndFrame(const Region &renderedDeviceRegion, const Region &damagedDeviceRegion, OutputFrame *frame) override;
    DrmDevice *scanoutDevice() const override;
    FormatModifierMap supportedDrmFormats() const override;
    bool importBuffers(int count);
    void releaseBuffers() override;

private:
    void onOutputTransformChanged();

    AnlandEglBackend *const m_backend;
    AnlandOutput *m_output;
    display_ctx *const m_display;

    int m_bufCount = 0;
    int m_currentIndex = 0;
    std::array<std::shared_ptr<GLTexture>, MAX_BUFS> m_textures;
    std::array<std::unique_ptr<GLFramebuffer>, MAX_BUFS> m_fbos;
    std::array<std::optional<RenderTarget>, MAX_BUFS> m_renderTargets;
    DamageJournal m_damageJournal;
    std::array<uint64_t, MAX_BUFS> m_bufferDamageSequence{};
    uint64_t m_damageSequence = 0;
    uint64_t m_fullRepaintCount = 0;
    uint64_t m_journalFallbackCount = 0;
};

class AnlandEglBackend : public EglBackend
{
    Q_OBJECT

public:
    AnlandEglBackend(AnlandBackend *b);
    ~AnlandEglBackend() override;

    bool init() override;
    QList<OutputLayer *> compatibleOutputLayers(BackendOutput *output) override;
    DrmDevice *drmDevice() const override;

    /** Client dma-buf support on a backend with no DRM render node.
     *
     * Advertises only driver-reported importable LINEAR layouts for 8-bit
     * RGB(A) and imports them through the surfaceless EGL display (the same
     * EXT_image_dma_buf_import path the presentation dmabufs already use).
     * Anything else fails closed, so clients fall back to SHM exactly as
     * before. No explicit-sync support is added: client->KWin stays implicit,
     * separate from the Android BufferQueue fence ownership. */
    bool initClientDmabuf();
    bool testImportBuffer(GraphicsBuffer *buffer) override;
    FormatModifierMap supportedFormats() const override;
    EGLImageKHR importBufferAsImage(GraphicsBuffer *buffer) override;
    EGLImageKHR importBufferAsImage(GraphicsBuffer *buffer, int plane, int format, const QSize &size) override;

    AnlandBackend *backend() const
    {
        return m_backend;
    }
    display_ctx *display() const;

private:
    bool initializeEgl();

    void addOutput(BackendOutput *output);
    void removeOutput(BackendOutput *output);

    AnlandBackend *m_backend;
    std::map<BackendOutput *, std::unique_ptr<AnlandEglLayer>> m_outputs;
    /** Conservative client-visible dma-buf set (LINEAR 8-bit RGB(A) only).
     * Empty until initClientDmabuf() proves at least one importable layout
     * against the live surfaceless EGL display. */
    FormatModifierMap m_clientFormats;
};

} // namespace KWin
