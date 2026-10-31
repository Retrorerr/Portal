import android.content.Context;
import android.os.Looper;
import android.os.SystemClock;
import android.view.MotionEvent;
import android.view.View;
import java.lang.reflect.*;
import java.util.*;

/** Exercises the installed Debug controller with a recording host; no input reaches a game. */
public class ControllerInputProbe {
  static final ArrayList<View> views = new ArrayList<>();
  static final float[] last = new float[10];
  static Method writeAxes;
  static Method getButtons;
  static int lastButtons;
  static void sample() throws Exception {
    Arrays.fill(last, 0f); lastButtons = 0;
    for (View v : views) { writeAxes.invoke(v, (Object) last); lastButtons |= (Integer) getButtons.invoke(v); }
  }
  static void check(boolean ok, String what) { if (!ok) throw new AssertionError(what + " " + Arrays.toString(last)); }
  static void event(View v, int action, float x, float y) {
    long now = SystemClock.uptimeMillis();
    MotionEvent e = MotionEvent.obtain(now, now, action, x, y, 0);
    v.onTouchEvent(e); e.recycle();
  }
  static void pointers(View v, int action, int[] ids, float... xy) {
    MotionEvent.PointerProperties[] props = new MotionEvent.PointerProperties[ids.length];
    MotionEvent.PointerCoords[] coords = new MotionEvent.PointerCoords[ids.length];
    for (int i = 0; i < ids.length; i++) {
      props[i] = new MotionEvent.PointerProperties(); props[i].id = ids[i]; props[i].toolType = 1;
      coords[i] = new MotionEvent.PointerCoords(); coords[i].x = xy[i * 2]; coords[i].y = xy[i * 2 + 1];
      coords[i].pressure = 1f; coords[i].size = 1f;
    }
    long now = SystemClock.uptimeMillis();
    MotionEvent e = MotionEvent.obtain(now, now, action, ids.length, props, coords, 0, 0, 1f, 1f, 0, 0, android.view.InputDevice.SOURCE_TOUCHSCREEN, 0);
    v.onTouchEvent(e); e.recycle();
  }
  static void held(View v) { event(v, 0, 120, 120); event(v, 2, 210, 60); }
  public static void main(String[] args) {
    try { run(); } catch (Throwable e) { e.printStackTrace(System.out); System.exit(1); }
  }
  static void run() throws Exception {
    Looper.prepareMainLooper();
    Method fonts = android.graphics.Typeface.class.getDeclaredMethod("loadPreinstalledSystemFontMap");
    fonts.setAccessible(true); fonts.invoke(null);
    Class<?> at = Class.forName("android.app.ActivityThread");
    Object thread = at.getMethod("systemMain").invoke(null);
    Context system = (Context) at.getMethod("getSystemContext").invoke(thread);
    Context context = system.createPackageContext("app.polarbear", Context.CONTEXT_INCLUDE_CODE | Context.CONTEXT_IGNORE_SECURITY);
    check((context.getApplicationInfo().flags & android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE) != 0, "Portal Debug required");
    ClassLoader loader = context.getClassLoader();
    Class<?> control = loader.loadClass("app.polarbear.gaming.Control");
    Class<?> host = loader.loadClass("app.polarbear.gaming.ControlHost");
    Class<?> view = loader.loadClass("app.polarbear.gaming.ControlView");
    writeAxes = view.getMethod("writeAxes", float[].class);
    getButtons = view.getMethod("getButtons");
    Object fake = Proxy.newProxyInstance(loader, new Class<?>[] {host}, (proxy, method, values) -> {
      switch (method.getName()) {
        case "getEditing": case "getAnalogTriggers": return false;
        case "padChanged": sample(); return null;
        default: return null;
      }
    });
    for (String name : new String[] {"LEFT_STICK", "RIGHT_STICK"}) {
      View v = (View) view.getConstructor(Context.class, control, host).newInstance(context, control.getField(name).get(null), fake);
      view.getMethod("setGeometry", float.class, float.class, float.class).invoke(v, 200f, 200f, 20f);
      v.layout(0, 0, 240, 240); views.add(v);
    }
    View left = views.get(0), right = views.get(1);
    held(left); check(last[0] > 0 && last[1] < 0, "left deflects");
    event(left, 1, 210, 60); check(last[0] == 0 && last[1] == 0, "UP reaches host");
    held(left); view.getMethod("release").invoke(left); check(last[0] == 0 && last[1] == 0, "reset reaches host");
    held(right); right.onWindowFocusChanged(false); check(last[2] == 0 && last[3] == 0, "focus loss reaches host");
    held(left); event(left, 3, 210, 60); check(last[0] == 0 && last[1] == 0, "CANCEL reaches host");
    held(left); event(left, 0, 120, 120); check(last[0] == 0 && last[1] == 0, "fresh DOWN clears stale stream");
    held(left); held(right); event(left, 1, 210, 60);
    check(last[0] == 0 && last[1] == 0 && last[2] > 0 && last[3] < 0, "sticks release independently");
    event(right, 1, 210, 60); check(Arrays.equals(last, new float[10]), "both sticks neutral");
    pointers(left, 0, new int[] {4}, 120, 120);
    pointers(left, 5 | (1 << 8), new int[] {4, 9}, 210, 60, 100, 120);
    check(last[0] > 0, "owner retains stick with a second finger");
    pointers(left, 6, new int[] {4, 9}, 210, 60, 100, 120);
    check(last[0] == 0 && last[1] == 0, "lifting owner releases despite second finger");
    pointers(left, 1, new int[] {9}, 100, 120);
    pointers(left, 0, new int[] {4}, 210, 60);
    pointers(left, 2, new int[] {9}, 210, 60);
    check(last[0] == 0 && last[1] == 0, "missing owner pointer releases");
    view.getMethod("release").invoke(left);
    for (String name : new String[] {"DPAD", "FACE", "LT", "RT", "LB", "RB", "BACK", "GUIDE", "START", "L3", "R3"}) {
      View v = (View) view.getConstructor(Context.class, control, host).newInstance(context, control.getField(name).get(null), fake);
      view.getMethod("setGeometry", float.class, float.class, float.class).invoke(v, 200f, 200f, 20f);
      v.layout(0, 0, 240, 240); views.add(v);
      held(v); check(lastButtons != 0 || !Arrays.equals(last, new float[10]), name + " activates");
      event(v, 1, 210, 60); check(lastButtons == 0 && Arrays.equals(last, new float[10]), name + " UP releases");
      held(v); event(v, 3, 210, 60); check(lastButtons == 0 && Arrays.equals(last, new float[10]), name + " CANCEL releases");
      views.remove(v);
    }
    System.out.println("Actual ControlView: UP, CANCEL, reset, focus loss, missing old UP multi-touch ownership, two-stick independence, buttons, triggers and D-pad passed");
    System.exit(0);
  }
}
