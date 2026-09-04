package app.polarbear;

import android.app.Activity;
import android.content.Context;
import android.graphics.Color;
import android.text.InputType;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.View;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.hardware.input.InputManager;
import android.util.Log;
import android.view.InputDevice;
import android.widget.EditText;
import android.widget.FrameLayout;

/**
 * A tiny editor used solely to give Android's IME an InputConnection while the native Wayland
 * surface remains the visible UI. All view access is serialized on the Activity UI thread. The
 * editor is one pixel, transparent, and marked not-important-for-accessibility so it cannot
 * steal pointer input or appear as a second control in the accessibility tree.
 */
public final class SoftKeyboardBridge {
    private static final String TAG = "LocalDesktopIme";
    private static final int MAX_COMMIT_CHARS = 64 * 1024;
    private static BridgeEditText editor;
    private static Activity editorActivity;
    private static InputManager monitoredInputManager;
    private static InputManager.InputDeviceListener inputDeviceListener;

    static {
        System.loadLibrary("localdesktop");
    }

    private SoftKeyboardBridge() {}

    /** Monitor physical keyboard hotplug using Android's authoritative input-device API. */
    public static void startHardwareKeyboardMonitor(final Activity activity) {
        if (activity == null) {
            return;
        }
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                InputManager manager =
                    (InputManager) activity.getSystemService(Context.INPUT_SERVICE);
                if (manager == null) {
                    nativeOnInputDevicesChanged(false, false);
                    return;
                }
                if (monitoredInputManager != manager || inputDeviceListener == null) {
                    if (monitoredInputManager != null && inputDeviceListener != null) {
                        monitoredInputManager.unregisterInputDeviceListener(inputDeviceListener);
                    }
                    inputDeviceListener = new InputManager.InputDeviceListener() {
                        @Override public void onInputDeviceAdded(int deviceId) { publishKeyboardState(); }
                        @Override public void onInputDeviceRemoved(int deviceId) { publishKeyboardState(); }
                        @Override public void onInputDeviceChanged(int deviceId) { publishKeyboardState(); }
                    };
                    monitoredInputManager = manager;
                    manager.registerInputDeviceListener(inputDeviceListener, null);
                }
                publishKeyboardState();
            }
        });
    }

    private static void publishKeyboardState() {
        boolean hasHw = hasPhysicalKeyboard();
        boolean hasDesktop = hasDesktopInput();
        Log.i(TAG, "publishKeyboardState: hasPhysicalKeyboard=" + hasHw + ", hasDesktopInput=" + hasDesktop);
        nativeOnInputDevicesChanged(hasHw, hasDesktop);
    }

    private static boolean hasPhysicalKeyboard() {
        InputManager manager = monitoredInputManager;
        if (manager == null) {
            return false;
        }
        for (int id : manager.getInputDeviceIds()) {
            InputDevice device = manager.getInputDevice(id);
            if (device == null || device.isVirtual() || !device.isExternal()) {
                continue;
            }
            boolean keyboardSource =
                (device.getSources() & InputDevice.SOURCE_KEYBOARD) == InputDevice.SOURCE_KEYBOARD;
            if (keyboardSource && device.getKeyboardType() == InputDevice.KEYBOARD_TYPE_ALPHABETIC) {
                return true;
            }
        }
        return false;
    }

    private static boolean hasDesktopInput() {
        InputManager manager = monitoredInputManager;
        if (manager == null) {
            return false;
        }
        for (int id : manager.getInputDeviceIds()) {
            InputDevice device = manager.getInputDevice(id);
            if (device == null || device.isVirtual() || !device.isExternal()) {
                continue;
            }
            int sources = device.getSources();
            boolean isAlphaKeyb = (sources & InputDevice.SOURCE_KEYBOARD) == InputDevice.SOURCE_KEYBOARD
                && device.getKeyboardType() == InputDevice.KEYBOARD_TYPE_ALPHABETIC;
            boolean isPointer = (sources & InputDevice.SOURCE_MOUSE) == InputDevice.SOURCE_MOUSE
                || (sources & InputDevice.SOURCE_TOUCHPAD) == InputDevice.SOURCE_TOUCHPAD;
            if (isAlphaKeyb || isPointer) {
                return true;
            }
        }
        return false;
    }

    /** Show the IME on the Android UI thread without a timing-dependent sleep. */
    public static void show(final Activity activity) {
        Log.i(TAG, "SoftKeyboardBridge.show() called");
        if (activity == null || activity.isFinishing() || activity.isDestroyed()) {
            Log.w(TAG, "SoftKeyboardBridge.show() ignored: activity is null/finishing/destroyed");
            return;
        }
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (activity.isFinishing() || activity.isDestroyed()) {
                    Log.w(TAG, "SoftKeyboardBridge.show() runOnUiThread ignored: activity is finishing/destroyed");
                    return;
                }
                BridgeEditText input = ensureEditor(activity);
                if (hasPhysicalKeyboard()) {
                    Log.i(TAG, "Physical keyboard present; suppressing soft input");
                    hide(activity);
                    return;
                }
                input.setVisibility(View.VISIBLE);
                input.setFocusableInTouchMode(true);
                boolean focusRequested = input.requestFocus();
                Log.i(TAG, "SoftKeyboardBridge editor requestFocus() result=" + focusRequested);
                InputMethodManager manager =
                    (InputMethodManager) activity.getSystemService(Context.INPUT_METHOD_SERVICE);
                if (manager != null) {
                    // Posting to the UI queue waits for focus/window attachment deterministically;
                    // unlike postDelayed it does not guess an emulator-specific timing budget.
                    input.post(new Runnable() {
                        @Override
                        public void run() {
                            Log.i(TAG, "Requesting showSoftInput for bridge editor");
                            if (!manager.showSoftInput(input, InputMethodManager.SHOW_IMPLICIT)) {
                                boolean res2 = manager.showSoftInput(input, 0);
                                Log.i(TAG, "manager.showSoftInput fallback result: " + res2);
                            } else {
                                Log.i(TAG, "manager.showSoftInput SHOW_IMPLICIT succeeded");
                            }
                        }
                    });
                }
            }
        });
    }

    /** Hide the IME and release the editor focus on the Android UI thread. */
    public static void hide(final Activity activity) {
        if (activity == null) {
            return;
        }
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                BridgeEditText input = editor;
                if (input == null) {
                    return;
                }
                Log.i(TAG, "Hiding soft input from window");
                InputMethodManager manager =
                    (InputMethodManager) activity.getSystemService(Context.INPUT_METHOD_SERVICE);
                if (manager != null) {
                    manager.hideSoftInputFromWindow(input.getWindowToken(), 0);
                }
                input.clearFocus();
                input.setVisibility(View.INVISIBLE);
            }
        });
    }

    private static BridgeEditText ensureEditor(Activity activity) {
        if (editor != null && editorActivity == activity && editor.getParent() != null) {
            return editor;
        }

        // NativeActivity recreation gives us a new content FrameLayout. Detach the old editor
        // from its old parent before retaining a reference to the new activity, otherwise the
        // old Activity/View tree remains reachable for the rest of the process.
        if (editor != null && editor.getParent() instanceof FrameLayout) {
            ((FrameLayout) editor.getParent()).removeView(editor);
        }

        View root = activity.findViewById(android.R.id.content);
        if (!(root instanceof FrameLayout)) {
            throw new IllegalStateException("NativeActivity content is not a FrameLayout");
        }

        BridgeEditText input = new BridgeEditText(activity);
        input.setBackgroundColor(Color.TRANSPARENT);
        input.setTextColor(Color.TRANSPARENT);
        input.setCursorVisible(false);
        input.setSingleLine(false);
        input.setFocusableInTouchMode(true);
        input.setShowSoftInputOnFocus(true);
        input.setVisibility(View.INVISIBLE);
        input.setAlpha(0.01f);
        input.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);

        FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(1, 1);
        params.gravity = Gravity.BOTTOM | Gravity.START;
        params.leftMargin = 1;
        params.bottomMargin = 1;
        ((FrameLayout) root).addView(input, params);
        editor = input;
        editorActivity = activity;
        return input;
    }

    private static final class BridgeEditText extends EditText {
        BridgeEditText(Context context) {
            super(context);
            setInputType(InputType.TYPE_CLASS_TEXT
                | InputType.TYPE_TEXT_FLAG_MULTI_LINE
                | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
            setImeOptions(EditorInfo.IME_FLAG_NO_EXTRACT_UI);
        }

        @Override
        public boolean onCheckIsTextEditor() {
            return true;
        }

        @Override
        public InputConnection onCreateInputConnection(EditorInfo outAttrs) {
            outAttrs.inputType = InputType.TYPE_CLASS_TEXT
                | InputType.TYPE_TEXT_FLAG_MULTI_LINE
                | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS;
            outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI;
            return new BaseInputConnection(this, true) {
                private void commit(CharSequence text) {
                    if (text != null && text.length() > 0) {
                        String value = text.length() > MAX_COMMIT_CHARS
                            ? text.subSequence(0, MAX_COMMIT_CHARS).toString()
                            : text.toString();
                        nativeOnTextCommit(value);
                    }
                }

                @Override
                public boolean commitText(CharSequence text, int newCursorPosition) {
                    commit(text);
                    return true;
                }

                @Override
                public boolean deleteSurroundingText(int beforeLength, int afterLength) {
                    // Backspace is represented as an ASCII control commit; the native side maps
                    // it to the evdev Backspace key rather than changing this editor's text.
                    if (beforeLength > 0) {
                        nativeOnTextCommit("\b");
                    }
                    return true;
                }

                @Override
                public boolean deleteSurroundingTextInCodePoints(int beforeLength, int afterLength) {
                    if (beforeLength > 0) {
                        nativeOnTextCommit("\b");
                    }
                    return true;
                }

                @Override
                public boolean sendKeyEvent(KeyEvent event) {
                    // Some IMEs send Enter/arrow keys through sendKeyEvent instead of commitText.
                    if (event != null && event.getAction() == KeyEvent.ACTION_DOWN) {
                        switch (event.getKeyCode()) {
                            case KeyEvent.KEYCODE_ENTER:
                                nativeOnTextCommit("\n");
                                return true;
                            case KeyEvent.KEYCODE_DEL:
                                nativeOnTextCommit("\b");
                                return true;
                            default:
                                break;
                        }
                    }
                    return super.sendKeyEvent(event);
                }

                @Override
                public boolean performEditorAction(int actionCode) {
                    nativeOnTextCommit("\n");
                    return true;
                }
            };
        }
    }

    private static native void nativeOnTextCommit(String text);
    private static native void nativeOnInputDevicesChanged(boolean hasPhysicalKeyboard, boolean hasDesktopInput);
    private static native void nativeOnHardwareKeyboardChanged(boolean present);
}
