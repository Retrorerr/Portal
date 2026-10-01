package app.polarbear;

import android.app.Activity;
import android.content.Context;
import android.graphics.Color;
import android.text.Editable;
import android.text.InputType;
import android.text.Selection;
import android.text.TextWatcher;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.View;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.CompletionInfo;
import android.view.inputmethod.CorrectionInfo;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputConnectionWrapper;
import android.view.inputmethod.InputMethodManager;
import android.hardware.input.InputManager;
import android.util.Log;
import android.view.InputDevice;
import android.widget.EditText;
import android.widget.FrameLayout;
import java.util.Arrays;
import java.util.regex.Pattern;

/**
 * A tiny editor used solely to give Android's IME an InputConnection while the native Wayland
 * surface remains the visible UI. All view access is serialized on the Activity UI thread. The
 * editor is one pixel, transparent, and marked not-important-for-accessibility so it cannot
 * steal pointer input or appear as a second control in the accessibility tree.
 */
public final class SoftKeyboardBridge {
    private static final String TAG = "LocalDesktopIme";
    // Chunks stay under the native queue's 64 KiB per-commit byte cap at any UTF-8 width.
    private static final int MAX_COMMIT_CHARS = 16 * 1024;
    private static final int MAX_MIRROR_CHARS = 4096;
    // Suggestions on: keyboards then compose the current word (shown in the guest as preedit)
    // and autocorrect it on commit instead of rewriting already committed text.
    private static final int INPUT_TYPE =
        InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_MULTI_LINE;
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
            if (isAlphaKeyb || (isPointer && !isStylusCompanion(device))) {
                return true;
            }
        }
        return false;
    }

    /**
     * Bluetooth styluses such as the OnePlus Stylo 2 register an external "mouse" for their
     * buttons while paired. It is not desktop input, so it must not end tablet mode.
     */
    private static boolean isStylusCompanion(InputDevice device) {
        String name = device.getName();
        return name != null && STYLUS_NAME.matcher(name).find();
    }

    private static final Pattern STYLUS_NAME =
        Pattern.compile("\\b(stylus|stylo|pen)\\b", Pattern.CASE_INSENSITIVE);

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
                final BridgeEditText input = ensureEditor(activity);
                if (hasPhysicalKeyboard()) {
                    Log.i(TAG, "Physical keyboard present; suppressing soft input");
                    hide(activity);
                    return;
                }
                // A show starts a new guest text context (another field, or the window came
                // back), so text mirrored from the previous one must not be editable anymore.
                input.flushPreedit();
                input.resetMirror();
                input.setVisibility(View.VISIBLE);
                input.setFocusableInTouchMode(true);
                boolean focusRequested = input.requestFocus();
                Log.i(TAG, "SoftKeyboardBridge editor requestFocus() result=" + focusRequested);
                input.pendingShow = true;
                // Posting to the UI queue waits for focus/window attachment deterministically;
                // unlike postDelayed it does not guess an emulator-specific timing budget.
                input.post(new Runnable() {
                    @Override
                    public void run() {
                        input.showSoftInputIfPending();
                    }
                });
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
                input.pendingShow = false;
                InputMethodManager manager =
                    (InputMethodManager) activity.getSystemService(Context.INPUT_METHOD_SERVICE);
                if (manager != null) {
                    manager.hideSoftInputFromWindow(input.getWindowToken(), 0);
                }
                // INVISIBLE first: clearFocus() on a still-focusable view can hand focus straight
                // back to it on older releases and summon the IME again.
                input.setVisibility(View.INVISIBLE);
                input.clearFocus();
                input.flushPreedit();
                input.resetMirror();
            }
        });
    }

    /**
     * The guest caret moved without the IME's involvement (tap or click into the text), so the
     * mirrored text no longer sits before it and must not be edited by autocorrect or deletes.
     * KWin (and IBus, on the toolkit's reset) commits any preedit on that tap itself.
     */
    public static void resetContext(final Activity activity) {
        if (activity == null) {
            return;
        }
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                BridgeEditText input = editor;
                if (input != null && editorActivity == activity) {
                    input.resetMirror();
                }
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

    /**
     * The editor mirrors the text typed into the guest since the last context reset, ending at
     * the guest caret. The IME edits it like any text field (so autocorrect, recorrection and
     * composing keyboards can replace earlier text through whichever InputConnection calls they
     * use), and every change is forwarded as the minimal backspace run plus insertion that turns
     * the previously sent text into the new one.
     *
     * The word the IME is still composing goes to the guest as preedit rather than text, so the
     * usual autocorrect (the composing "i" committed as "I ") replaces it in one input-method
     * frame. Deletes are only needed for real backspaces and recorrection of committed words:
     * Qt clients such as LibreOffice apply a Backspace key after text sent behind it, so a
     * delete-then-retype correction is not reliable there.
     */
    private static final class BridgeEditText extends EditText {
        private String sent = "";
        private String sentPreedit = "";
        private int batchDepth;
        private boolean resetting;
        private boolean syncPosted;
        boolean pendingShow;

        BridgeEditText(Context context) {
            super(context);
            setInputType(INPUT_TYPE);
            setImeOptions(EditorInfo.IME_FLAG_NO_EXTRACT_UI);
            // Catches edits made through InputConnection calls not wrapped below (newer API
            // additions such as replaceText); the wrapper syncs the common calls directly.
            addTextChangedListener(new TextWatcher() {
                @Override public void beforeTextChanged(CharSequence s, int start, int count, int after) {}
                @Override public void onTextChanged(CharSequence s, int start, int before, int count) {}
                @Override public void afterTextChanged(Editable s) { scheduleSync(); }
            });
        }

        @Override
        public boolean onCheckIsTextEditor() {
            return true;
        }

        void showSoftInputIfPending() {
            if (!pendingShow || getVisibility() != View.VISIBLE) {
                return;
            }
            if (!isFocused() && !requestFocus()) {
                Log.i(TAG, "Bridge editor cannot take focus yet; retrying on window focus");
                return;
            }
            InputMethodManager manager =
                (InputMethodManager) getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
            if (manager == null) {
                return;
            }
            Log.i(TAG, "Requesting showSoftInput for bridge editor");
            if (manager.showSoftInput(this, InputMethodManager.SHOW_IMPLICIT)
                || manager.showSoftInput(this, 0)) {
                pendingShow = false;
                Log.i(TAG, "manager.showSoftInput accepted");
            } else {
                // Refused while the window lacks input focus (resume, notification shade or a
                // dialog closing); onWindowFocusChanged retries once it can host the IME.
                Log.i(TAG, "manager.showSoftInput refused; retrying on window focus");
            }
        }

        @Override
        public void onWindowFocusChanged(boolean hasWindowFocus) {
            super.onWindowFocusChanged(hasWindowFocus);
            if (hasWindowFocus) {
                showSoftInputIfPending();
            }
        }

        /** Commit the preedit shown in the guest, for resets the guest does not see itself. */
        void flushPreedit() {
            if (!sentPreedit.isEmpty()) {
                nativeOnPreeditFlush();
                sentPreedit = "";
            }
        }

        /** Forget the mirrored text without touching the guest and restart the IME on empty. */
        void resetMirror() {
            Editable content = getText();
            sentPreedit = "";
            if (content == null || (content.length() == 0 && sent.isEmpty())) {
                sent = "";
                return;
            }
            resetting = true;
            try {
                BaseInputConnection.removeComposingSpans(content);
                content.clear();
            } finally {
                resetting = false;
            }
            sent = "";
            InputMethodManager manager =
                (InputMethodManager) getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
            if (manager != null) {
                manager.restartInput(this);
            }
        }

        private void postReset() {
            post(new Runnable() {
                @Override
                public void run() {
                    // A reset mid-composition would orphan the word the IME is still building.
                    Editable content = getText();
                    if (content != null && BaseInputConnection.getComposingSpanStart(content) >= 0) {
                        return;
                    }
                    resetMirror();
                }
            });
        }

        private void scheduleSync() {
            if (resetting || syncPosted) {
                return;
            }
            syncPosted = true;
            post(new Runnable() {
                @Override
                public void run() {
                    syncPosted = false;
                    if (batchDepth == 0) {
                        sync();
                    }
                }
            });
        }

        void sync() {
            sync(false);
        }

        /**
         * Send the guest whatever turns the previously sent text into the editor's text, with
         * the composing word as preedit unless {@code commitComposing} (Enter) or the focused
         * guest client has no input-method channel to show one.
         */
        void sync(boolean commitComposing) {
            if (resetting) {
                return;
            }
            Editable content = getText();
            String now = content == null ? "" : content.toString();
            String committed = now;
            String preedit = "";
            if (!commitComposing && content != null && nativeIsPreeditSupported()) {
                int start = BaseInputConnection.getComposingSpanStart(content);
                int end = BaseInputConnection.getComposingSpanEnd(content);
                // Only a word composed at the end and not yet sent is held back: recomposing a
                // word the guest already has keeps it as text, it is never taken away again.
                if (start >= 0 && start < end && end == now.length()
                    && start >= commonPrefix(now, sent)) {
                    committed = now.substring(0, start);
                    preedit = now.substring(start);
                }
            }

            if (!committed.equals(sent)) {
                int prefix = commonPrefix(committed, sent);
                int removed = sent.codePointCount(prefix, sent.length());
                String added = committed.substring(prefix).replace("\r\n", "\n").replace('\r', '\n');
                sent = committed;

                sendBackspaces(removed);
                int from = 0;
                int newline;
                while ((newline = added.indexOf('\n', from)) >= 0) {
                    sendText(added.substring(from, newline));
                    nativeOnTextCommit("\n");
                    from = newline + 1;
                }
                sendText(added.substring(from));
                // A commit replaces the guest preedit in the same input-method frame.
                if (!added.isEmpty()) {
                    sentPreedit = "";
                }
                // After Enter the guest caret may be in another context (submitted field, new
                // prompt); text before it must not be editable through the old mirror.
                if (added.indexOf('\n') >= 0 || committed.length() > MAX_MIRROR_CHARS) {
                    postReset();
                }
            }
            if (!preedit.equals(sentPreedit)) {
                nativeOnPreedit(preedit);
                sentPreedit = preedit;
            }
        }

        private static int commonPrefix(String a, String b) {
            int limit = Math.min(a.length(), b.length());
            int prefix = 0;
            while (prefix < limit && a.charAt(prefix) == b.charAt(prefix)) {
                prefix++;
            }
            // Never split a surrogate pair.
            if (prefix > 0 && Character.isHighSurrogate(a.charAt(prefix - 1))) {
                prefix--;
            }
            return prefix;
        }

        /** Delete one code point (or the selection) before the editor caret, as Backspace. */
        void deleteBackward() {
            Editable content = getText();
            int start = Selection.getSelectionStart(content);
            int end = Selection.getSelectionEnd(content);
            if (content == null || start < 0 || end < 0) {
                sync();
                sendBackspaces(1);
                return;
            }
            if (start != end) {
                content.delete(Math.min(start, end), Math.max(start, end));
            } else if (start > 0) {
                int length = Character.charCount(Character.codePointBefore(content, start));
                content.delete(start - length, start);
            } else {
                // Nothing mirrored before the caret, but the guest may still have text there.
                sync();
                sendBackspaces(1);
                return;
            }
            sync();
        }

        private void sendBackspaces(int count) {
            if (count > 0 && !sentPreedit.isEmpty()) {
                // A Backspace must reach committed text, not the preedit shown before it.
                nativeOnPreedit("");
                sentPreedit = "";
            }
            // The IBus engine caps one DELETE command at 64, so long runs go out in pieces.
            while (count > 0) {
                int run = Math.min(count, 64);
                char[] chars = new char[run];
                Arrays.fill(chars, '\b');
                nativeOnTextCommit(new String(chars));
                count -= run;
            }
        }

        private static void sendText(String text) {
            int from = 0;
            while (from < text.length()) {
                int to = Math.min(text.length(), from + MAX_COMMIT_CHARS);
                if (to < text.length() && Character.isHighSurrogate(text.charAt(to - 1))) {
                    to--;
                }
                nativeOnTextCommit(text.substring(from, to));
                from = to;
            }
        }

        private int charsBeforeCaret() {
            return Math.max(Selection.getSelectionStart(getText()), 0);
        }

        private int codePointsBeforeCaret() {
            Editable content = getText();
            int start = charsBeforeCaret();
            return content == null || start == 0 ? 0 : Character.codePointCount(content, 0, start);
        }

        @Override
        public InputConnection onCreateInputConnection(EditorInfo outAttrs) {
            // TextView's own connection keeps batch edits, selection updates and extracted
            // text consistent for the IME; the wrapper only forwards the resulting edits.
            InputConnection target = super.onCreateInputConnection(outAttrs);
            outAttrs.inputType = INPUT_TYPE;
            outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI;
            if (target == null) {
                return null;
            }
            batchDepth = 0;
            return new InputConnectionWrapper(target, true) {
                private boolean edited(boolean result) {
                    if (batchDepth == 0) {
                        sync();
                    }
                    return result;
                }

                @Override
                public boolean beginBatchEdit() {
                    batchDepth++;
                    return super.beginBatchEdit();
                }

                @Override
                public boolean endBatchEdit() {
                    boolean result = super.endBatchEdit();
                    if (batchDepth > 0) {
                        batchDepth--;
                    }
                    return edited(result);
                }

                @Override
                public boolean commitText(CharSequence text, int newCursorPosition) {
                    return edited(super.commitText(text, newCursorPosition));
                }

                @Override
                public boolean setComposingText(CharSequence text, int newCursorPosition) {
                    return edited(super.setComposingText(text, newCursorPosition));
                }

                @Override
                public boolean setComposingRegion(int start, int end) {
                    return edited(super.setComposingRegion(start, end));
                }

                @Override
                public boolean finishComposingText() {
                    return edited(super.finishComposingText());
                }

                @Override
                public boolean commitCompletion(CompletionInfo text) {
                    return edited(super.commitCompletion(text));
                }

                @Override
                public boolean commitCorrection(CorrectionInfo correctionInfo) {
                    return edited(super.commitCorrection(correctionInfo));
                }

                @Override
                public boolean performContextMenuAction(int id) {
                    return edited(super.performContextMenuAction(id));
                }

                @Override
                public boolean deleteSurroundingText(int beforeLength, int afterLength) {
                    // Deletes reaching past the mirrored text still remove guest text (e.g.
                    // Backspace in an empty editor), so the excess goes out as Backspace.
                    int excess = Math.max(0, beforeLength - charsBeforeCaret());
                    boolean result = edited(super.deleteSurroundingText(beforeLength, afterLength));
                    sendBackspaces(excess);
                    return result;
                }

                @Override
                public boolean deleteSurroundingTextInCodePoints(int beforeLength, int afterLength) {
                    int excess = Math.max(0, beforeLength - codePointsBeforeCaret());
                    boolean result =
                        edited(super.deleteSurroundingTextInCodePoints(beforeLength, afterLength));
                    sendBackspaces(excess);
                    return result;
                }

                @Override
                public boolean sendKeyEvent(KeyEvent event) {
                    // Some IMEs send Enter/Backspace through sendKeyEvent instead of text edits.
                    if (event != null && event.getAction() == KeyEvent.ACTION_DOWN) {
                        switch (event.getKeyCode()) {
                            case KeyEvent.KEYCODE_ENTER:
                                sync(true);
                                nativeOnTextCommit("\n");
                                postReset();
                                return true;
                            case KeyEvent.KEYCODE_DEL:
                                deleteBackward();
                                return true;
                            default:
                                break;
                        }
                    }
                    return super.sendKeyEvent(event);
                }

                @Override
                public boolean performEditorAction(int actionCode) {
                    sync(true);
                    nativeOnTextCommit("\n");
                    postReset();
                    return true;
                }

                @Override
                public void closeConnection() {
                    super.closeConnection();
                    batchDepth = 0;
                    sync();
                }
            };
        }
    }

    private static native void nativeOnTextCommit(String text);
    private static native void nativeOnPreedit(String text);
    private static native void nativeOnPreeditFlush();
    private static native boolean nativeIsPreeditSupported();
    private static native void nativeOnInputDevicesChanged(boolean hasPhysicalKeyboard, boolean hasDesktopInput);
    private static native void nativeOnHardwareKeyboardChanged(boolean present);
}
