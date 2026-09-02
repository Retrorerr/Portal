package app.polarbear;

import android.app.Activity;
import android.content.Context;
import android.graphics.Color;
import android.view.Gravity;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.View;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;
import android.widget.FrameLayout;

import java.lang.ref.WeakReference;

/**
 * A tiny editor used solely to give Android's IME an InputConnection while the native Wayland
 * surface remains the visible UI.  It is kept transparent and off the content area so it does
 * not steal pointer input from the nested desktop.
 */
public final class SoftKeyboardBridge {
    private static final String TAG = "LocalDesktopIme";
    private static WeakReference<Activity> activityRef = new WeakReference<>(null);
    private static BridgeEditText editor;

    static {
        System.loadLibrary("localdesktop");
    }

    private SoftKeyboardBridge() {}

    public static void show(final Activity activity) {
        if (activity == null) {
            return;
        }
        activity.runOnUiThread(() -> {
            activityRef = new WeakReference<>(activity);
            BridgeEditText input = ensureEditor(activity);
            input.setVisibility(View.VISIBLE);
            input.requestFocus();
            InputMethodManager manager =
                (InputMethodManager) activity.getSystemService(Context.INPUT_METHOD_SERVICE);
            if (manager != null) {
                input.postDelayed(() -> manager.showSoftInput(input, InputMethodManager.SHOW_IMPLICIT), 50);
            }
        });
    }

    public static void hide(final Activity activity) {
        if (activity == null) {
            return;
        }
        activity.runOnUiThread(() -> {
            BridgeEditText input = editor;
            if (input == null) {
                return;
            }
            InputMethodManager manager =
                (InputMethodManager) activity.getSystemService(Context.INPUT_METHOD_SERVICE);
            if (manager != null) {
                manager.hideSoftInputFromWindow(input.getWindowToken(), 0);
            }
            input.clearFocus();
            input.setVisibility(View.INVISIBLE);
        });
    }

    private static BridgeEditText ensureEditor(Activity activity) {
        if (editor != null && editor.getContext() == activity) {
            return editor;
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
        input.setVisibility(View.INVISIBLE);
        input.setAlpha(0.01f);
        input.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);

        FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(1, 1);
        params.gravity = Gravity.BOTTOM | Gravity.START;
        params.leftMargin = 1;
        params.bottomMargin = 1;
        ((FrameLayout) root).addView(input, params);
        editor = input;
        return input;
    }

    private static final class BridgeEditText extends EditText {
        BridgeEditText(Context context) {
            super(context);
            setInputType(android.text.InputType.TYPE_CLASS_TEXT
                | android.text.InputType.TYPE_TEXT_FLAG_MULTI_LINE
                | android.text.InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        }

        @Override
        public boolean onCheckIsTextEditor() {
            return true;
        }

        @Override
        public InputConnection onCreateInputConnection(EditorInfo outAttrs) {
            outAttrs.inputType = android.text.InputType.TYPE_CLASS_TEXT
                | android.text.InputType.TYPE_TEXT_FLAG_MULTI_LINE
                | android.text.InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS;
            outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI;
            return new BaseInputConnection(this, true) {
                @Override
                public boolean commitText(CharSequence text, int newCursorPosition) {
                    if (text != null && text.length() > 0) {
                        nativeOnTextCommit(text.toString());
                    }
                    return true;
                }

                @Override
                public boolean deleteSurroundingText(int beforeLength, int afterLength) {
                    // Backspace is represented as an ASCII control commit; the native side maps
                    // it to the evdev Delete/Backspace key rather than changing this editor's text.
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
            };
        }
    }

    private static native void nativeOnTextCommit(String text);
}
