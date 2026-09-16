/*
    KWin - the KDE window manager
    This file is part of the KDE project.

    SPDX-License-Identifier: GPL-2.0-or-later

    Input for the anland backend. A single combined InputDevice (pointer +
    keyboard + touch) fed from the display daemon's data fd. Mirrors the single
    weston_seat that weston's backend-anland sets up.

    The device also presents the Portal touchpad identity over D-Bus
    (org.kde.KWin.InputDevice at /org/kde/KWin/InputDevice/portal_touchpad)
    so Plasma System Settings exposes natural-scroll and scroll-factor
    controls. This mirrors the retired 6.3.6 Wayland-backend touchpad patch
    (patches/kwin/debian-6.3.6/0001-wayland-portal-touchpad-scroll-settings.patch):
    the settings apply only to finger-source smooth scroll in
    pointerAxisFinger(); wheel/continuous deltas and axis-stop are untouched,
    and the Android host keeps sending raw buffer-px deltas so exactly one
    layer owns each transformation.
*/
#pragma once

#include "core/inputbackend.h"
#include "core/inputdevice.h"

#include <KConfigGroup>

namespace KWin
{

class AnlandBackend;

class AnlandInputDevice : public InputDevice
{
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "org.kde.KWin.InputDevice")
    Q_PROPERTY(bool touchpad READ isTouchpad CONSTANT)
    Q_PROPERTY(bool pointer READ isPointer CONSTANT)
    Q_PROPERTY(QString name READ name CONSTANT)
    Q_PROPERTY(QString sysName READ sysName CONSTANT)
    Q_PROPERTY(bool supportsDisableEvents READ falseValue CONSTANT)
    Q_PROPERTY(bool enabled READ trueValue CONSTANT)
    Q_PROPERTY(int supportedButtons READ zeroValue CONSTANT)
    Q_PROPERTY(bool supportsLeftHanded READ falseValue CONSTANT)
    Q_PROPERTY(bool leftHandedEnabledByDefault READ falseValue CONSTANT)
    Q_PROPERTY(bool leftHanded READ falseValue CONSTANT)
    Q_PROPERTY(bool supportsPointerAcceleration READ falseValue CONSTANT)
    Q_PROPERTY(bool supportsPointerAccelerationProfileFlat READ falseValue CONSTANT)
    Q_PROPERTY(bool supportsPointerAccelerationProfileAdaptive READ falseValue CONSTANT)
    Q_PROPERTY(bool supportsDisableWhileTyping READ falseValue CONSTANT)
    Q_PROPERTY(bool supportsDisableEventsOnExternalMouse READ falseValue CONSTANT)
    Q_PROPERTY(qreal defaultPointerAcceleration READ zeroRealValue CONSTANT)
    Q_PROPERTY(bool defaultPointerAccelerationProfileFlat READ falseValue CONSTANT)
    Q_PROPERTY(bool defaultPointerAccelerationProfileAdaptive READ falseValue CONSTANT)
    Q_PROPERTY(bool disableEventsOnExternalMouseEnabledByDefault READ falseValue CONSTANT)
    Q_PROPERTY(bool disableWhileTypingEnabledByDefault READ falseValue CONSTANT)
    Q_PROPERTY(qreal pointerAcceleration READ zeroRealValue CONSTANT)
    Q_PROPERTY(bool pointerAccelerationProfileFlat READ falseValue CONSTANT)
    Q_PROPERTY(bool pointerAccelerationProfileAdaptive READ falseValue CONSTANT)
    Q_PROPERTY(bool disableEventsOnExternalMouse READ falseValue CONSTANT)
    Q_PROPERTY(bool disableWhileTyping READ falseValue CONSTANT)
    Q_PROPERTY(int tapFingerCount READ zeroValue CONSTANT)
    Q_PROPERTY(bool supportsMiddleEmulation READ falseValue CONSTANT)
    Q_PROPERTY(bool tapToClickEnabledByDefault READ falseValue CONSTANT)
    Q_PROPERTY(bool tapAndDragEnabledByDefault READ falseValue CONSTANT)
    Q_PROPERTY(bool tapDragLockEnabledByDefault READ falseValue CONSTANT)
    Q_PROPERTY(bool middleEmulationEnabledByDefault READ falseValue CONSTANT)
    Q_PROPERTY(bool tapToClick READ falseValue CONSTANT)
    Q_PROPERTY(bool tapAndDrag READ falseValue CONSTANT)
    Q_PROPERTY(bool tapDragLock READ falseValue CONSTANT)
    Q_PROPERTY(bool middleEmulation READ falseValue CONSTANT)
    Q_PROPERTY(bool lmrTapButtonMapEnabledByDefault READ falseValue CONSTANT)
    Q_PROPERTY(bool lmrTapButtonMap READ falseValue CONSTANT)
    Q_PROPERTY(bool supportsNaturalScroll READ isTouchpad CONSTANT)
    Q_PROPERTY(bool naturalScrollEnabledByDefault READ falseValue CONSTANT)
    Q_PROPERTY(bool naturalScroll READ isNaturalScroll WRITE setNaturalScroll NOTIFY naturalScrollChanged)
    Q_PROPERTY(bool supportsScrollTwoFinger READ falseValue CONSTANT)
    Q_PROPERTY(bool supportsScrollEdge READ falseValue CONSTANT)
    Q_PROPERTY(bool supportsScrollOnButtonDown READ falseValue CONSTANT)
    Q_PROPERTY(bool scrollTwoFingerEnabledByDefault READ falseValue CONSTANT)
    Q_PROPERTY(bool scrollEdgeEnabledByDefault READ falseValue CONSTANT)
    Q_PROPERTY(bool scrollOnButtonDownEnabledByDefault READ falseValue CONSTANT)
    Q_PROPERTY(quint32 defaultScrollButton READ zeroUnsignedValue CONSTANT)
    Q_PROPERTY(bool scrollTwoFinger READ falseValue CONSTANT)
    Q_PROPERTY(bool scrollEdge READ falseValue CONSTANT)
    Q_PROPERTY(bool scrollOnButtonDown READ falseValue CONSTANT)
    Q_PROPERTY(quint32 scrollButton READ zeroUnsignedValue CONSTANT)
    Q_PROPERTY(qreal scrollFactor READ scrollFactor WRITE setScrollFactor NOTIFY scrollFactorChanged)
    Q_PROPERTY(bool supportsClickMethodAreas READ falseValue CONSTANT)
    Q_PROPERTY(bool supportsClickMethodClickfinger READ falseValue CONSTANT)
    Q_PROPERTY(bool defaultClickMethodAreas READ falseValue CONSTANT)
    Q_PROPERTY(bool defaultClickMethodClickfinger READ falseValue CONSTANT)
    Q_PROPERTY(bool clickMethodAreas READ falseValue CONSTANT)
    Q_PROPERTY(bool clickMethodClickfinger READ falseValue CONSTANT)

public:
    explicit AnlandInputDevice(QObject *parent = nullptr);
    ~AnlandInputDevice() override;

    QString name() const override;
    QString sysName() const;
    bool isEnabled() const override;
    void setEnabled(bool enabled) override;
    LEDs leds() const override;
    void setLeds(LEDs leds) override;

    bool isKeyboard() const override;
    bool isPointer() const override;
    bool isTouchpad() const override;
    bool isTouch() const override;
    bool isTabletTool() const override;
    bool isTabletPad() const override;
    bool isTabletModeSwitch() const override;
    bool isLidSwitch() const override;

    // D-Bus constant helpers (unsupported capabilities report safe defaults).
    bool falseValue() const;
    bool trueValue() const;
    int zeroValue() const;
    quint32 zeroUnsignedValue() const;
    qreal zeroRealValue() const;
    bool isNaturalScroll() const;
    void setNaturalScroll(bool enabled);
    qreal scrollFactor() const;
    void setScrollFactor(qreal factor);

    // Event injection helpers (called by AnlandBackend from the data-fd reader).
    void pointerMotionAbsolute(const QPointF &position);
    void pointerMotion(const QPointF &pos, const QPointF &delta, const QPointF &deltaUnaccel);
    void pointerButton(quint32 button, bool pressed);
    void pointerAxis(PointerAxis axis, qreal delta, qint32 deltaV120);
    // Touchpad finger-source smooth scroll (buffer-px delta, Finger source for
    // kinetic scrolling). The consumer sends raw deltas; the kcminputrc
    // scroll factor and natural-scroll inversion are applied here, exactly
    // once.
    void pointerAxisFinger(PointerAxis axis, qreal delta);
    // Terminates an active finger scroll stream (zero-delta Finger event so
    // SeatInterface emits wl_pointer.axis_stop and kinetic scrolling settles).
    void pointerAxisStop(PointerAxis axis);
    void keyboardKey(quint32 keycode, bool pressed);
    void touchDown(qint32 id, const QPointF &position);
    void touchMotion(qint32 id, const QPointF &position);
    void touchUp(qint32 id);
    void touchFrame();
    void touchCancel();

Q_SIGNALS:
    void naturalScrollChanged();
    void scrollFactorChanged();

private:
    bool m_enabled = true;
    KConfigGroup m_config;
    bool m_naturalScroll = false;
    qreal m_scrollFactor = 1.0;
};

class AnlandInputBackend : public InputBackend
{
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "org.kde.KWin.InputDeviceManager")
    Q_PROPERTY(QStringList devicesSysNames READ devicesSysNames CONSTANT)

public:
    explicit AnlandInputBackend(AnlandBackend *backend);
    ~AnlandInputBackend() override;
    using InputBackend::deviceAdded;
    using InputBackend::deviceRemoved;

    void initialize() override;
    QStringList devicesSysNames() const;

    // Plasma 6.7 KWinDevices::DevicesModel enumerates via these scriptable
    // methods (not the devicesSysNames property): without ListPointers the
    // touchpad KCM sees zero rows and hides its page entirely.
    Q_SCRIPTABLE QStringList ListPointers() const;
    Q_SCRIPTABLE QStringList ListKeyboards() const;
    Q_SCRIPTABLE QStringList ListTouch() const;

Q_SIGNALS:
    void deviceAdded(const QString &sysName);
    void deviceRemoved(const QString &sysName);

private:
    AnlandBackend *m_backend;
};

} // namespace KWin
