package app.polarbear.files.tests;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Intent;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.os.SystemClock;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import org.json.JSONObject;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.lang.reflect.Method;
import java.security.MessageDigest;
import java.util.Arrays;
import java.util.UUID;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

/** Device tests run only against the independently signed Portal Debug app. */
public final class PortalFilesInstrumentation extends Instrumentation {
    private Class<?> transfers;
    private Class<?> overlay;
    private Method run;
    private Activity activity;
    private String prefix;
    private long activeDragDown;
    private boolean dragInProgress;
    private boolean motionOnly;
    private boolean uiOnly;

    @Override public void onCreate(Bundle arguments) {
        super.onCreate(arguments);
        motionOnly = arguments != null && Boolean.parseBoolean(arguments.getString("motionOnly", "false"));
        uiOnly = arguments != null && Boolean.parseBoolean(arguments.getString("uiOnly", "false"));
        start();
    }

    private void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }
    private Object invoke(Class<?> type, String name) throws Exception {
        return type.getMethod(name).invoke(type.getField("INSTANCE").get(null));
    }
    private void onMain(Runnable action) { runOnMainSync(action); }
    private void status(String text) {
        if (prefix != null) try (FileOutputStream file = new FileOutputStream(new File(getTargetContext().getCacheDir(),prefix+"-checks.log"),true)) {
            file.write((text+"\n").getBytes(java.nio.charset.StandardCharsets.UTF_8));
        } catch (java.io.IOException error) { throw new RuntimeException("Could not record device check",error); }
        Bundle result = new Bundle(); result.putString("stream", text + "\n"); sendStatus(0, result);
    }
    @Override public void finish(int code, Bundle result) {
        if (prefix != null) try (FileOutputStream file = new FileOutputStream(new File(getTargetContext().getCacheDir(),prefix+"-result.txt"))) {
            file.write(("code="+code+"\n"+result.getString("stream","")).getBytes(java.nio.charset.StandardCharsets.UTF_8));
        } catch (java.io.IOException error) { result.putString("stream",result.getString("stream","")+"\nCould not persist result: "+error); }
        super.finish(code,result);
    }
    private boolean showing() throws Exception { return (Boolean) overlay.getMethod("isShowing").invoke(null); }
    private void waitShowing(boolean expected) throws Exception {
        long deadline = SystemClock.uptimeMillis() + 10000;
        while (showing() != expected && SystemClock.uptimeMillis() < deadline) SystemClock.sleep(50);
        check(showing() == expected, "Veil visibility did not settle to " + expected);
    }
    private JSONObject operation(JSONObject request, int input, int output, String job) throws Exception {
        JSONObject response = new JSONObject((String) run.invoke(null, request.toString(), input, output, job));
        check(response.optBoolean("ok"), response.toString());
        return response;
    }
    private JSONObject operation(JSONObject request) throws Exception {
        return operation(request, -1, -1, UUID.randomUUID().toString());
    }
    private byte[] digest(File file) throws Exception {
        MessageDigest digest = MessageDigest.getInstance("SHA-256");
        byte[] buffer = new byte[256 * 1024];
        try (FileInputStream input = new FileInputStream(file)) {
            int count; while ((count = input.read(buffer)) != -1) digest.update(buffer, 0, count);
        }
        return digest.digest();
    }
    private String importFile(File file, String directory, String name) throws Exception {
        String job = UUID.randomUUID().toString();
        try (ParcelFileDescriptor fd = ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)) {
            return operation(new JSONObject().put("action", "import").put("directory", directory)
                .put("name", name).put("size", file.length()).put("job", job), fd.getFd(), -1, job).getString("name");
        }
    }
    private String find(String directory, String name) throws Exception {
        JSONObject listing = operation(new JSONObject().put("action", "list").put("directory", directory).put("hidden", true));
        int offset = 0;
        while (true) {
            org.json.JSONArray entries = listing.getJSONArray("entries");
            for (int i = 0; i < entries.length(); i++) {
                JSONObject entry = entries.getJSONObject(i);
                if (entry.getString("name").equals(name)) return entry.getString("id");
            }
            if (!listing.getBoolean("more")) throw new AssertionError("Imported file missing: " + name);
            offset += entries.length();
            listing = operation(new JSONObject().put("action", "list").put("directory", directory).put("hidden", true).put("offset", offset));
        }
    }
    private void roundTrip(File source, String directory, String name) throws Exception {
        String imported = importFile(source, directory, name);
        File exported = new File(getTargetContext().getCacheDir(), prefix + "-export");
        try {
            try (ParcelFileDescriptor output = ParcelFileDescriptor.open(exported,
                    ParcelFileDescriptor.MODE_CREATE | ParcelFileDescriptor.MODE_TRUNCATE | ParcelFileDescriptor.MODE_WRITE_ONLY)) {
                operation(new JSONObject().put("action", "export").put("entry", find(directory, imported)), -1, output.getFd(), UUID.randomUUID().toString());
            }
            check(source.length() == exported.length(), "Round-trip byte count changed");
            check(Arrays.equals(digest(source), digest(exported)), "Round-trip SHA-256 changed");
        } finally { exported.delete(); }
    }
    private View findCapsule(View view) {
        if (view.getClass().getSimpleName().equals("PortalCapsuleView") && view.isShown()) return view;
        if (view instanceof android.view.ViewGroup) {
            android.view.ViewGroup group = (android.view.ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                View found = findCapsule(group.getChildAt(i));
                if (found != null) return found;
            }
        }
        return null;
    }
    private android.graphics.Rect capsuleBounds() throws Exception {
        long deadline = SystemClock.uptimeMillis() + 10000;
        while (SystemClock.uptimeMillis() < deadline) {
            final android.graphics.Rect bounds = new android.graphics.Rect();
            onMain(() -> {
                View capsule = findCapsule(activity.getWindow().getDecorView());
                if (capsule != null) { try {
                    java.lang.reflect.Field hit = capsule.getClass().getDeclaredField("hubBounds"); hit.setAccessible(true);
                    android.graphics.RectF rect = (android.graphics.RectF) hit.get(capsule);
                    rect.round(bounds);
                    int[] origin = new int[2]; capsule.getLocationOnScreen(origin); bounds.offset(origin[0],origin[1]);
                } catch (Exception e) { throw new RuntimeException(e); } }
            });
            if (!bounds.isEmpty()) return bounds;
            SystemClock.sleep(100);
        }
        Class<?> controls = getTargetContext().getClassLoader().loadClass("app.polarbear.TouchControls");
        StringBuilder state = new StringBuilder("Capsule absent; overlay=" + showing());
        for (String name : new String[] { "handle", "activity", "root" }) {
            java.lang.reflect.Field field = controls.getDeclaredField(name); field.setAccessible(true);
            state.append(" ").append(name).append("=").append(field.get(controls.getField("INSTANCE").get(null)));
        }
        throw new AssertionError(state.toString());
    }
    private View capsuleView() { return findCapsule(activity.getWindow().getDecorView()); }
    private void collectSemantics(View view, String needle, java.util.List<android.graphics.Rect> found) throws Exception {
        if (view.getClass().getSimpleName().equals("AndroidComposeView")) {
            Object owner = view.getClass().getMethod("getSemanticsOwner").invoke(view);
            Object node = owner.getClass().getMethod("getRootSemanticsNode").invoke(owner);
            int[] origin = new int[2]; view.getLocationOnScreen(origin);
            collectSemanticsNode(node, needle.replace(" ", ""), origin, found);
        } else if (view instanceof android.view.ViewGroup) {
            android.view.ViewGroup group = (android.view.ViewGroup) view;
            for (int i=0; i<group.getChildCount(); i++) collectSemantics(group.getChildAt(i), needle, found);
        }
    }
    private void collectSemanticsNode(Object node, String needle, int[] origin, java.util.List<android.graphics.Rect> found) throws Exception {
        String config = node.getClass().getMethod("getConfig").invoke(node).toString().replace(" ", "");
        if (config.contains(needle)) {
            Object rect = node.getClass().getMethod("getBoundsInRoot").invoke(node);
            Class<?> type = rect.getClass();
            android.graphics.Rect bounds = new android.graphics.Rect(
                Math.round((Float) type.getMethod("getLeft").invoke(rect))+origin[0],
                Math.round((Float) type.getMethod("getTop").invoke(rect))+origin[1],
                Math.round((Float) type.getMethod("getRight").invoke(rect))+origin[0],
                Math.round((Float) type.getMethod("getBottom").invoke(rect))+origin[1]);
            if (!bounds.isEmpty()) found.add(bounds);
        }
        for (Object child : (java.util.List<?>) node.getClass().getMethod("getChildren").invoke(node))
            collectSemanticsNode(child, needle, origin, found);
    }
    private android.graphics.Rect semanticBounds(String... needles) throws Exception {
        long deadline = SystemClock.uptimeMillis()+5000;
        while (SystemClock.uptimeMillis()<deadline) {
            java.util.List<android.graphics.Rect> matches = new java.util.ArrayList<>();
            onMain(() -> { try { for (String needle : needles) collectSemantics(activity.getWindow().getDecorView(), needle, matches); }
                catch (Exception e) { throw new RuntimeException(e); } });
            if (!matches.isEmpty()) return matches.get(matches.size()-1);
            SystemClock.sleep(50);
        }
        throw new AssertionError("Missing visible semantics: " + java.util.Arrays.toString(needles));
    }
    private void setState(Class<?> owner, String getter, boolean value) throws Exception {
        Object state = invoke(owner, getter);
        Class<?> mutable = getTargetContext().getClassLoader().loadClass("androidx.compose.runtime.MutableState");
        mutable.getMethod("setValue", Object.class).invoke(state, value);
    }
    private boolean boolState(Class<?> owner, String getter) throws Exception {
        Object state = invoke(owner, getter);
        Class<?> immutable = getTargetContext().getClassLoader().loadClass("androidx.compose.runtime.State");
        return (Boolean) immutable.getMethod("getValue").invoke(state);
    }
    private void testContextIcons() throws Exception {
        Class<?> controls = getTargetContext().getClassLoader().loadClass("app.polarbear.TouchControls");
        Class<?> session = getTargetContext().getClassLoader().loadClass("app.polarbear.SessionControls");
        java.lang.reflect.Field handlerField = controls.getDeclaredField("main"), pollField = controls.getDeclaredField("poll");
        handlerField.setAccessible(true); pollField.setAccessible(true);
        android.os.Handler handler = (android.os.Handler) handlerField.get(controls.getField("INSTANCE").get(null));
        Runnable poll = (Runnable) pollField.get(controls.getField("INSTANCE").get(null));
        handler.removeCallbacks(poll);
        boolean focus = boolState(session,"getTextInputFocused"), steam = boolState(controls,"getSteamRunning"), game = boolState(controls,"getGameRunning");
        try {
            onMain(() -> { try { setState(session,"getTextInputFocused",false); setState(controls,"getSteamRunning",true); }
                catch (Exception e) { throw new RuntimeException(e); } });
            semanticBounds("Game options suggested");
            onMain(() -> { try { setState(session,"getTextInputFocused",true); }
                catch (Exception e) { throw new RuntimeException(e); } });
            semanticBounds("Keyboard suggested");
            onMain(() -> { try { setState(session,"getTextInputFocused",false); setState(controls,"getSteamRunning",false); setState(controls,"getGameRunning",false); }
                catch (Exception e) { throw new RuntimeException(e); } });
            semanticBounds("Portal veil suggested","Files suggested");
        } finally {
            onMain(() -> { try { setState(session,"getTextInputFocused",focus); setState(controls,"getSteamRunning",steam); setState(controls,"getGameRunning",game); }
                catch (Exception e) { throw new RuntimeException(e); } });
            handler.post(poll);
        }
        status("Context icons and keyboard priority passed without changing menu behaviour");
    }
    private void testUtilityInputOwnership() throws Exception {
        Class<?> session = getTargetContext().getClassLoader().loadClass("app.polarbear.SessionControls");
        Class<?> page = getTargetContext().getClassLoader().loadClass("app.polarbear.UtilityPage");
        Object dock = java.util.Arrays.stream(page.getEnumConstants()).filter(v -> v.toString().equals("Dock")).findFirst().get();
        openMenu();
        onMain(() -> { try {
            View view = capsuleView();
            java.lang.reflect.Field id = view.getClass().getDeclaredField("presentationId$delegate"); id.setAccessible(true);
            Class<?> integerState = getTargetContext().getClassLoader().loadClass("androidx.compose.runtime.IntState");
            int current = (Integer) integerState.getMethod("getIntValue").invoke(id.get(view));
            Method settled = view.getClass().getDeclaredMethod("onPresentationSettled",page,int.class); settled.setAccessible(true);
            // Deliver both a prior completion and a completion for the wrong
            // page. Neither may release a visible menu's pointer ownership.
            settled.invoke(view,dock,current-1);
            settled.invoke(view,dock,current);
            check((Boolean) session.getMethod("isUtilityVisible").invoke(null),"Stale close completion released menu input");
            java.lang.reflect.Field token = view.getClass().getDeclaredField("inputOwner"); token.setAccessible(true);
            Object owner = token.get(view), previous = new Object(), instance = session.getField("INSTANCE").get(null);
            check((Boolean) session.getMethod("acquireUtility",Object.class).invoke(instance,previous),"Could not acquire test menu owner");
            check((Boolean) session.getMethod("acquireUtility",Object.class).invoke(instance,owner),"Could not restore current menu owner");
            check(!(Boolean) session.getMethod("releaseUtility",Object.class).invoke(instance,previous),"A retired owner released current menu input");
            check((Boolean) session.getMethod("isUtilityVisible").invoke(null),"Owner replacement lost menu input");
        } catch (Exception e) { throw new RuntimeException(e); } });
        for (int cycle=0;cycle<8;cycle++) {
            onMain(() -> { try {
                View view=capsuleView();
                view.getClass().getMethod("dismiss").invoke(view);
                view.getClass().getMethod("reveal").invoke(view);
            } catch (Exception e) { throw new RuntimeException(e); } });
            SystemClock.sleep(cycle%2==0 ? 16 : 80);
            check((Boolean) session.getMethod("isUtilityVisible").invoke(null),"Rapid reopen lost menu input");
        }
        waitUtility("Menu");
        tap(utilityBounds("keyboard")); waitUtility("Keyboard");
        check((Boolean) session.getMethod("isUtilityVisible").invoke(null),"Keyboard lost menu input");
        escapeUtility(); waitUtility("Menu"); escapeUtility(); waitUtility("Dock");
        check(!(Boolean) session.getMethod("isUtilityVisible").invoke(null),"Docked menu kept desktop input blocked");
        openMenu();
        onMain(() -> {
            View view = capsuleView();
            android.view.ViewGroup parent = (android.view.ViewGroup) view.getParent();
            int index = parent.indexOfChild(view); android.view.ViewGroup.LayoutParams params = view.getLayoutParams();
            parent.removeView(view); parent.addView(view,index,params);
        });
        waitUtility("Dock");
        check(!(Boolean) session.getMethod("isUtilityVisible").invoke(null),"Reattached control retained a hidden menu's input");
        openMenu(); tap(utilityBounds("keyboard")); waitUtility("Keyboard");
        escapeUtility(); waitUtility("Menu"); escapeUtility(); waitUtility("Dock");
        status("Stale completion, retired-owner release, rapid reopen and actual keyboard taps preserved input ownership");
        status("Detached and reattached controls reset presentation and remained responsive");
    }
    private void testOutsideDismissal() throws Exception {
        for (String panel : new String[]{"files","apps"}) {
            onMain(() -> { try { overlay.getMethod("setActivePanel",String.class).invoke(overlay.getField("INSTANCE").get(null),panel); }
                catch (Exception e) { throw new RuntimeException(e); } });
            SystemClock.sleep(900);
            String label = panel.equals("files") ? "Files" : "Apps";
            android.graphics.Rect before = semanticBounds(label);
            SystemClock.sleep(2400);
            android.graphics.Rect after = semanticBounds(label);
            check(Math.abs(before.top-after.top)<=1, "The open panel moved with the veil's idle hint");
            tap(after);
            check(panel.equals(invoke(overlay,"getActivePanel")), "An inside tap collapsed the card");
            long insideDown = SystemClock.uptimeMillis();
            singleTouch(insideDown,MotionEvent.ACTION_DOWN,after.centerX(),after.centerY());
            float density = activity.getResources().getDisplayMetrics().density;
            for (int step=1;step<=5;step++) {
                singleTouch(insideDown,MotionEvent.ACTION_MOVE,after.centerX(),after.centerY()-step*5*density);
                SystemClock.sleep(30);
            }
            singleTouch(insideDown,MotionEvent.ACTION_UP,after.centerX(),after.centerY()-25*density);
            check(panel.equals(invoke(overlay,"getActivePanel")) && showing(), "Scrolling inside the card changed the veil");
            final android.graphics.Rect outside = new android.graphics.Rect();
            onMain(() -> {
                View root = activity.getWindow().getDecorView(); int[] origin = new int[2]; root.getLocationOnScreen(origin);
                int x = origin[0]+Math.round(8*activity.getResources().getDisplayMetrics().density), y = origin[1]+root.getHeight()/2;
                outside.set(x,y,x+1,y+1);
            });
            long canceledDown = SystemClock.uptimeMillis();
            singleTouch(canceledDown,MotionEvent.ACTION_DOWN,outside.centerX(),outside.centerY());
            singleTouch(canceledDown,MotionEvent.ACTION_CANCEL,outside.centerX(),outside.centerY());
            check(panel.equals(invoke(overlay,"getActivePanel")), "A canceled outside touch collapsed the card");
            tap(outside); SystemClock.sleep(600);
            check(invoke(overlay,"getActivePanel")==null, "Outside tap did not collapse " + panel);
            check(showing(), "Outside tap dismissed the whole veil");
        }
        status("Inside taps, outside dismissal and paused veil breathing passed for Apps and Files");
    }
    private void testGuestTextSuggestion() throws Exception {
        Class<?> session = getTargetContext().getClassLoader().loadClass("app.polarbear.SessionControls");
        Method focused = session.getDeclaredMethod("nativeTextInputActive"); focused.setAccessible(true);
        if ((Boolean) focused.invoke(null)) {
            semanticBounds("Keyboard suggested");
            status("Existing guest text focus produces the keyboard suggestion");
            return;
        }
        long now = SystemClock.uptimeMillis();
        sendKeySync(new KeyEvent(now,now,KeyEvent.ACTION_DOWN,KeyEvent.KEYCODE_ALT_LEFT,0));
        sendKeySync(new KeyEvent(now,now+1,KeyEvent.ACTION_DOWN,KeyEvent.KEYCODE_SPACE,0,KeyEvent.META_ALT_ON));
        sendKeySync(new KeyEvent(now,now+2,KeyEvent.ACTION_UP,KeyEvent.KEYCODE_SPACE,0,KeyEvent.META_ALT_ON));
        sendKeySync(new KeyEvent(now,now+3,KeyEvent.ACTION_UP,KeyEvent.KEYCODE_ALT_LEFT,0));
        try {
            long deadline=SystemClock.uptimeMillis()+10000;
            while (!(Boolean) focused.invoke(null) && SystemClock.uptimeMillis()<deadline) SystemClock.sleep(50);
            check((Boolean) focused.invoke(null),"KRunner did not report guest text focus");
            semanticBounds("Keyboard suggested");
            openMenu();
            Class<?> bridge = getTargetContext().getClassLoader().loadClass("app.polarbear.SoftKeyboardBridge");
            Method wants = bridge.getDeclaredMethod("nativeWantsSoftKeyboard"); wants.setAccessible(true);
            check(!(Boolean) wants.invoke(null),"Guest keyboard remained eligible over Portal controls");
            java.lang.reflect.Field editor = bridge.getDeclaredField("editor"); editor.setAccessible(true);
            long hiddenDeadline = SystemClock.uptimeMillis()+5000;
            while (SystemClock.uptimeMillis()<hiddenDeadline) {
                View input = (View) editor.get(null);
                if (input == null || input.getVisibility()!=View.VISIBLE) break;
                SystemClock.sleep(50);
            }
            View input = (View) editor.get(null);
            check(input == null || input.getVisibility()!=View.VISIBLE,"Guest keyboard editor covered the utility menu");
            // Repeat a delayed guest show request, as can happen during an app-focus race.
            bridge.getMethod("show",Activity.class).invoke(null,activity);
            SystemClock.sleep(300);
            input = (View) editor.get(null);
            check(input == null || input.getVisibility()!=View.VISIBLE,"Delayed guest focus reopened the keyboard over controls");
            check((Boolean) focused.invoke(null),"Opening controls discarded the real guest text focus");
            escapeUtility(); waitUtility("Dock");
        } finally {
            for (int attempt=0;attempt<3;attempt++) {
                sendKeyDownUpSync(KeyEvent.KEYCODE_ESCAPE); SystemClock.sleep(250);
                if (!(Boolean) focused.invoke(null)) break;
            }
        }
        check(!(Boolean) focused.invoke(null),"Guest text focus did not clear after closing KRunner");
        status("Real guest text-field focus and keyboard suggestion passed");
        status("Guest keyboard and delayed focus requests stayed behind Portal controls");
    }
    private void waitUtility(String expected) throws Exception {
        long deadline = SystemClock.uptimeMillis()+10000;
        java.util.concurrent.atomic.AtomicBoolean ready = new java.util.concurrent.atomic.AtomicBoolean();
        while (!ready.get() && SystemClock.uptimeMillis()<deadline) {
            onMain(() -> { try {
                View view = capsuleView();
                if (view != null) {
                    java.lang.reflect.Field state = view.getClass().getDeclaredField("settledPage"); state.setAccessible(true);
                    Object page = state.get(view); ready.set(page != null && page.toString().equals(expected));
                }
            } catch (Exception e) { throw new RuntimeException(e); } });
            if (!ready.get()) SystemClock.sleep(50);
        }
        check(ready.get(), "Utility did not settle to " + expected);
    }
    private android.graphics.Rect utilityBounds(String action) throws Exception {
        final android.graphics.Rect bounds = new android.graphics.Rect();
        onMain(() -> { try {
            View view = capsuleView();
            java.lang.reflect.Field field = view.getClass().getDeclaredField("actionBounds"); field.setAccessible(true);
            android.graphics.RectF rect = (android.graphics.RectF) ((java.util.Map<?,?>) field.get(view)).get(action);
            if (rect != null) { rect.round(bounds); int[] origin = new int[2]; view.getLocationOnScreen(origin); bounds.offset(origin[0],origin[1]); }
        } catch (Exception e) { throw new RuntimeException(e); } });
        check(!bounds.isEmpty(), "No laid-out utility action: " + action); return bounds;
    }
    private void tap(android.graphics.Rect bounds) {
        long down=SystemClock.uptimeMillis();
        singleTouch(down,MotionEvent.ACTION_DOWN,bounds.centerX(),bounds.centerY());
        singleTouch(down,MotionEvent.ACTION_UP,bounds.centerX(),bounds.centerY());
    }
    private void capture(String suffix) throws Exception {
        android.graphics.Bitmap bitmap = getUiAutomation(android.app.UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES).takeScreenshot();
        check(bitmap != null, "Could not capture utility UI");
        try (FileOutputStream file = new FileOutputStream(new File(getTargetContext().getCacheDir(),prefix+suffix+".png"))) {
            bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG,100,file);
        } finally { bitmap.recycle(); }
    }
    private void openMenu() throws Exception { tap(capsuleBounds()); waitUtility("Menu"); }
    private void openCapsule() throws Exception { openMenu(); tap(utilityBounds("veil")); }
    private void testRoundtable() throws Exception {
        openMenu();
        for (String action : new String[] { "veil","files","apps","keyboard" }) {
            android.graphics.Rect bounds = utilityBounds(action);
            check(bounds.left>=0 && bounds.top>=0 && bounds.right<=activity.getWindow().getDecorView().getWidth(), "Bubble left the viewport");
        }
        capture("-roundtable");
        tap(utilityBounds("keyboard")); waitUtility("Keyboard"); capture("-keyboard");
        check(!showing(), "Keyboard preview restarted or covered the guest with the veil");
        long now=SystemClock.uptimeMillis();
        sendKeySync(new KeyEvent(now,now,KeyEvent.ACTION_DOWN,KeyEvent.KEYCODE_ESCAPE,0));
        sendKeySync(new KeyEvent(now,now+1,KeyEvent.ACTION_UP,KeyEvent.KEYCODE_ESCAPE,0));
        waitUtility("Menu");
        now=SystemClock.uptimeMillis();
        sendKeySync(new KeyEvent(now,now,KeyEvent.ACTION_DOWN,KeyEvent.KEYCODE_ESCAPE,0));
        sendKeySync(new KeyEvent(now,now+1,KeyEvent.ACTION_UP,KeyEvent.KEYCODE_ESCAPE,0));
        waitUtility("Dock");
        check(!showing(), "Dismissing the utility opened the veil");
        status("Labelled utility roundtable, keyboard preview, nested Escape and guest preservation passed");
    }
    private String shellText(String command) throws Exception {
        try (ParcelFileDescriptor descriptor = getUiAutomation(android.app.UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES).executeShellCommand(command);
             java.io.InputStream input = new ParcelFileDescriptor.AutoCloseInputStream(descriptor);
             java.io.ByteArrayOutputStream output = new java.io.ByteArrayOutputStream()) {
            byte[] block = new byte[8192]; int count;
            while ((count=input.read(block))!=-1) output.write(block,0,count);
            return output.toString("UTF-8").trim();
        }
    }
    private Object utilityArea() throws Exception {
        java.util.concurrent.atomic.AtomicReference<Object> result = new java.util.concurrent.atomic.AtomicReference<>();
        onMain(() -> { try {
            View view=capsuleView(); java.lang.reflect.Field f=view.getClass().getDeclaredField("area$delegate"); f.setAccessible(true);
            Class<?> state=getTargetContext().getClassLoader().loadClass("androidx.compose.runtime.State");
            result.set(state.getMethod("getValue").invoke(f.get(view)));
        } catch (Exception e) { throw new RuntimeException(e); } });
        return result.get();
    }
    private void setUtilityArea(Object value) {
        onMain(() -> { try {
            View view=capsuleView(); java.lang.reflect.Field f=view.getClass().getDeclaredField("area$delegate"); f.setAccessible(true);
            Class<?> state=getTargetContext().getClassLoader().loadClass("androidx.compose.runtime.MutableState");
            state.getMethod("setValue",Object.class).invoke(f.get(view),value);
        } catch (Exception e) { throw new RuntimeException(e); } });
    }
    private Object compactArea(float heightDp) throws Exception {
        float d=activity.getResources().getDisplayMetrics().density;
        Class<?> area=getTargetContext().getClassLoader().loadClass("app.polarbear.UtilityMenuGeometry$Area");
        return area.getConstructor(float.class,float.class,float.class,float.class).newInstance(40*d,40*d,360*d,(40+heightDp)*d);
    }
    private void escapeUtility() {
        long now=SystemClock.uptimeMillis();
        sendKeySync(new KeyEvent(now,now,KeyEvent.ACTION_DOWN,KeyEvent.KEYCODE_ESCAPE,0));
        sendKeySync(new KeyEvent(now,now+1,KeyEvent.ACTION_UP,KeyEvent.KEYCODE_ESCAPE,0));
    }
    private void testResponsiveUtility() throws Exception {
        String originalFont=shellText("settings get system font_scale");
        String originalNight=shellText("cmd uimode night");
        String night=originalNight.substring(originalNight.indexOf(':')+1).trim().split("\\s+")[0];
        check(java.util.Arrays.asList("yes","no","auto","custom").contains(night),"Unrecognized night-mode setting");
        check(originalFont.equals("null") || originalFont.matches("[0-9]+(\\.[0-9]+)?"),"Unrecognized text size");
        openMenu(); Object originalArea=utilityArea();
        try {
            setUtilityArea(compactArea(360)); SystemClock.sleep(1400); capture("-compact");
            for (String action:new String[]{"veil","files","apps","keyboard"}) check(!utilityBounds(action).isEmpty(),"Compact action missing");
            shellText("settings put system font_scale 1.6"); SystemClock.sleep(1300);
            setUtilityArea(compactArea(700)); SystemClock.sleep(1400); capture("-large-text");
            shellText("cmd uimode night no"); SystemClock.sleep(1300);
            setUtilityArea(compactArea(700)); SystemClock.sleep(1400); capture("-light-large-text");
            check(!showing(),"Responsive utility checks restarted or opened the veil");
        } finally {
            shellText(originalFont.equals("null") ? "settings delete system font_scale" : "settings put system font_scale "+originalFont);
            shellText("cmd uimode night "+night);
            SystemClock.sleep(1300); setUtilityArea(originalArea); SystemClock.sleep(1300);
        }
        escapeUtility(); waitUtility("Dock");
        status("Compact controls, large text and light theme passed; original settings restored");
    }
    private void profileUtilityMotion() throws Exception {
        for (int pass=0;pass<(motionOnly ? 12 : 4);pass++) {
            if (pass==1) shellText("dumpsys gfxinfo app.polarbear reset");
            openMenu(); tap(utilityBounds("keyboard")); waitUtility("Keyboard");
            escapeUtility(); waitUtility("Menu"); escapeUtility(); waitUtility("Dock");
        }
        try (FileOutputStream output=new FileOutputStream(new File(getTargetContext().getCacheDir(),prefix+"-utility-gfxinfo.txt"))) {
            output.write(shellText("dumpsys gfxinfo app.polarbear").getBytes(java.nio.charset.StandardCharsets.UTF_8));
        }
        status("Recorded warm roundtable and keyboard motion frame data");
    }
    private Object waitFolderReady() throws Exception {
        long deadline = SystemClock.uptimeMillis() + 15000;
        while (SystemClock.uptimeMillis() < deadline) {
            Object folder = invoke(transfers, "getFolder");
            if (!(Boolean) folder.getClass().getMethod("getLoading").invoke(folder) &&
                    !((String) folder.getClass().getMethod("getId").invoke(folder)).isEmpty()) {
                check(folder.getClass().getMethod("getError").invoke(folder) == null, "File browser failed to load");
                return folder;
            }
            SystemClock.sleep(50);
        }
        throw new AssertionError("File browser did not settle");
    }
    private void testFileBrowserMotion() throws Exception {
        onMain(() -> { try {
            overlay.getMethod("setActivePanel", String.class).invoke(overlay.getField("INSTANCE").get(null), "files");
            invoke(transfers, "beginSave");
        } catch (Exception e) { throw new RuntimeException(e); } });
        try {
            waitFolderReady(); SystemClock.sleep(700); capture("-chooser-large");
            onMain(() -> { try { invoke(transfers, "home"); } catch (Exception e) { throw new RuntimeException(e); } });
            Object home = waitFolderReady();
            check(home.getClass().getMethod("getName").invoke(home).equals("."), "Home navigation left the home directory");
            SystemClock.sleep(700); capture("-chooser-home-large");
            String downloads = (String) invoke(transfers, "getImportDirectory");
            onMain(() -> { try {
                transfers.getMethod("browse", String.class, boolean.class, boolean.class)
                    .invoke(transfers.getField("INSTANCE").get(null), downloads, false, false);
            } catch (Exception e) { throw new RuntimeException(e); } });
            waitFolderReady();
        } finally {
            onMain(() -> { try { invoke(transfers, "closeSave"); } catch (Exception e) { throw new RuntimeException(e); } });
        }
        status("Large-text file browser and Home/Downloads navigation passed");
    }
    private void testCapsuleDrag() throws Exception {
        android.content.SharedPreferences preferences = getTargetContext().getSharedPreferences("portal_touch_controls", 0);
        boolean hadX = preferences.contains("handle_x"), hadY = preferences.contains("handle_y");
        float savedX = preferences.getFloat("handle_x", 0.5f), savedY = preferences.getFloat("handle_y", -1f);
        try {
        android.graphics.Rect original = capsuleBounds();
        float density = activity.getResources().getDisplayMetrics().density;
        long down = SystemClock.uptimeMillis();
        singleTouch(down, MotionEvent.ACTION_DOWN, original.centerX(), original.centerY());
        for (int i = 1; i <= 12; i++) {
            singleTouch(down, MotionEvent.ACTION_MOVE, original.centerX() + i * 5 * density, original.centerY() + i * 5 * density);
            SystemClock.sleep(16);
        }
        singleTouch(down, MotionEvent.ACTION_UP, original.centerX() + 60 * density, original.centerY() + 60 * density);
        SystemClock.sleep(1200);
        check(!showing(), "Dragging accidentally opened the veil");
        android.graphics.Rect moved = capsuleBounds();
        check(moved.centerY() > original.centerY() + 30 * density, "Capsule did not coast with the drag");
        openCapsule(); waitShowing(true);
        onMain(() -> { try { invoke(overlay, "requestReturnToDesktop"); } catch (Exception e) { throw new RuntimeException(e); } });
        waitShowing(false);
        android.graphics.Rect restored = capsuleBounds();
        check(Math.abs(restored.centerY() - moved.centerY()) < 3 * density, "Reopening lost capsule position");
        status("Capsule dragging, click cancellation and position restoration passed");
        } finally {
            android.content.SharedPreferences.Editor editor = preferences.edit();
            if (hadX) editor.putFloat("handle_x", savedX); else editor.remove("handle_x");
            if (hadY) editor.putFloat("handle_y", savedY); else editor.remove("handle_y");
            editor.commit();
            onMain(() -> { try {
                Class<?> controls = getTargetContext().getClassLoader().loadClass("app.polarbear.TouchControls");
                Method place = controls.getDeclaredMethod("placeHandle"); place.setAccessible(true);
                place.invoke(controls.getField("INSTANCE").get(null));
            } catch (Exception e) { throw new RuntimeException(e); } });
            SystemClock.sleep(800);
        }
    }
    private android.view.accessibility.AccessibilityNodeInfo findDescription(android.view.accessibility.AccessibilityNodeInfo node, String text) {
        if (node.getContentDescription() != null && text.equals(node.getContentDescription().toString())) return node;
        for (int i = 0; i < node.getChildCount(); i++) {
            android.view.accessibility.AccessibilityNodeInfo child = node.getChild(i);
            if (child != null) {
                android.view.accessibility.AccessibilityNodeInfo found = findDescription(child, text);
                if (found != null) return found;
            }
        }
        return null;
    }

    private Object awaitTransfer(String source, String phase) throws Exception {
        long deadline = SystemClock.uptimeMillis() + 20000;
        while (SystemClock.uptimeMillis() < deadline) {
            for (Object item : (java.util.List<?>) invoke(transfers, "getItems")) {
                if (!item.getClass().getMethod("getSource").invoke(item).equals(source)) continue;
                String actual = item.getClass().getMethod("getPhase").invoke(item).toString();
                if (actual.equals(phase)) return item;
                check(!actual.equals("Failed"), "Coordinator failed: " + item.getClass().getMethod("getError").invoke(item));
            }
            SystemClock.sleep(100);
        }
        throw new AssertionError("Coordinator did not finish " + source);
    }

    private android.content.IntentFilter documentFilter(String action) throws Exception {
        android.content.IntentFilter filter = new android.content.IntentFilter(action);
        filter.addCategory(Intent.CATEGORY_OPENABLE);
        filter.addDataType("*/*");
        return filter;
    }

    private void testPickerTransfers(String downloads) throws Exception {
        android.net.Uri uri = android.net.Uri.parse("content://app.polarbear.files.tests.fixture/" + prefix + "-uri.bin");
        onMain(() -> { try { invoke(transfers, "open"); } catch (Exception e) { throw new RuntimeException(e); } });
        long openingDeadline = SystemClock.uptimeMillis() + 15000;
        while (invoke(transfers, "getImportDirectory").equals("") && SystemClock.uptimeMillis() < openingDeadline) SystemClock.sleep(100);
        ActivityMonitor picker = addMonitor(documentFilter(Intent.ACTION_OPEN_DOCUMENT),
            new ActivityResult(Activity.RESULT_OK, new Intent().setData(uri)), true);
        try {
            onMain(() -> { try { invoke(transfers, "chooseFiles"); } catch (Exception e) { throw new RuntimeException(e); } });
            check(picker.getHits() == 1, "Choose files did not launch Android's document contract");
        } finally { removeMonitor(picker); }
        Object imported = awaitTransfer(uri.toString(), "Done");
        String name = (String) imported.getClass().getMethod("getName").invoke(imported);
        String identifier = find(downloads, name);
        File actual = new File(getTargetContext().getCacheDir(), prefix + "-uri-export");
        try {
            try (ParcelFileDescriptor output = ParcelFileDescriptor.open(actual, ParcelFileDescriptor.MODE_CREATE | ParcelFileDescriptor.MODE_TRUNCATE | ParcelFileDescriptor.MODE_WRITE_ONLY)) {
                operation(new JSONObject().put("action", "export").put("entry", identifier), -1, output.getFd(), UUID.randomUUID().toString());
            }
            check(actual.length() == FixtureProvider.LENGTH, "Content URI import changed length");
            try (FileInputStream input = new FileInputStream(actual)) {
                byte[] buffer = new byte[256 * 1024]; int count;
                while ((count = input.read(buffer)) != -1) for (int i = 0; i < count; i++) check(buffer[i] == (byte) i, "Content URI bytes changed");
            }
            onMain(() -> { try { invoke(transfers, "open"); } catch (Exception e) { throw new RuntimeException(e); } });
            long deadline = SystemClock.uptimeMillis() + 15000;
            Object chosen = null;
            while (SystemClock.uptimeMillis() < deadline && chosen == null) {
                Object folder = invoke(transfers, "getFolder");
                if (!(Boolean) folder.getClass().getMethod("getLoading").invoke(folder)) {
                    for (Object entry : (java.util.List<?>) folder.getClass().getMethod("getEntries").invoke(folder)) {
                        if (entry.getClass().getMethod("getId").invoke(entry).equals(identifier)) { chosen = entry; break; }
                    }
                }
                if (chosen == null) SystemClock.sleep(100);
            }
            check(chosen != null, "Browser did not show imported content URI");
            final Object entry = chosen;
            onMain(() -> { try {
                transfers.getMethod("toggle", entry.getClass()).invoke(transfers.getField("INSTANCE").get(null), entry);
            } catch (Exception error) { throw new RuntimeException(error); } });
            android.net.Uri saved = android.net.Uri.parse("content://app.polarbear.files.tests.fixture/" + prefix + "-out.bin");
            ActivityMonitor saver = addMonitor(documentFilter(Intent.ACTION_CREATE_DOCUMENT),
                new ActivityResult(Activity.RESULT_OK, new Intent().setData(saved)), true);
            try {
                onMain(() -> { try { invoke(transfers, "saveSelection"); } catch (Exception e) { throw new RuntimeException(e); } });
                check(saver.getHits() == 1, "Save to Android did not launch Android's save contract");
                awaitTransfer(identifier, "Done");
                File savedCopy = new File(getTargetContext().getCacheDir(), prefix + "-saved");
                try {
                    try (java.io.InputStream input = getTargetContext().getContentResolver().openInputStream(saved);
                         FileOutputStream output = new FileOutputStream(savedCopy)) {
                        byte[] buffer = new byte[256 * 1024]; int count;
                        while ((count = input.read(buffer)) != -1) output.write(buffer, 0, count);
                    }
                    check(Arrays.equals(digest(actual), digest(savedCopy)), "Android saved bytes changed");
                } finally { savedCopy.delete(); getTargetContext().getContentResolver().delete(saved, null, null); }
            } finally { removeMonitor(saver); }
            status("Picker-result import and Android save passed");
            android.net.Uri broken = android.net.Uri.parse("content://app.polarbear.files.tests.fixture/" + prefix + "-broken.bin");
            ActivityMonitor failingPicker = addMonitor(documentFilter(Intent.ACTION_OPEN_DOCUMENT),
                new ActivityResult(Activity.RESULT_OK, new Intent().setData(broken)), true);
            try {
                onMain(() -> { try { invoke(transfers, "chooseFiles"); } catch (Exception e) { throw new RuntimeException(e); } });
                awaitTransfer(broken.toString(), "Failed");
                boolean missing = false;
                try { find(downloads, broken.getLastPathSegment()); } catch (AssertionError expected) { missing = true; }
                check(missing, "Failed provider left a visible partial import");
                status("Provider error after EOF did not publish a partial file");
            } finally { removeMonitor(failingPicker); }
        } finally { actual.delete(); }
    }

    private void singleTouch(long down, int action, float x, float y) {
        if (action == MotionEvent.ACTION_DOWN) { activeDragDown = down; dragInProgress = true; }
        if (action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_CANCEL) dragInProgress = false;
        String name = action == MotionEvent.ACTION_DOWN ? "DOWN" : action == MotionEvent.ACTION_UP ? "UP" :
            action == MotionEvent.ACTION_CANCEL ? "CANCEL" : "MOVE";
        // Explicit display selection avoids vendor rejection of UiAutomation's
        // default display on this tablet. Coordinates come from view bounds.
        try (ParcelFileDescriptor command = getUiAutomation(android.app.UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
                .executeShellCommand("input touchscreen -d 0 motionevent " + name + " " + Math.round(x) + " " + Math.round(y));
             java.io.InputStream output = new ParcelFileDescriptor.AutoCloseInputStream(command)) {
            byte[] bytes = new byte[1024]; while (output.read(bytes) != -1) {}
        } catch (Exception error) { throw new RuntimeException(error); }
    }
    private java.util.concurrent.Future<?> shellDrag(android.graphics.Rect source, android.graphics.Rect target) {
        java.util.concurrent.ExecutorService worker = Executors.newSingleThreadExecutor();
        return worker.submit(() -> {
            try (ParcelFileDescriptor command = getUiAutomation(android.app.UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
                    .executeShellCommand("input touchscreen -d 0 draganddrop " + source.centerX() + " " + source.centerY() + " " + target.centerX() + " " + target.centerY() + " 2200");
                 java.io.InputStream output = new ParcelFileDescriptor.AutoCloseInputStream(command)) {
                byte[] bytes = new byte[1024]; while (output.read(bytes) != -1) {}
            } catch (Exception error) { throw new RuntimeException(error); }
            finally { worker.shutdown(); }
        });
    }
    private android.graphics.Rect fixtureBounds(String text) throws Exception {
        android.app.UiAutomation automation = getUiAutomation(android.app.UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES);
        long deadline = SystemClock.uptimeMillis() + 8000;
        while (SystemClock.uptimeMillis() < deadline) {
            android.view.accessibility.AccessibilityNodeInfo root = automation.getRootInActiveWindow();
            if (root != null) {
                java.util.List<android.view.accessibility.AccessibilityNodeInfo> nodes = root.findAccessibilityNodeInfosByText(text);
                if (!nodes.isEmpty()) {
                    android.graphics.Rect bounds = new android.graphics.Rect(); nodes.get(0).getBoundsInScreen(bounds);
                    if (!bounds.isEmpty()) return bounds;
                }
            }
            SystemClock.sleep(100);
        }
        throw new AssertionError("Drag fixture was not accessible: " + text);
    }
    private void testBatchSave(String downloads) throws Exception {
        String unicode = find(downloads, prefix + "-日本語.bin");
        String empty = find(downloads, prefix + "-empty.bin");
        onMain(() -> { try { invoke(transfers, "beginSave"); } catch (Exception e) { throw new RuntimeException(e); } });
        long deadline = SystemClock.uptimeMillis() + 10000;
        while ((Boolean) invoke(transfers, "getFolder").getClass().getMethod("getLoading").invoke(invoke(transfers, "getFolder")) && SystemClock.uptimeMillis() < deadline) SystemClock.sleep(50);
        Object folder = invoke(transfers, "getFolder");
        for (Object entry : (java.util.List<?>) folder.getClass().getMethod("getEntries").invoke(folder)) {
            String id = (String) entry.getClass().getMethod("getId").invoke(entry);
            if (id.equals(unicode) || id.equals(empty)) onMain(() -> { try {
                transfers.getMethod("toggle", entry.getClass()).invoke(transfers.getField("INSTANCE").get(null), entry);
            } catch (Exception e) { throw new RuntimeException(e); } });
        }
        android.net.Uri tree = android.provider.DocumentsContract.buildTreeDocumentUri("app.polarbear.files.tests.documents", "root");
        ActivityMonitor destination = addMonitor(new android.content.IntentFilter(Intent.ACTION_OPEN_DOCUMENT_TREE),
            new ActivityResult(Activity.RESULT_OK, new Intent().setData(tree)), true);
        try {
            onMain(() -> { try { invoke(transfers, "saveSelection"); } catch (Exception e) { throw new RuntimeException(e); } });
            check(destination.getHits() == 1, "Batch did not use Android's destination-folder picker");
            for (String id : new String[] { unicode, empty }) {
                Object saved = awaitTransfer(id, "Done");
                String name = (String) saved.getClass().getMethod("getName").invoke(saved);
                android.net.Uri uri = android.provider.DocumentsContract.buildDocumentUriUsingTree(tree, name);
                try {
                    long size = 0;
                    try (java.io.InputStream input = getTargetContext().getContentResolver().openInputStream(uri)) {
                        byte[] buffer = new byte[256 * 1024]; int count;
                        while ((count = input.read(buffer)) != -1) {
                            for (int i = 0; i < count; i++) check(buffer[i] == (byte) (size + i), "Batch save changed bytes");
                            size += count;
                        }
                    }
                    check(size == (id.equals(empty) ? 0 : FixtureProvider.LENGTH), "Batch save changed length");
                } finally { android.provider.DocumentsContract.deleteDocument(getTargetContext().getContentResolver(), uri); }
            }
            status("Android batch destination contract, Unicode content and empty-file save passed");
        } finally { removeMonitor(destination); }
    }

    private void testInterruptedSaveRecovery(String downloads) throws Exception {
        android.net.Uri tree = android.provider.DocumentsContract.buildTreeDocumentUri("app.polarbear.files.tests.documents", "root");
        android.net.Uri parent = android.provider.DocumentsContract.buildDocumentUriUsingTree(tree, "root");
        android.net.Uri partial = android.provider.DocumentsContract.createDocument(getTargetContext().getContentResolver(), parent,
            "application/octet-stream", prefix + "-interrupted.bin");
        android.net.Uri complete = android.provider.DocumentsContract.createDocument(getTargetContext().getContentResolver(), parent,
            "application/octet-stream", prefix + "-completed.bin");
        try {
            for (android.net.Uri uri : new android.net.Uri[] { partial, complete }) {
                try (java.io.OutputStream output = getTargetContext().getContentResolver().openOutputStream(uri)) { output.write(42); }
            }
            java.util.concurrent.atomic.AtomicReference<java.util.concurrent.Future<?>> durable = new java.util.concurrent.atomic.AtomicReference<>();
            onMain(() -> { try {
                Method persist = transfers.getDeclaredMethod("persist"); persist.setAccessible(true);
                durable.set((java.util.concurrent.Future<?>) persist.invoke(transfers.getField("INSTANCE").get(null)));
            } catch (Exception e) { throw new RuntimeException(e); } });
            durable.get().get(5, TimeUnit.SECONDS);
            android.util.AtomicFile journal = new android.util.AtomicFile(new File(getTargetContext().getFilesDir(), "file-transfers.json"));
            org.json.JSONArray records = new org.json.JSONArray(new String(journal.readFully(), java.nio.charset.StandardCharsets.UTF_8));
            String source = find(downloads, prefix + "-日本語.bin");
            String interruptedId = UUID.randomUUID().toString();
            for (boolean done : new boolean[] { false, true }) records.put(new JSONObject()
                .put("id", done ? UUID.randomUUID().toString() : interruptedId)
                .put("name", prefix + (done ? "-completed.bin" : "-interrupted.bin"))
                .put("kind", "Save").put("phase", done ? "Done" : "Running").put("total", 1)
                .put("source", source).put("destination", "tree:" + tree).put("output", (done ? complete : partial).toString()));
            java.io.FileOutputStream output = journal.startWrite();
            output.write(records.toString().getBytes(java.nio.charset.StandardCharsets.UTF_8)); journal.finishWrite(output);
            onMain(() -> { try {
                Method restore = transfers.getDeclaredMethod("restore"); restore.setAccessible(true);
                restore.invoke(transfers.getField("INSTANCE").get(null));
            } catch (Exception e) { throw new RuntimeException(e); } });
            long deadline = SystemClock.uptimeMillis() + 10000;
            boolean deleted = false;
            while (!deleted && SystemClock.uptimeMillis() < deadline) {
                try (java.io.InputStream input = getTargetContext().getContentResolver().openInputStream(partial)) { }
                catch (java.io.FileNotFoundException expected) { deleted = true; }
                if (!deleted) SystemClock.sleep(50);
            }
            check(deleted, "Interrupted export left its partial Android document");
            try (java.io.InputStream input = getTargetContext().getContentResolver().openInputStream(complete)) {
                check(input.read() == 42, "Recovery deleted a completed export");
            }
            boolean marked = false;
            for (Object item : (java.util.List<?>) invoke(transfers, "getItems")) {
                if (interruptedId.equals(item.getClass().getMethod("getId").invoke(item)))
                    marked = item.getClass().getMethod("getPhase").invoke(item).toString().equals("Interrupted");
            }
            check(marked, "Interrupted export was retried automatically or lost");
            status("Interrupted Android save cleanup preserved completed files and requires explicit retry");
        } finally {
            try { android.provider.DocumentsContract.deleteDocument(getTargetContext().getContentResolver(), partial); } catch (Exception ignored) {}
            android.provider.DocumentsContract.deleteDocument(getTargetContext().getContentResolver(), complete);
        }
    }

    private void testCrossAppDragging() throws Exception {
        Method receiveDrag = transfers.getDeclaredMethod("receiveDrag", android.view.DragEvent.class);
        receiveDrag.setAccessible(true);
        onMain(() -> { try {
            android.view.View root = (android.view.View) activity.getClass().getMethod("overlayHost").invoke(activity);
            root.setOnDragListener((view, event) -> {
                if (event.getAction() != android.view.DragEvent.ACTION_DRAG_LOCATION) {
                    try {
                        java.lang.reflect.Field proof = overlay.getDeclaredField("desktopReadyLatched"); proof.setAccessible(true);
                        status("Portal drag action=" + event.getAction() + " ready=" + ((java.util.concurrent.atomic.AtomicBoolean) proof.get(null)).get());
                    } catch (Exception ignored) { status("Portal drag action=" + event.getAction()); }
                }
                try { return (Boolean) receiveDrag.invoke(transfers.getField("INSTANCE").get(null), event); }
                catch (Exception error) { throw new RuntimeException(error); }
            });
        } catch (Exception error) { throw new RuntimeException(error); } });
        onMain(() -> { try { overlay.getMethod("setActivePanel", String.class).invoke(overlay.getField("INSTANCE").get(null), "files"); }
            catch (Exception e) { throw new RuntimeException(e); } });
        long measuring = SystemClock.uptimeMillis() + 8000;
        android.graphics.RectF drop = null;
        while (SystemClock.uptimeMillis() < measuring) {
            drop = (android.graphics.RectF) invoke(transfers, "getDropBounds");
            if (!drop.isEmpty()) break;
            SystemClock.sleep(50);
        }
        check(drop != null && !drop.isEmpty(), "Drop target did not measure");
        SystemClock.sleep(400);
        drop = new android.graphics.RectF((android.graphics.RectF) invoke(transfers, "getDropBounds"));
        View desktop = activity.getWindow().getDecorView();
        int[] origin = new int[2]; desktop.getLocationOnScreen(origin);
        android.graphics.Rect destination = new android.graphics.Rect(); drop.round(destination); destination.offset(origin[0], origin[1]);
        onMain(() -> { try { invoke(overlay, "requestReturnToDesktop"); } catch (Exception e) { throw new RuntimeException(e); } });
        waitShowing(false);
        android.net.Uri incoming = android.net.Uri.parse("content://app.polarbear.files.tests.privatefixture/" + prefix + "-d0-uri.bin");
        onMain(() -> activity.startActivity(new Intent().setClassName("app.polarbear.files.tests", "app.polarbear.files.tests.DragFixtureActivity")
            .putExtra("uri", incoming.toString())));
        android.graphics.Rect source = fixtureBounds("Drag test file");
        status("Drag source bounds=" + source.toShortString());
        SystemClock.sleep(400);
        java.util.concurrent.Future<?> incomingGesture = shellDrag(source, destination);
        waitShowing(true);
        incomingGesture.get(10, TimeUnit.SECONDS);
        awaitTransfer(incoming.toString(), "Done");
        status("Real cross-app incoming drag and automatic veil recall passed");


    }

    @Override public void onStart() {
        Bundle result = new Bundle();
        try {
            check(getTargetContext().getPackageName().equals("app.polarbear"), "Tests require Portal Debug");
            prefix = "portal-transfer-qa-" + UUID.randomUUID();
            status("Starting capsule regression " + prefix);
            Intent intent = new Intent().setClassName("app.polarbear", "app.polarbear.PortalActivity")
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK);
            activity = startActivitySync(intent);
            transfers = getTargetContext().getClassLoader().loadClass("app.polarbear.FileTransfers");
            overlay = getTargetContext().getClassLoader().loadClass("app.polarbear.ComposeOverlay");
            run = transfers.getDeclaredMethod("nativeRun", String.class, int.class, int.class, String.class); run.setAccessible(true);
            java.lang.reflect.Field readiness = overlay.getDeclaredField("desktopReadyLatched"); readiness.setAccessible(true);
            long deadline = SystemClock.uptimeMillis() + 90000;
            while (!((java.util.concurrent.atomic.AtomicBoolean) readiness.get(overlay.getField("INSTANCE").get(null))).get() && SystemClock.uptimeMillis() < deadline) SystemClock.sleep(200);
            check(((java.util.concurrent.atomic.AtomicBoolean) readiness.get(overlay.getField("INSTANCE").get(null))).get(), "Desktop never presented a ready frame");
            waitShowing(true);
            onMain(() -> { try { invoke(overlay, "requestReturnToDesktop"); } catch (Exception e) { throw new RuntimeException(e); } });
            waitShowing(false);
            if (motionOnly) {
                status("Motion profile ready");
                SystemClock.sleep(2000);
                profileUtilityMotion();
                result.putString("stream", "Portal motion checks passed\nFixture prefix: " + prefix + "\n");
                finish(Activity.RESULT_OK, result);
                return;
            }
            openCapsule(); waitShowing(true);
            check((Boolean) invoke(overlay, "getSessionMode"), "Capsule did not open session controls");
            status("Utility roundtable recalled the session controls");
            onMain(() -> { try { invoke(overlay, "requestReturnToDesktop"); } catch (Exception e) { throw new RuntimeException(e); } });
            waitShowing(false);
            testCapsuleDrag();
            testContextIcons();
            testGuestTextSuggestion();
            testUtilityInputOwnership();
            testRoundtable();
            if (uiOnly) {
                openCapsule(); waitShowing(true); SystemClock.sleep(700);
                testOutsideDismissal();
                testFileBrowserMotion();
                result.putString("stream", "Portal contextual controls and outside-tap checks passed\nFixture prefix: " + prefix + "\n");
                finish(Activity.RESULT_OK, result);
                return;
            }
            testResponsiveUtility();
            profileUtilityMotion();
            long now = SystemClock.uptimeMillis();
            sendKeySync(new KeyEvent(now, now, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_P, 0, KeyEvent.META_CTRL_ON | KeyEvent.META_ALT_ON));
            sendKeySync(new KeyEvent(now, now + 1, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_P, 0, KeyEvent.META_CTRL_ON | KeyEvent.META_ALT_ON));
            waitShowing(true); status("Keyboard recall passed on device");
            testOutsideDismissal();
            for (int cycle = 0; cycle < 3; cycle++) {
                for (String panel : new String[] { "apps", null, "files", null }) {
                    onMain(() -> { try { overlay.getMethod("setActivePanel", String.class).invoke(overlay.getField("INSTANCE").get(null), panel); }
                        catch (Exception e) { throw new RuntimeException(e); } });
                    SystemClock.sleep(cycle == 2 ? 60 : 500);
                }
            }
            SystemClock.sleep(600);
            status("Apps/Files capsule morphs and rapid open/close transitions passed");
            for (String panel : new String[] { "files", "apps" }) {
                onMain(() -> { try { overlay.getMethod("setActivePanel", String.class).invoke(overlay.getField("INSTANCE").get(null), panel); }
                    catch (Exception e) { throw new RuntimeException(e); } });
                SystemClock.sleep(800);
                capture("-" + panel + "-card");
            }
            String originalFont = shellText("settings get system font_scale");
            String originalNight = shellText("cmd uimode night").split(":")[1].trim().split("\\s+")[0];
            try {
                shellText("settings put system font_scale 1.6");
                shellText("cmd uimode night no");
                SystemClock.sleep(1300);
                for (String panel : new String[] { "files", "apps" }) {
                    onMain(() -> { try { overlay.getMethod("setActivePanel", String.class).invoke(overlay.getField("INSTANCE").get(null), panel); }
                        catch (Exception e) { throw new RuntimeException(e); } });
                    SystemClock.sleep(800);
                    capture("-" + panel + "-light-large");
                }
                testFileBrowserMotion();
            } finally {
                shellText(originalFont.equals("null") ? "settings delete system font_scale" : "settings put system font_scale " + originalFont);
                shellText("cmd uimode night " + originalNight);
                SystemClock.sleep(1300);
            }
            onMain(() -> { try { overlay.getMethod("setActivePanel", String.class).invoke(overlay.getField("INSTANCE").get(null), new Object[] { null }); }
                catch (Exception e) { throw new RuntimeException(e); } });

            String downloads = operation(new JSONObject().put("action", "defaults")).getString("downloads");
            File source = new File(getTargetContext().getCacheDir(), prefix + "-source");
            try {
                try (FileOutputStream output = new FileOutputStream(source)) {
                    byte[] buffer = new byte[256 * 1024];
                    for (int i = 0; i < buffer.length; i++) buffer[i] = (byte) i;
                    for (int i = 0; i < 17; i++) output.write(buffer);
                }
                roundTrip(source, downloads, prefix + "-日本語.bin");
                String one = importFile(source, downloads, prefix + "-collision.bin");
                String two = importFile(source, downloads, prefix + "-collision.bin");
                check(!one.equals(two), "Duplicate import replaced a file");
                try (FileOutputStream output = new FileOutputStream(source)) { }
                roundTrip(source, downloads, prefix + "-empty.bin");
                status("Native descriptor round trips, Unicode, empty files and collisions passed");

                ParcelFileDescriptor[] pipe = ParcelFileDescriptor.createPipe();
                String job = UUID.randomUUID().toString();
                java.util.concurrent.ExecutorService worker = Executors.newSingleThreadExecutor();
                try {
                    java.util.concurrent.Future<String> pending = worker.submit(() -> (String) run.invoke(null,
                        new JSONObject().put("action", "import").put("directory", downloads).put("name", prefix + "-canceled.bin").put("job", job).toString(),
                        pipe[0].getFd(), -1, job));
                    SystemClock.sleep(800);
                    Method cancel = transfers.getDeclaredMethod("nativeCancel", String.class); cancel.setAccessible(true); cancel.invoke(null, job);
                    check(!new JSONObject(pending.get(8, TimeUnit.SECONDS)).optBoolean("ok"), "Canceled transfer reported success");
                    operation(new JSONObject().put("action", "cleanup").put("directory", downloads).put("job", job));
                    status("Blocked-stream cancellation passed");
                } finally { pipe[0].close(); pipe[1].close(); worker.shutdownNow(); }

                testPickerTransfers(downloads);
                testBatchSave(downloads);
                testInterruptedSaveRecovery(downloads);
                testCrossAppDragging();

                onMain(() -> { try { overlay.getMethod("setActivePanel", String.class).invoke(overlay.getField("INSTANCE").get(null), "files"); } catch (Exception e) { throw new RuntimeException(e); } });
                status("PASS; test fixture prefix=" + prefix);
                result.putString("stream", "Portal Files device checks passed\nFixture prefix: " + prefix + "\n");
            } finally { source.delete(); }
            finish(Activity.RESULT_OK, result);
        } catch (Throwable error) {
            if (dragInProgress) { try { singleTouch(activeDragDown, MotionEvent.ACTION_CANCEL, 0, 0); } catch (Throwable ignored) {} }
            java.io.StringWriter stack = new java.io.StringWriter(); error.printStackTrace(new java.io.PrintWriter(stack));
            result.putString("stream", stack.toString()); finish(Activity.RESULT_CANCELED, result);
        }
    }
}
