/*
    KWin - the KDE window manager
    This file is part of the KDE project.

    SPDX-License-Identifier: GPL-2.0-or-later
*/
#include "anland_input.h"
#include "anland_backend.h"
#include "anland_logging.h"

#include <KSharedConfig>

#include <QDBusConnection>

#include <chrono>

namespace KWin
{

static std::chrono::microseconds now()
{
    return std::chrono::duration_cast<std::chrono::microseconds>(
        std::chrono::steady_clock::now().time_since_epoch());
}

AnlandInputDevice::AnlandInputDevice(QObject *parent)
    : InputDevice(parent)
{
    const auto config = KSharedConfig::openConfig(QStringLiteral("kcminputrc"));
    m_config = config->group(QStringLiteral("Libinput"))
                   .group(QStringLiteral("0"))
                   .group(QStringLiteral("0"))
                   .group(name());
    m_naturalScroll = m_config.readEntry("NaturalScroll", false);
    m_scrollFactor = m_config.readEntry("ScrollFactor", 1.0);

    const bool registered = QDBusConnection::sessionBus().registerObject(QStringLiteral("/org/kde/KWin/InputDevice/") + sysName(),
                                                                         QStringLiteral("org.kde.KWin.InputDevice"),
                                                                         this,
                                                                         QDBusConnection::ExportAllProperties);
    qCWarning(KWIN_ANLAND) << "Portal Touchpad input device registered:" << registered
                        << "naturalScroll:" << m_naturalScroll << "scrollFactor:" << m_scrollFactor;
}

AnlandInputDevice::~AnlandInputDevice()
{
    QDBusConnection::sessionBus().unregisterObject(QStringLiteral("/org/kde/KWin/InputDevice/") + sysName());
}

QString AnlandInputDevice::name() const
{
    return QStringLiteral("Portal Touchpad");
}

QString AnlandInputDevice::sysName() const
{
    return QStringLiteral("portal_touchpad");
}

bool AnlandInputDevice::isEnabled() const
{
    return m_enabled;
}

void AnlandInputDevice::setEnabled(bool enabled)
{
    m_enabled = enabled;
}

LEDs AnlandInputDevice::leds() const
{
    return LEDs();
}

void AnlandInputDevice::setLeds(LEDs leds)
{
}

bool AnlandInputDevice::isKeyboard() const
{
    return true;
}

bool AnlandInputDevice::isPointer() const
{
    return true;
}

bool AnlandInputDevice::isTouchpad() const
{
    return true;
}

bool AnlandInputDevice::isTouch() const
{
    return true;
}

bool AnlandInputDevice::isTabletTool() const
{
    return false;
}

bool AnlandInputDevice::isTabletPad() const
{
    return false;
}

bool AnlandInputDevice::isTabletModeSwitch() const
{
    return false;
}

bool AnlandInputDevice::isLidSwitch() const
{
    return false;
}

// KWin only delivers pointer events to client apps on a wl_pointer.frame, so every
// pointer event must be followed by a pointerFrame() (exactly as the libinput
// backend does). Without it scroll is dropped entirely; cursor motion and focus
// still appear to work only because PointerInputRedirection handles those
// compositor-side, independent of client delivery.
void AnlandInputDevice::pointerMotionAbsolute(const QPointF &position)
{
    Q_EMIT InputDevice::pointerMotionAbsolute(position, now(), this);
    Q_EMIT InputDevice::pointerFrame(this);
}

void AnlandInputDevice::pointerMotion(const QPointF &pos, const QPointF &delta, const QPointF &deltaUnaccel)
{
    Q_EMIT InputDevice::pointerMotionAbsolute(pos, now(), this);
    Q_EMIT InputDevice::pointerMotion(delta, deltaUnaccel, now(), this);
    Q_EMIT InputDevice::pointerFrame(this);
}

void AnlandInputDevice::pointerButton(quint32 button, bool pressed)
{
    Q_EMIT pointerButtonChanged(button,
                                pressed ? PointerButtonState::Pressed : PointerButtonState::Released,
                                now(), this);
    Q_EMIT InputDevice::pointerFrame(this);
}

void AnlandInputDevice::pointerAxis(PointerAxis axis, qreal delta, qint32 deltaV120)
{
    const PointerAxisSource source =
        deltaV120 != 0 ? PointerAxisSource::Wheel : PointerAxisSource::Continuous;
    // 6.x pointerAxisChanged carries an explicit "inverted" (natural scroll) flag;
    // the daemon already delivers deltas in KWin's direction, so it is never set.
    Q_EMIT pointerAxisChanged(axis, delta, deltaV120, source, false, now(), this);
    Q_EMIT InputDevice::pointerFrame(this);
}

void AnlandInputDevice::pointerAxisFinger(PointerAxis axis, qreal delta)
{
    // Exactly one layer owns each transformation: the host sends raw
    // buffer-px deltas, so the kcminputrc scroll factor and natural-scroll
    // inversion are applied here, finger-source only.
    delta *= m_scrollFactor;
    if (m_naturalScroll) {
        delta = -delta;
    }
    Q_EMIT pointerAxisChanged(axis, delta, 0, PointerAxisSource::Finger, m_naturalScroll, now(), this);
    Q_EMIT InputDevice::pointerFrame(this);
}

void AnlandInputDevice::pointerAxisStop(PointerAxis axis)
{
    Q_EMIT pointerAxisChanged(axis, 0, 0, PointerAxisSource::Finger, false, now(), this);
    Q_EMIT InputDevice::pointerFrame(this);
}

void AnlandInputDevice::keyboardKey(quint32 keycode, bool pressed)
{
    Q_EMIT keyChanged(keycode,
                      pressed ? KeyboardKeyState::Pressed : KeyboardKeyState::Released,
                      now(), this);
}

void AnlandInputDevice::touchDown(qint32 id, const QPointF &position)
{
    Q_EMIT InputDevice::touchDown(id, position, now(), this);
}

void AnlandInputDevice::touchMotion(qint32 id, const QPointF &position)
{
    Q_EMIT InputDevice::touchMotion(id, position, now(), this);
}

void AnlandInputDevice::touchUp(qint32 id)
{
    Q_EMIT InputDevice::touchUp(id, now(), this);
}

void AnlandInputDevice::touchFrame()
{
    Q_EMIT InputDevice::touchFrame(this);
}

void AnlandInputDevice::touchCancel()
{
    Q_EMIT touchCanceled(this);
}

bool AnlandInputDevice::falseValue() const
{
    return false;
}

bool AnlandInputDevice::trueValue() const
{
    return true;
}

int AnlandInputDevice::zeroValue() const
{
    return 0;
}

quint32 AnlandInputDevice::zeroUnsignedValue() const
{
    return 0;
}

qreal AnlandInputDevice::zeroRealValue() const
{
    return 0;
}

bool AnlandInputDevice::isNaturalScroll() const
{
    return m_naturalScroll;
}

void AnlandInputDevice::setNaturalScroll(bool enabled)
{
    if (m_naturalScroll == enabled) {
        return;
    }
    m_naturalScroll = enabled;
    m_config.writeEntry("NaturalScroll", enabled);
    m_config.sync();
    qCWarning(KWIN_ANLAND) << "Portal Touchpad natural scroll set to" << enabled;
    Q_EMIT naturalScrollChanged();
}

qreal AnlandInputDevice::scrollFactor() const
{
    return m_scrollFactor;
}

void AnlandInputDevice::setScrollFactor(qreal factor)
{
    if (m_scrollFactor == factor) {
        return;
    }
    m_scrollFactor = factor;
    m_config.writeEntry("ScrollFactor", factor);
    m_config.sync();
    qCWarning(KWIN_ANLAND) << "Portal Touchpad scroll factor set to" << factor;
    Q_EMIT scrollFactorChanged();
}

AnlandInputBackend::AnlandInputBackend(AnlandBackend *backend)
    : m_backend(backend)
{
}

AnlandInputBackend::~AnlandInputBackend()
{
    QDBusConnection::sessionBus().unregisterObject(QStringLiteral("/org/kde/KWin/InputDevice"));
}

void AnlandInputBackend::initialize()
{
    // Register the manager object BEFORE emitting deviceAdded: a
    // late-starting KCM enumerates via ListPointers() and must find the
    // object already owned on the bus.
    const bool managerRegistered = QDBusConnection::sessionBus().registerObject(QStringLiteral("/org/kde/KWin/InputDevice"),
                                                                                 QStringLiteral("org.kde.KWin.InputDeviceManager"),
                                                                                 this,
                                                                                 QDBusConnection::ExportAllProperties | QDBusConnection::ExportAllSignals | QDBusConnection::ExportScriptableContents);
    qCWarning(KWIN_ANLAND) << "Portal input device manager registered:" << managerRegistered;

    if (AnlandInputDevice *device = m_backend->inputDevice()) {
        Q_EMIT deviceAdded(device);
        if (device->isTouchpad()) {
            Q_EMIT deviceAdded(device->sysName());
        }
    }
}

QStringList AnlandInputBackend::devicesSysNames() const
{
    if (AnlandInputDevice *device = m_backend->inputDevice()) {
        return QStringList{device->sysName()};
    }
    return QStringList{};
}

QStringList AnlandInputBackend::ListPointers() const
{
    if (AnlandInputDevice *device = m_backend->inputDevice()) {
        if (device->isPointer()) {
            return QStringList{device->sysName()};
        }
    }
    return QStringList{};
}

QStringList AnlandInputBackend::ListKeyboards() const
{
    return QStringList{};
}

QStringList AnlandInputBackend::ListTouch() const
{
    return QStringList{};
}

} // namespace KWin

#include "moc_anland_input.cpp"
