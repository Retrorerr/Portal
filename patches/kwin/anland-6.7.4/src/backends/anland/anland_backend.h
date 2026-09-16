/*
    KWin - the KDE window manager
    This file is part of the KDE project.

    SPDX-License-Identifier: GPL-2.0-or-later

    Native KWin output+input backend that talks to the Android display daemon
    directly (via libdisplay_producer), instead of running nested inside the
    weston "anland" compositor. Port of weston/libweston/backend-anland/anland.c
    to KWin's OutputBackend architecture.
*/
#pragma once

#include "core/outputbackend.h"

#include <QByteArray>
#include <QPointF>
#include <QVector>
#include <chrono>
#include <memory>

extern "C" {
#include "display_producer.h"
#include "protocol.h"
}

class QSocketNotifier;
class QTimer;

namespace KWin
{

class AnlandOutput;
class AnlandInputDevice;
class DrmDevice;
class RenderDevice;
class EglBackend;
class EglDisplay;
class InputBackend;

class KWIN_EXPORT AnlandBackend : public OutputBackend
{
    Q_OBJECT

public:
    explicit AnlandBackend(const QString &socketPath = QString(), QObject *parent = nullptr);
    ~AnlandBackend() override;

    bool initialize() override;

    std::unique_ptr<EglBackend> createOpenGLBackend() override;
    std::unique_ptr<InputBackend> createInputBackend() override;
    QList<CompositingType> supportedCompositors() const override;
    QList<BackendOutput *> outputs() const override;

    EglDisplay *sceneEglDisplayObject() const override;

    /** Publish the EGL display owned by the Anland OpenGL backend while it is
     * alive. The surfaceless path has no RenderDevice to provide this through
     * OutputBackend's normal DRM-owned display accessor. */
    void setSceneEglDisplayObject(EglDisplay *display);

    display_ctx *display() const
    {
        return m_display;
    }
    DrmDevice *drmDevice() const;
    RenderDevice *renderDevice() const
    {
        return m_renderDevice.get();
    }
    AnlandInputDevice *inputDevice() const
    {
        return m_inputDevice.get();
    }

    /** Request the consumer slot for one real KWin repaint without blocking.
     *
     * Frame-slot scheduling state machine (main thread only, never blocks):
     *
     *   Idle --repaintScheduled--> WorkRequested --slot selected--> SlotReady
     *     --frameRequested dispatch--> (compositor traversal) --present-->
     *     AwaitingPresentation --next slot selected--> Idle
     *
     * Encoding (no traversal may start unless a fresh slot was selected):
     * - Idle:                 !m_workRequested && !m_consumerReady
     * - WorkRequested:         m_workRequested && !m_consumerReady
     * - SlotReady:             m_consumerReady (request consumed by selection)
     * - AwaitingPresentation:  AnlandOutput::m_awaitingPresent (the previous
     *                          frame completes when the NEXT slot is selected,
     *                          which is also SurfaceFlinger's latch tick for it;
     *                          there is no distinct final-present ack in the
     *                          current protocol)
     * - Fallback/Rebinding:    m_inFallback (all request state cleared)
     *
     * Rendering itself runs synchronously inside the frameRequested dispatch,
     * so there is no explicit Rendering state visible to the event loop.
     *
     * This is called from RenderLoop's repaintScheduled signal before the
     * compositor timer is armed. It sends exactly one FRAME_WANTED when none
     * is outstanding (coalesced repaints share it; damage accumulates in the
     * layers) and returns immediately. The buffer-ready eventfd notifier
     * records the selection later; Compositor::handleFrameRequested consults
     * consumeSlotForFrame() and skips the traversal until then, so
     * doBeginFrame always sees the index selected for that work item. */
    bool requestFrameWork();

    /** Consume one selected slot for the compositor traversal that is being
     * dispatched right now. Called synchronously from the frameRequested
     * dispatch (Compositor::handleFrameRequested) on the main thread.
     *
     * Returns true when a fresh consumer slot is ready: the caller must run
     * the traversal immediately (present() consumes the selection). Returns
     * false when the slot has not arrived yet: the dispatch is recorded
     * (coalesced) and skipped WITHOUT touching frame accounting, and the
     * buffer-ready notifier schedules a new dispatch on arrival. Never sends
     * FRAME_WANTED (repaintScheduled owns that) and never blocks. */
    bool consumeSlotForFrame();

    /** Complete the currently selected work item. Damaged frames are held
     * until the next selected slot proves the previous queue handoff; a
     * no-damage frame is completed immediately after its cancel request. */
    bool notifyFramePresented(bool damaged);

    /** Re-run the Workspace output layout after an output changed its mode at
     *  runtime (AnlandOutput::resize). The backend mutates the mode directly via
     *  setState() instead of going through OutputConfiguration, so — exactly like
     *  DrmBackend/VirtualBackend do after altering their output set — it must emit
     *  outputsQueried() itself. Otherwise Workspace::updateOutputs() never runs and
     *  windows keep their old geometry (the mode-changed signal alone does not
     *  trigger a relayout). */
    void notifyOutputsChanged()
    {
        Q_EMIT outputsQueried();
    }

private:
    void setupNotifiers();
    void teardownNotifiers();
    void onInputReadable();
    void onBufferReady();
    /** Wedged-consumer backstop: no slot arrived within kFrameSlotTimeoutMs
     * of the request. Warns, drops the pending request state and enters
     * fallback (mirrors the old synchronous-timeout path, minus the block). */
    void onFrameTimeout();
    /** Drop all in-flight request state (timeout, request/selection/defer
     * flags, request timing). Never touches fences or queued buffers. */
    void cancelPendingFrameWork();
    /** (Re)arm the single-shot slot-selection watchdog for one request. */
    void startFrameTimeout();
    void processInputEvent(const InputEvent &ev);
    QPointF mapInputToLogical(const QPointF &devicePoint) const;
    void onReconnectTimer();
    void enterFallback();

    // Clipboard sync — bidirectional bridge between KWin selection / consumer
    void onClipboardChanged();
    void sendClipboardToConsumer(const QByteArray &text);
    void sendClipboardToKWin(const QByteArray &text);

    // Inject UTF-8 text from the consumer's IME into the focused KWin client.
    void sendTextInputToKWin(const QByteArray &text);

    static void fallbackTrampoline(void *data);

    QString m_socketPath;
    display_ctx *m_display = nullptr;

    std::unique_ptr<RenderDevice> m_renderDevice;
    EglDisplay *m_sceneEglDisplay = nullptr;
    QVector<AnlandOutput *> m_outputs;
    std::unique_ptr<AnlandInputDevice> m_inputDevice;

    QSocketNotifier *m_inputNotifier = nullptr;
    QSocketNotifier *m_bufReadyNotifier = nullptr;
    QTimer *m_reconnectTimer = nullptr;

    bool m_consumerReady = false;
    bool m_workRequested = false;
    bool m_inFallback = false;
    /** A frameRequested dispatch fired while no slot was selected. The next
     * buffer-ready arrival schedules a replacement dispatch; coalesced (at
     * most one pending) and cleared on selection, cancel, and fallback. */
    bool m_dispatchDeferred = false;
    /** Watchdog for a consumer that accepted FRAME_WANTED but never selects.
     * Single-shot; armed per request, stopped on selection/cancel/fallback. */
    QTimer *m_frameTimeout = nullptr;
    /** m_requestSentAt is valid only while m_requestTimed is set (one
     * outstanding request). Used for request->select latency telemetry. */
    std::chrono::steady_clock::time_point m_requestSentAt{};
    bool m_requestTimed = false;
    /** Time of the most recent slot selection; valid while m_consumerReady.
     * Used for select->traversal-start latency telemetry. */
    std::chrono::steady_clock::time_point m_slotSelectedAt{};
    // Last known clipboard text — used to de-duplicate (KWin changed -> we sent ->
    // consumer sets the same text on Android -> consumer sends back to KWin).
    // QByteArray is trivially sent over the data channel as UTF-8.
    QByteArray m_clipboardText;
};

} // namespace KWin
