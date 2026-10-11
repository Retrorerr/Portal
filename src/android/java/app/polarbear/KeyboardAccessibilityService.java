package app.polarbear;

import android.accessibilityservice.AccessibilityService;
import android.accessibilityservice.AccessibilityServiceInfo;
import android.util.Log;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.accessibility.AccessibilityEvent;

public class KeyboardAccessibilityService extends AccessibilityService {
    private static final String TAG = "LocalDesktopA11y";

    static {
        System.loadLibrary("localdesktop");
    }

    @Override
    protected void onServiceConnected() {
        super.onServiceConnected();

        Log.i(TAG, "Keyboard accessibility service connected");
        nativeSetServiceConnected(true);

        AccessibilityServiceInfo info = new AccessibilityServiceInfo();
        info.packageNames = new String[] {getPackageName()};
        info.eventTypes = AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED;
        info.notificationTimeout = 50;
        info.flags = AccessibilityServiceInfo.FLAG_REQUEST_FILTER_KEY_EVENTS;
        info.feedbackType = AccessibilityServiceInfo.FEEDBACK_GENERIC;
        setServiceInfo(info);
    }

    @Override
    protected boolean onKeyEvent(KeyEvent event) {
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
