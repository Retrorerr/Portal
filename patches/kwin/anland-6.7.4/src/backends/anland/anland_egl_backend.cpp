/*
    KWin - the KDE window manager
    This file is part of the KDE project.

    SPDX-License-Identifier: GPL-2.0-or-later
*/
#include "anland_egl_backend.h"
#include "anland_backend.h"
#include "anland_logging.h"
#include "anland_output.h"

// kwin
#include "core/graphicsbuffer.h" // DmaBufAttributes
#include "core/output.h" // OutputTransform
#include "opengl/egldisplay.h"
#include "opengl/eglcontext.h"
#include "opengl/eglnativefence.h"
#include "opengl/eglutils_p.h"
#include "opengl/glutils.h"
#include "utils/filedescriptor.h"
#include "wayland/linuxdmabufv1clientbuffer.h"
#include "wayland_server.h"

#include <drm_fourcc.h>
#include <sys/types.h>
#include <unistd.h>

#ifndef EGL_PLATFORM_SURFACELESS_MESA
#define EGL_PLATFORM_SURFACELESS_MESA 0x31DD
#endif

namespace KWin
{

/*
 * The daemon's screen_info.format / buf_info.format uses the consumer-side
 * pixel-format enum (see common/protocol.h). 1 == RGBA_8888 in Android memory
 * layout, which is ABGR8888 in DRM fourcc terms; everything else is treated as
 * XRGB8888. Mirrors protocol_format_to_drm() in weston's backend-anland.
 */
static uint32_t protocol_format_to_drm(uint32_t fmt)
{
    switch (fmt) {
    case 1:
        return DRM_FORMAT_ABGR8888;
    default:
        return DRM_FORMAT_XRGB8888;
    }
}

AnlandEglLayer::AnlandEglLayer(BackendOutput *output, AnlandEglBackend *backend)
    : OutputLayer(output, OutputLayerType::Primary)
    , m_backend(backend)
    , m_output(static_cast<AnlandOutput *>(output))
    , m_display(backend->display())
{
    // React to runtime orientation changes (System Settings / kscreen-doctor)
    // through the output's transformChanged signal instead of polling the transform
    // in the per-frame render path. Qt drops the connection automatically when this
    // layer (a QObject via OutputLayer) or the output is destroyed.
    connect(m_output, &BackendOutput::transformChanged, this, &AnlandEglLayer::onOutputTransformChanged);
}

AnlandEglLayer::~AnlandEglLayer()
{
    // Avoid leaving a dangling pointer in the output when we're destroyed without a
    // removeOutput() call (e.g. ~AnlandEglBackend clearing m_outputs).
    if (m_output && m_output->eglLayer() == this) {
        m_output->setEglLayer(nullptr);
    }
    releaseBuffers();
}

void AnlandEglLayer::releaseBuffers()
{
    // Destroying the GL textures/framebuffers needs the context current. Callers
    // (backend state machine on fallback, ~AnlandEglLayer) may run outside a frame.
    m_backend->openglContext()->makeCurrent();

    for (int i = 0; i < MAX_BUFS; i++) {
        m_fbos[i].reset();
        m_textures[i].reset();
        m_renderTargets[i].reset();
    }
    m_damageJournal.clear();
    m_bufferDamageSequence.fill(0);
    m_damageSequence = 0;
    m_bufCount = 0;
}

bool AnlandEglLayer::importBuffers(int count)
{
    m_backend->openglContext()->makeCurrent();

    releaseBuffers();

    // The consumer reads this dmabuf top-down, while GL renders bottom-up, so the
    // content transform always carries a vertical flip. On top of that we fold in
    // the output's configured rotation, so the scene is rendered pre-rotated into
    // the (fixed-size, landscape) dmabuf — that is what lets the very same buffer
    // drive a portrait / 180° display. KWin's renderer (RenderTarget/RenderViewport)
    // bakes this single transform into the root projection with no extra copy; it is
    // the official 6.x replacement for the old GLFramebuffer::setYInverted() flag the
    // 5.27 patch carried. Combining the output rotation with FlipY mirrors the DRM
    // backend exactly (drmOutput()->transform().combine(OutputTransform::FlipY)).
    // The tag is sticky on each texture; after import it is only ever updated
    // reactively, in onOutputTransformChanged().
    const OutputTransform contentTransform = m_output->transform().combine(OutputTransform::FlipY);

    for (int i = 0; i < count; i++) {
        const int fd = get_dmabuf_fd_at(m_display, i);
        buf_info info;
        if (fd < 0 || get_dmabuf_info_at(m_display, i, &info) < 0) {
            qCWarning(KWIN_ANLAND) << "failed to get dmabuf info for buffer" << i;
            releaseBuffers();
            return false;
        }

        /* The per-buffer width/height come from the consumer's native resolution
         * (buf_info, filled by collect_dmabufs). If it differs from the current
         * OutputMode, resize the output to match — the consumer may have rotated or
         * switched display modes. All buffers in a set share the same size, so we
         * only need to check the first buffer. */
        if (i == 0) {
            const QSize bufSize(info.width, info.height);
            if (bufSize != m_output->modeSize() && bufSize.isValid()) {
                qCInfo(KWIN_ANLAND) << "dmabuf size changed, resizing output to" << bufSize;
                m_output->resize(bufSize);
            }
        }
        const QSize actual(info.width, info.height);

        DmaBufAttributes attrs;
        attrs.planeCount = 1;
        attrs.width = actual.width();
        attrs.height = actual.height();
        attrs.format = protocol_format_to_drm(info.format);
        attrs.modifier = info.modifier;
        // The producer owns the dmabuf fd; DmaBufAttributes (and the EGLImage we
        // hand the fd to) must not close it, so dup() into the owning slot.
        attrs.fd[0] = FileDescriptor(dup(fd));
        attrs.offset[0] = static_cast<int>(info.offset);
        attrs.pitch[0] = static_cast<int>(info.stride);

        // EglBackend::importDmaBufAsTexture() builds the EGLImage and wraps it in
        // a GLTexture in one step (the 5.27-era manual EGLImageKHR +
        // EGLImageTexture(...) dance is gone in 6.x).
        std::shared_ptr<GLTexture> texture = m_backend->importDmaBufAsTexture(attrs);
        if (!texture) {
            qCWarning(KWIN_ANLAND) << "failed to import dmabuf" << i << "as texture";
            releaseBuffers();
            return false;
        }

        texture->setContentTransform(contentTransform);
        auto fbo = std::make_unique<GLFramebuffer>(texture.get());
        if (!fbo->valid()) {
            qCWarning(KWIN_ANLAND) << "framebuffer for dmabuf" << i << "is not complete";
            releaseBuffers();
            return false;
        }

        qCDebug(KWIN_ANLAND) << "imported buffer" << i << "fd" << fd << actual
                             << "fmt" << Qt::hex << attrs.format << "mod" << attrs.modifier;

        m_textures[i] = std::move(texture);
        m_fbos[i] = std::move(fbo);
        m_renderTargets[i].emplace(m_fbos[i].get());
    }

    m_bufCount = count;
    // New or re-imported slots have unknown contents. Clearing the journal and
    // zeroing their last sequence makes doBeginFrame request a full repaint.
    m_damageJournal.clear();
    m_damageJournal.setCapacity(MAX_BUFS * 4);
    m_bufferDamageSequence.fill(0);
    m_damageSequence = 0;
    m_fullRepaintCount++;
    return true;
}

void AnlandEglLayer::onOutputTransformChanged()
{
    const OutputTransform contentTransform = m_output->transform().combine(OutputTransform::FlipY);
    for (int i = 0; i < m_bufCount; i++) {
        m_textures[i]->setContentTransform(contentTransform);
        // The cached RenderTarget captured the old transform; rebuild it so the
        // renderer picks up the new content transform on the next doBeginFrame.
        m_renderTargets[i].emplace(m_fbos[i].get());
    }
    m_damageJournal.clear();
    m_bufferDamageSequence.fill(0);
    m_damageSequence = 0;
    m_fullRepaintCount++;
    addDeviceRepaint(Region::infinite());
}

std::optional<OutputLayerBeginFrameInfo> AnlandEglLayer::doBeginFrame()
{
    m_backend->openglContext()->makeCurrent();

    m_currentIndex = get_selected_idx(m_display);
    if (m_currentIndex < 0 || m_currentIndex >= m_bufCount || !m_renderTargets[m_currentIndex]) {
        qCWarning(KWIN_ANLAND) << "invalid Anland selected buffer" << m_currentIndex << "of" << m_bufCount;
        return std::nullopt;
    }

    const uint64_t lastSequence = m_bufferDamageSequence[m_currentIndex];
    const bool stateGap = lastSequence > m_damageSequence;
    const uint64_t age = (lastSequence == 0 || stateGap)
        ? 0
        : (m_damageSequence - lastSequence + 1);
    const bool full = age == 0 || age > static_cast<uint64_t>(m_damageJournal.capacity());
    const Region repaint = full
        ? Region::infinite()
        : m_damageJournal.accumulate(static_cast<int>(age), Region::infinite());
    if (full) {
        m_fullRepaintCount++;
        if (stateGap || (lastSequence != 0 && age > static_cast<uint64_t>(m_damageJournal.capacity()))) {
            m_journalFallbackCount++;
        }
    }
    qCDebug(KWIN_ANLAND) << "anland.damage buffer=" << m_currentIndex
                         << "age=" << age
                         << "full=" << full
                         << "stateGap=" << stateGap
                         << "sequence=" << m_damageSequence
                         << "fullRepaints=" << m_fullRepaintCount
                         << "journalFallbacks=" << m_journalFallbackCount;

    return OutputLayerBeginFrameInfo{
        .renderTarget = *m_renderTargets[m_currentIndex],
        .repaint = repaint,
    };
}

bool AnlandEglLayer::doEndFrame(const Region &renderedDeviceRegion, const Region &damagedDeviceRegion, OutputFrame *frame)
{
    const bool hasDamage = !damagedDeviceRegion.isEmpty();
    m_output->setFrameDamage(hasDamage);
    if (!hasDamage) {
        // The consumer did select a slot for this scheduled KWin frame, but
        // the compositor produced no new pixels. Do not manufacture a render
        // fence or queue identical contents; the producer sends NO_DAMAGE and
        // the consumer cancels the slot with -1.
        set_render_fence(m_display, -1);
        return true;
    }
    glFlush(); // flush pending rendering commands into the dmabuf.
    m_damageSequence++;
    m_damageJournal.add(damagedDeviceRegion);
    m_bufferDamageSequence[m_currentIndex] = m_damageSequence;
    qCDebug(KWIN_ANLAND) << "anland.damage commit sequence=" << m_damageSequence
                         << "buffer=" << m_currentIndex
                         << "damagedRects=" << damagedDeviceRegion.rects().size()
                         << "bounding=" << damagedDeviceRegion.boundingRect();

    // Instead of CPU-blocking on glFinish, create a fence for the just-submitted
    // GPU work and hand it to the consumer (via the transport). The consumer passes
    // it to ANativeWindow_queueBuffer, so SurfaceFlinger waits on it GPU-side before
    // scanout -- letting us submit the buffer before its render completes.
    EGLNativeFence fence{m_backend->openglContext()->displayObject()};
    set_render_fence(m_display, fence.takeFileDescriptor().take());
    return true;
}

DrmDevice *AnlandEglLayer::scanoutDevice() const
{
    return m_backend->drmDevice();
}

FormatModifierMap AnlandEglLayer::supportedDrmFormats() const
{
    return {};
}

AnlandEglBackend::AnlandEglBackend(AnlandBackend *b)
    : EglBackend()
    , m_backend(b)
{
}

AnlandEglBackend::~AnlandEglBackend()
{
    m_backend->setSceneEglDisplayObject(nullptr);
    m_outputs.clear();
    cleanup();
}

display_ctx *AnlandEglBackend::display() const
{
    return m_backend->display();
}

DrmDevice *AnlandEglBackend::drmDevice() const
{
    return m_backend->drmDevice();
}

bool AnlandEglBackend::initializeEgl()
{
    const RenderDevice *renderDevice = m_backend->renderDevice();
    if (!initClientExtensions(renderDevice != nullptr)) {
        return false;
    }
    if (renderDevice) {
        setRenderDevice(m_backend->renderDevice());
        return true;
    }

    if (!hasClientExtension(QByteArrayLiteral("EGL_MESA_platform_surfaceless"))) {
        qCWarning(KWIN_ANLAND) << "EGL_MESA_platform_surfaceless is unsupported";
        return false;
    }

    const EGLDisplay display = eglGetPlatformDisplayEXT(EGL_PLATFORM_SURFACELESS_MESA, EGL_DEFAULT_DISPLAY, nullptr);
    if (display == EGL_NO_DISPLAY) {
        qCWarning(KWIN_ANLAND) << "eglGetPlatformDisplayEXT(surfaceless) failed" << getEglErrorString();
        return false;
    }
    if (!setEglDisplay(display)) {
        qCWarning(KWIN_ANLAND) << "Could not initialize the surfaceless EGL display" << getEglErrorString();
        return false;
    }
    m_backend->setSceneEglDisplayObject(eglDisplayObject());
    qCInfo(KWIN_ANLAND) << "using KGSL surfaceless EGL without a DRM render device";
    return true;
}

bool AnlandEglBackend::init()
{
    if (!initializeEgl()) {
        qCWarning(KWIN_ANLAND) << "Could not initialize egl";
        return false;
    }
    if (!createContext()) {
        qCWarning(KWIN_ANLAND) << "Could not initialize rendering context";
        return false;
    }

    initWayland();

    // Client dma-buf is best-effort: without it every client silently falls
    // back to SHM exactly as before, so a failure here must never fail the
    // session (unlike the presentation EGL setup above).
    if (!initClientDmabuf()) {
        qCWarning(KWIN_ANLAND) << "client linux-dmabuf unavailable; clients fall back to SHM";
    }

    const auto outputs = m_backend->outputs();
    for (BackendOutput *output : outputs) {
        addOutput(output);
    }

    connect(m_backend, &AnlandBackend::outputAdded, this, &AnlandEglBackend::addOutput);
    connect(m_backend, &AnlandBackend::outputRemoved, this, &AnlandEglBackend::removeOutput);
    return true;
}

void AnlandEglBackend::addOutput(BackendOutput *output)
{
    openglContext()->makeCurrent();
    auto *anlandOutput = static_cast<AnlandOutput *>(output);
    auto layer = std::make_unique<AnlandEglLayer>(output, this);
    // Let AnlandBackend reach this layer through its output (output->eglLayer()).
    anlandOutput->setEglLayer(layer.get());
    m_outputs[output] = std::move(layer);
}

bool AnlandEglBackend::initClientDmabuf()
{
    if (!WaylandServer::self()) {
        return false;
    }
    EglDisplay *display = eglDisplayObject();
    if (!display) {
        return false;
    }
    // Start conservatively: only layouts the live surfaceless EGL display
    // reports as importable, restricted to LINEAR 8-bit RGB(A). Anything else
    // (tiled/UBWC, YUV, vendor modifiers) fails closed to SHM for now.
    static constexpr uint32_t kClientFormats[] = {
        DRM_FORMAT_XRGB8888,
        DRM_FORMAT_ARGB8888,
        DRM_FORMAT_XBGR8888,
        DRM_FORMAT_ABGR8888,
    };
    const FormatModifierMap &reported = display->nonExternalOnlySupportedDrmFormats();
    FormatModifierMap advertised;
    for (const uint32_t format : kClientFormats) {
        const auto it = reported.find(format);
        if (it == reported.end()) {
            continue;
        }
        ModifierList linear;
        for (const uint64_t modifier : std::as_const(*it)) {
            if (modifier == DRM_FORMAT_MOD_LINEAR) {
                linear.insert(modifier);
            }
        }
        if (!linear.empty()) {
            advertised.insert(format, linear);
        }
    }
    if (advertised.empty()) {
        qCWarning(KWIN_ANLAND) << "surfaceless EGL reports no importable LINEAR RGB(A) layout; client dmabuf disabled";
        return false;
    }
    m_clientFormats = advertised;
    // No DRM render node exists on this backend (surfaceless KGSL by design),
    // so there is no device id to advertise. dev 0 keeps every tranche tied
    // to the single main device instead of fabricating an msm node clients
    // cannot open; worst case a client ignores the table and uses SHM.
    m_tranches = QList<LinuxDmaBufV1Feedback::Tranche>{
        LinuxDmaBufV1Feedback::Tranche{
            .device = static_cast<dev_t>(0),
            .flags = LinuxDmaBufV1Feedback::TrancheFlag::Sampling,
            .formatTable = advertised,
        },
    };

    LinuxDmaBufV1ClientBufferIntegration *dmabuf = waylandServer()->linuxDmabuf();
    dmabuf->setRenderBackend(this);
    dmabuf->setSupportedFormatsWithModifiers(m_tranches);
    // Deliberately NOT calling waylandServer()->setRenderBackend(): that enables
    // the linux-drm-syncobj global by dereferencing backend->drmDevice(), which
    // is null by design here, and client explicit sync is out of scope (the
    // client->KWin boundary stays implicit).

    QStringList names;
    for (auto it = advertised.constBegin(); it != advertised.constEnd(); ++it) {
        names << QStringLiteral("0x%1").arg(it.key(), 8, 16, QChar::fromLatin1('0'));
    }
    qCInfo(KWIN_ANLAND) << "client linux-dmabuf ready:" << advertised.size() << "formats (" << names.join(QStringLiteral(", ")) << "), LINEAR only, device=none";
    return true;
}

bool AnlandEglBackend::testImportBuffer(GraphicsBuffer *buffer)
{
    const DmaBufAttributes *attrs = buffer ? buffer->dmabufAttributes() : nullptr;
    if (!attrs) {
        return false;
    }
    const auto it = m_clientFormats.find(attrs->format);
    if (it == m_clientFormats.end() || !it->contains(attrs->modifier)) {
        return false;
    }
    return importBufferAsImage(buffer) != EGL_NO_IMAGE_KHR;
}

FormatModifierMap AnlandEglBackend::supportedFormats() const
{
    return m_clientFormats;
}

EGLImageKHR AnlandEglBackend::importBufferAsImage(GraphicsBuffer *buffer)
{
    EglDisplay *display = eglDisplayObject();
    if (!display) {
        return EGL_NO_IMAGE_KHR;
    }
    return display->importBufferAsImage(buffer);
}

EGLImageKHR AnlandEglBackend::importBufferAsImage(GraphicsBuffer *buffer, int plane, int format, const QSize &size)
{
    EglDisplay *display = eglDisplayObject();
    if (!display) {
        return EGL_NO_IMAGE_KHR;
    }
    return display->importBufferAsImage(buffer, plane, format, size);
}
void AnlandEglBackend::removeOutput(BackendOutput *output)
{
    openglContext()->makeCurrent();
    static_cast<AnlandOutput *>(output)->setEglLayer(nullptr);
    m_outputs.erase(output);
}

QList<OutputLayer *> AnlandEglBackend::compatibleOutputLayers(BackendOutput *output)
{
    auto it = m_outputs.find(output);
    if (it == m_outputs.end()) {
        return {};
    }
    return {it->second.get()};
}

} // namespace KWin

#include "moc_anland_egl_backend.cpp"
