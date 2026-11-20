package app.polarbear;

import android.accessibilityservice.AccessibilityService;
import android.accessibilityservice.AccessibilityServiceInfo;
import android.content.Context;
import android.os.Build;
import android.util.Log;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.accessibility.AccessibilityEvent;

public class KeyboardAccessibilityService extends AccessibilityService {
    private static final String TAG = "LocalDesktopA11y";
    private static KeyboardAccessibilityService instance;

    static {
        System.loadLibrary("localdesktop");
    }

    @Override
    protected void onServiceConnected() {
        super.onServiceConnected();

        Log.i(TAG, "Keyboard accessibility service connected");
        instance = this;
        nativeSetServiceConnected(true);

        AccessibilityServiceInfo info = new AccessibilityServiceInfo();
        info.packageNames = new String[] {getPackageName()};
        info.eventTypes = AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED;
        info.notificationTimeout = 50;
        info.flags = AccessibilityServiceInfo.FLAG_REQUEST_FILTER_KEY_EVENTS;
        info.feedbackType = AccessibilityServiceInfo.FEEDBACK_GENERIC;
        setServiceInfo(info);
    }

    /**
     * Android 14 lets this service take the mouse and touchpad after Android's
     * own gesture recognizer, with no window focus, so a desktop on another
     * display can hold them while the touchscreen stays with Android.
     */
    static boolean canRouteMotion() {
        return Build.VERSION.SDK_INT >= 34 && instance != null;
    }

    /** The running service, whose windows may be accessibility overlays; null when it isn't running. */
    static Context overlayContext() {
        return instance;
    }

    /** UI thread: sends the mouse and touchpad to {@link TabletGrab} instead of the tablet's apps. */
    static boolean routeMotion(boolean routed) {
        KeyboardAccessibilityService service = instance;
        if (Build.VERSION.SDK_INT < 34 || service == null) return false;
        AccessibilityServiceInfo info = service.getServiceInfo();
        if (info == null) return false;
        int sources = routed ? InputDevice.SOURCE_MOUSE : 0;
        info.setMotionEventSources(sources);
        service.setServiceInfo(info);
        AccessibilityServiceInfo applied = service.getServiceInfo();
        int now = applied == null ? -1 : applied.getMotionEventSources();
        Log.i(TAG, "Motion sources 0x" + Integer.toHexString(now));
        return now == sources;
    }

    private static int routedEvents;
    private static long routedLoggedAt;

    /** Routed motion is hot: a count at most every two seconds keeps a test observable. */
    private static void noteRoutedMotion(MotionEvent event) {
        routedEvents++;
        long now = android.os.SystemClock.uptimeMillis();
        if (now - routedLoggedAt < 2000) return;
        Log.i(TAG, "Routed motion: " + routedEvents + " events, last source 0x"
                + Integer.toHexString(event.getSource()) + " action " + event.getActionMasked());
        routedEvents = 0;
        routedLoggedAt = now;
    }

    @Override
    public void onMotionEvent(MotionEvent event) {
        // Whatever ended the grab, Android gets its mouse and touchpad back.
        if (!TabletGrab.isRouted()) {
            Log.i(TAG, "Motion after the grab ended; Android gets the mouse back");
            routeMotion(false);
            return;
        }
        noteRoutedMotion(event);
        TabletGrab.routeMotion(event);
    }

    @Override
    protected boolean onKeyEvent(KeyEvent event) {
        // A desktop on another display holding the mouse through this service
        // also holds the keyboard; nothing else is focused for it.
        if (TabletGrab.isRouted()) return TabletGrab.routeKey(event) || super.onKeyEvent(event);
        // The tablet controller holds the keyboard for a desktop on another
        // display (and watches for its release keys). With per-display focus
        // Portal's own window can stay focused on the monitor meanwhile.
        if (TabletGrab.active) return super.onKeyEvent(event);
        if (SessionControls.onShortcut(event)) return true;
        if (ComposeOverlay.isShowing() || SessionControls.isUtilityVisible()) return super.onKeyEvent(event);
        if (isMouseSideButton(event)) {
            // Android turns mouse side buttons into Back/Forward keys; let
            // them through so PortalActivity sends them as mouse buttons.
            return super.onKeyEvent(event);
        }
        return nativeOnKeyEvent(
                event.getAction(),
                event.getKeyCode(),
                event.getScanCode(),
                event.getEventTime())
            || super.onKeyEvent(event);
    }

    /** Android reports a mouse's side buttons as Back/Forward keys. */
    static boolean isMouseSideButton(KeyEvent event) {
        int code = event.getKeyCode();
        return (code == KeyEvent.KEYCODE_BACK || code == KeyEvent.KEYCODE_FORWARD)
            && (event.getSource() & InputDevice.SOURCE_MOUSE) == InputDevice.SOURCE_MOUSE;
    }

    @Override
    public void onAccessibilityEvent(AccessibilityEvent event) {}

    @Override
    public void onInterrupt() {}

    @Override
    public void onDestroy() {
        if (instance == this) instance = null;
        TabletGrab.routeLost();
        nativeSetServiceConnected(false);
        super.onDestroy();
    }

    private static native void nativeSetServiceConnected(boolean connected);

    private static native boolean nativeOnKeyEvent(
        int action,
        int keyCode,
        int scanCode,
        long eventTime
    );
}
