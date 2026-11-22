import android.content.Context;
import android.os.Looper;
import android.os.SystemClock;
import android.util.DisplayMetrics;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;
import java.lang.reflect.*;
import java.util.*;

/**
 * Exercises the installed Debug controller (app.polarbear.gaming.Gamepad) with a
 * recording sink, floating over a stand-in desktop and as the whole screen; no
 * input reaches a game.
 */
public class ControllerInputProbe {
  static final float[] last = new float[10];
  static int lastButtons;
  static ClassLoader loader;
  static Class<?> controlClass;

  static void check(boolean ok, String what) {
    if (!ok) throw new AssertionError(what + " buttons=" + lastButtons + " axes=" + Arrays.toString(last));
  }
  static boolean neutral() { return lastButtons == 0 && Arrays.equals(last, new float[10]); }

  static boolean pointers(View v, int action, int[] ids, float... xy) {
    MotionEvent.PointerProperties[] props = new MotionEvent.PointerProperties[ids.length];
    MotionEvent.PointerCoords[] coords = new MotionEvent.PointerCoords[ids.length];
    for (int i = 0; i < ids.length; i++) {
      props[i] = new MotionEvent.PointerProperties(); props[i].id = ids[i]; props[i].toolType = MotionEvent.TOOL_TYPE_FINGER;
      coords[i] = new MotionEvent.PointerCoords(); coords[i].x = xy[i * 2]; coords[i].y = xy[i * 2 + 1];
      coords[i].pressure = 1f; coords[i].size = 1f;
    }
    long now = SystemClock.uptimeMillis();
    MotionEvent e = MotionEvent.obtain(now, now, action, ids.length, props, coords, 0, 0, 1f, 1f, 0, 0, InputDevice.SOURCE_TOUCHSCREEN, 0);
    boolean took = v.onTouchEvent(e); e.recycle();
    return took;
  }
  /** One finger on [v]; each view gets its own finger id, as Android gives fingers down together. */
  static boolean event(View v, int action, float x, float y) { return pointers(v, action, new int[] {fingerFor(v)}, x, y); }
  static final List<View> fingers = new ArrayList<>();
  static int fingerFor(View v) {
    if (!fingers.contains(v)) fingers.add(v);
    return fingers.indexOf(v);
  }
  static float cx(View v) { return v.getWidth() / 2f; }
  static float cy(View v) { return v.getHeight() / 2f; }
  /** A thumb lands in the middle of [v]'s control and slides toward the lower right. */
  static void held(View v) {
    event(v, MotionEvent.ACTION_DOWN, cx(v), cy(v));
    event(v, MotionEvent.ACTION_MOVE, cx(v) + v.getWidth() * 0.18f, cy(v) + v.getHeight() * 0.18f);
  }

  public static void main(String[] args) {
    try { run(); } catch (Throwable e) { e.printStackTrace(System.out); System.exit(1); }
  }

  static Object gamepad(Context context, boolean wholeScreen) throws Exception {
    Class<?> sink = loader.loadClass("app.polarbear.gaming.PadSink");
    Class<?> haptics = loader.loadClass("app.polarbear.gaming.PadHaptics");
    Class<?> function0 = loader.loadClass("kotlin.jvm.functions.Function0");
    Object quiet = Proxy.newProxyInstance(loader, new Class<?>[] {function0}, (p, m, a) ->
        m.getName().equals("invoke") ? Boolean.TRUE : defaults(p, m, a));
    Object feel = haptics.getConstructor(Context.class, function0).newInstance(context, quiet);
    Object recorder = Proxy.newProxyInstance(loader, new Class<?>[] {sink}, (p, m, a) -> {
      if (!m.getName().equals("send")) return defaults(p, m, a);
      lastButtons = (Integer) a[0];
      System.arraycopy((float[]) a[1], 0, last, 0, 10);
      return Boolean.TRUE;
    });
    return loader.loadClass("app.polarbear.gaming.Gamepad")
        .getConstructor(Context.class, sink, haptics, boolean.class).newInstance(context, recorder, feel, wholeScreen);
  }

  static Object defaults(Object proxy, Method m, Object[] a) {
    switch (m.getName()) {
      case "hashCode": return System.identityHashCode(proxy);
      case "equals": return proxy == a[0];
      case "toString": return "probe";
      default: return null;
    }
  }

  static void layout(View v, int w, int h) {
    v.measure(View.MeasureSpec.makeMeasureSpec(w, View.MeasureSpec.EXACTLY), View.MeasureSpec.makeMeasureSpec(h, View.MeasureSpec.EXACTLY));
    v.layout(0, 0, w, h);
  }

  /** Android 16 views ask ApplicationSharedMemory, which only an app process sets up. */
  static void prepareSharedMemory() throws Exception {
    Class<?> memory;
    try {
      memory = Class.forName("com.android.internal.os.ApplicationSharedMemory");
    } catch (ClassNotFoundException older) {
      return;
    }
    try {
      memory.getMethod("getInstance").invoke(null);
      return;
    } catch (InvocationTargetException missing) {
      // Not initialized: make one for this process.
    }
    Object created = memory.getMethod("create").invoke(null);
    for (Field field : memory.getDeclaredFields()) {
      if (Modifier.isStatic(field.getModifiers()) && field.getType() == memory) {
        field.setAccessible(true);
        if (field.get(null) == null) field.set(null, created);
      }
    }
  }

  static void run() throws Exception {
    Looper.prepareMainLooper();
    prepareSharedMemory();
    Method fonts = android.graphics.Typeface.class.getDeclaredMethod("loadPreinstalledSystemFontMap");
    fonts.setAccessible(true); fonts.invoke(null);
    Class<?> at = Class.forName("android.app.ActivityThread");
    Object thread = at.getMethod("systemMain").invoke(null);
    Context system = (Context) at.getMethod("getSystemContext").invoke(thread);
    Context context = system.createPackageContext("app.polarbear", Context.CONTEXT_INCLUDE_CODE | Context.CONTEXT_IGNORE_SECURITY);
    check((context.getApplicationInfo().flags & android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE) != 0, "Portal Debug required");
    loader = context.getClassLoader();
    controlClass = loader.loadClass("app.polarbear.gaming.Control");
    Class<?> gamepadClass = loader.loadClass("app.polarbear.gaming.Gamepad");
    Class<?> function0 = loader.loadClass("kotlin.jvm.functions.Function0");
    DisplayMetrics metrics = context.getResources().getDisplayMetrics();
    int w = Math.max(metrics.widthPixels, metrics.heightPixels);
    int h = Math.min(metrics.widthPixels, metrics.heightPixels);

    // ---- Floating over the desktop: one view per control. ----
    Object floating = gamepad(context, false);
    FrameLayout desktop = new FrameLayout(context);
    Object index = Proxy.newProxyInstance(loader, new Class<?>[] {function0}, (p, m, a) ->
        m.getName().equals("invoke") ? Integer.valueOf(0) : defaults(p, m, a));
    gamepadClass.getMethod("floatIn", FrameLayout.class, function0).invoke(floating, desktop, index);
    gamepadClass.getMethod("resize", int.class, int.class).invoke(floating, w, h);
    gamepadClass.getMethod("show", boolean.class).invoke(floating, false);
    layout(desktop, w, h);
    Map<String, View> views = new HashMap<>();
    Method controls = loader.loadClass("app.polarbear.gaming.GamepadView").getMethod("getControls");
    for (int i = 0; i < desktop.getChildCount(); i++) {
      View v = desktop.getChildAt(i);
      List<?> owned = (List<?>) controls.invoke(v);
      check(owned.size() == 1, "one control per floating view");
      views.put(((Enum<?>) owned.get(0)).name(), v);
      check(!v.onGenericMotionEvent(MotionEvent.obtain(0, 0, MotionEvent.ACTION_HOVER_MOVE, 1, 1, 0)), "hover reaches the desktop");
    }
    check(views.size() == 13, "a view for every control, got " + views.keySet());
    check(neutral(), "plugged in at rest");
    View left = views.get("LEFT_STICK"), right = views.get("RIGHT_STICK");

    held(left); check(last[0] > 0 && last[1] > 0, "left deflects");
    event(left, MotionEvent.ACTION_UP, cx(left), cy(left)); check(neutral(), "UP reaches host");
    held(left); gamepadClass.getMethod("release").invoke(floating); check(neutral(), "release reaches host");
    held(right); right.onWindowFocusChanged(false); check(neutral(), "focus loss reaches host");
    held(left); event(left, MotionEvent.ACTION_CANCEL, cx(left), cy(left)); check(neutral(), "CANCEL reaches host");
    held(left); event(left, MotionEvent.ACTION_DOWN, cx(left), cy(left)); check(last[0] == 0 && last[1] == 0, "fresh DOWN clears stale stream");
    event(left, MotionEvent.ACTION_UP, cx(left), cy(left));
    held(left); held(right); event(left, MotionEvent.ACTION_UP, cx(left), cy(left));
    check(last[0] == 0 && last[1] == 0 && last[2] > 0 && last[3] > 0, "sticks release independently");
    event(right, MotionEvent.ACTION_UP, cx(right), cy(right)); check(neutral(), "both sticks neutral");

    float sx = cx(left), sy = cy(left), far = left.getWidth() * 0.18f;
    pointers(left, MotionEvent.ACTION_DOWN, new int[] {4}, sx, sy);
    pointers(left, MotionEvent.ACTION_POINTER_DOWN | (1 << MotionEvent.ACTION_POINTER_INDEX_SHIFT), new int[] {4, 9}, sx + far, sy, sx, sy);
    pointers(left, MotionEvent.ACTION_MOVE, new int[] {4, 9}, sx + far, sy, sx, sy);
    check(last[0] > 0, "owner keeps the stick with a second finger on it");
    pointers(left, MotionEvent.ACTION_POINTER_UP, new int[] {4, 9}, sx + far, sy, sx, sy);
    check(last[0] == 0 && last[1] == 0, "lifting the owner releases despite the second finger");
    pointers(left, MotionEvent.ACTION_UP, new int[] {9}, sx, sy);
    pointers(left, MotionEvent.ACTION_DOWN, new int[] {4}, sx, sy);
    pointers(left, MotionEvent.ACTION_MOVE, new int[] {4}, sx + far, sy);
    pointers(left, MotionEvent.ACTION_MOVE, new int[] {9}, sx + far, sy);
    check(last[0] == 0 && last[1] == 0, "missing owner pointer releases");
    gamepadClass.getMethod("release").invoke(floating);

    // A finger off every control is the desktop's: the view refuses it.
    View guide = views.get("GUIDE");
    check(!event(guide, MotionEvent.ACTION_DOWN, 0, 0), "a corner of a control's view passes to the desktop");
    check(neutral(), "a refused finger presses nothing");

    for (String name : new String[] {"DPAD", "FACE", "LT", "RT", "LB", "RB", "BACK", "GUIDE", "START", "L3", "R3"}) {
      View v = views.get(name);
      held(v); check(!neutral(), name + " activates");
      event(v, MotionEvent.ACTION_UP, cx(v), cy(v)); check(neutral(), name + " UP releases");
      held(v); event(v, MotionEvent.ACTION_CANCEL, cx(v), cy(v)); check(neutral(), name + " CANCEL releases");
    }

    // ---- The layout editor: a touch takes the control nearest it, even through a neighbour's view. ----
    Class<?> function1 = loader.loadClass("kotlin.jvm.functions.Function1");
    Class<?> function3 = loader.loadClass("kotlin.jvm.functions.Function3");
    final Object[] picked = new Object[1];
    final Object[] dropped = new Object[1];
    gamepadClass.getMethod("setOnSelect", function1).invoke(floating, Proxy.newProxyInstance(loader, new Class<?>[] {function1}, (p, m, a) -> {
      if (!m.getName().equals("invoke")) return defaults(p, m, a);
      picked[0] = a[0];
      return null;
    }));
    gamepadClass.getMethod("setOnMoved", function3).invoke(floating, Proxy.newProxyInstance(loader, new Class<?>[] {function3}, (p, m, a) -> {
      if (!m.getName().equals("invoke")) return defaults(p, m, a);
      dropped[0] = a[0];
      return null;
    }));
    gamepadClass.getMethod("setEditing", boolean.class).invoke(floating, true);
    check(!event(guide, MotionEvent.ACTION_DOWN, 0, 0), "in the editor, a corner of a control's view passes to the desktop");
    View over = null, under = null;
    for (Map.Entry<String, View> a : views.entrySet()) for (Map.Entry<String, View> b : views.entrySet()) {
      View top = a.getValue(), other = b.getValue();
      if (top == other) continue;
      float x = other.getLeft() + cx(other) - top.getLeft(), y = other.getTop() + cy(other) - top.getTop();
      if (x < 0 || y < 0 || x >= top.getWidth() || y >= top.getHeight()) continue;
      picked[0] = null;
      check(event(top, MotionEvent.ACTION_DOWN, x, y), "the editor takes a touch on " + b.getKey());
      check(picked[0] != null && ((Enum<?>) picked[0]).name().equals(b.getKey()),
          "a touch on " + b.getKey() + " through " + a.getKey() + "'s view selects " + b.getKey() + ", got " + picked[0]);
      event(top, MotionEvent.ACTION_UP, x, y);
      if (over == null) { over = top; under = other; }
    }
    check(over != null, "some control's view lies over a neighbour");
    // Dragging through the neighbour's view moves the neighbour, and the drop says where.
    float ox = under.getLeft() + cx(under) - over.getLeft(), oy = under.getTop() + cy(under) - over.getTop();
    float step = 30f * metrics.density;
    event(over, MotionEvent.ACTION_DOWN, ox, oy);
    event(over, MotionEvent.ACTION_MOVE, ox + step, oy);
    check(under.getTranslationX() > step * 0.9f && over.getTranslationX() == 0f, "a drag moves the control under the finger");
    dropped[0] = null;
    event(over, MotionEvent.ACTION_UP, ox + step, oy);
    check(dropped[0] != null && dropped[0] == picked[0], "the drop reports the moved control");
    // A tap that does not drag leaves the control where it was.
    View face = views.get("FACE");
    face.setTranslationX(0f);
    event(face, MotionEvent.ACTION_DOWN, cx(face), cy(face));
    event(face, MotionEvent.ACTION_MOVE, cx(face) + 1f, cy(face));
    dropped[0] = null;
    event(face, MotionEvent.ACTION_UP, cx(face) + 1f, cy(face));
    check(dropped[0] == null && face.getTranslationX() == 0f, "a tap selects without moving");
    gamepadClass.getMethod("setEditing", boolean.class).invoke(floating, false);
    check(neutral(), "the editor presses nothing");

    gamepadClass.getMethod("detach").invoke(floating);
    check(desktop.getChildCount() == 0, "detach takes every view");

    // ---- The whole screen: one view takes every finger. ----
    Object whole = gamepad(context, true);
    View screen = (View) gamepadClass.getMethod("screenView").invoke(whole);
    layout(screen, w, h);
    gamepadClass.getMethod("show", boolean.class).invoke(whole, false);
    check(screen.onGenericMotionEvent(MotionEvent.obtain(0, 0, MotionEvent.ACTION_HOVER_MOVE, 1, 1, 0)), "the page swallows hover");
    check(event(screen, MotionEvent.ACTION_DOWN, w * 0.1f, h * 0.62f), "the whole screen takes a landing");
    event(screen, MotionEvent.ACTION_MOVE, w * 0.15f, h * 0.62f);
    check(last[0] > 0, "a stick floats to the thumb on its half");
    event(screen, MotionEvent.ACTION_UP, w * 0.15f, h * 0.62f); check(neutral(), "whole-screen UP releases");

    System.out.println("Gamepad on device: floating views (UP, CANCEL, release, focus loss, lost UP, second fingers, two sticks, every control, desktop pass-through, layout editor) and the whole screen passed");
    System.exit(0);
  }
}
