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

    @Override public void onCreate(Bundle arguments) { super.onCreate(arguments); start(); }

    private void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }
    private Object invoke(Class<?> type, String name) throws Exception {
        return type.getMethod(name).invoke(type.getField("INSTANCE").get(null));
    }
    private void onMain(Runnable action) { runOnMainSync(action); }
    private void status(String text) {
        Bundle result = new Bundle(); result.putString("stream", text + "\n"); sendStatus(0, result);
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
                if (capsule != null) capsule.getGlobalVisibleRect(bounds);
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
    private void openCapsule() throws Exception {
        android.graphics.Rect bounds = capsuleBounds();
        long down = SystemClock.uptimeMillis();
        singleTouch(down, MotionEvent.ACTION_DOWN, bounds.centerX(), bounds.centerY());
        singleTouch(down, MotionEvent.ACTION_UP, bounds.centerX(), bounds.centerY());
    }
    private void testCapsuleDrag() throws Exception {
        android.graphics.Rect original = capsuleBounds();
        float density = activity.getResources().getDisplayMetrics().density;
        long down = SystemClock.uptimeMillis();
        singleTouch(down, MotionEvent.ACTION_DOWN, original.centerX(), original.centerY());
        for (int i = 1; i <= 12; i++) {
            singleTouch(down, MotionEvent.ACTION_MOVE, original.centerX() + i * 5 * density, original.centerY() + i * 5 * density);
            SystemClock.sleep(16);
        }
        singleTouch(down, MotionEvent.ACTION_UP, original.centerX() + 60 * density, original.centerY() + 60 * density);
        SystemClock.sleep(300);
        check(!showing(), "Dragging accidentally opened the veil");
        android.graphics.Rect moved = capsuleBounds();
        check(moved.centerY() > original.centerY() + 30 * density, "Capsule did not follow the drag");
        openCapsule(); waitShowing(true);
        onMain(() -> { try { invoke(overlay, "requestReturnToDesktop"); } catch (Exception e) { throw new RuntimeException(e); } });
        waitShowing(false);
        android.graphics.Rect restored = capsuleBounds();
        check(Math.abs(restored.centerY() - moved.centerY()) < 3 * density, "Reopening lost capsule position");
        status("Capsule dragging, click cancellation and position restoration passed");
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
            openCapsule(); waitShowing(true);
            check((Boolean) invoke(overlay, "getSessionMode"), "Capsule did not open session controls");
            status("Floating capsule tap recalled the session controls");
            onMain(() -> { try { invoke(overlay, "requestReturnToDesktop"); } catch (Exception e) { throw new RuntimeException(e); } });
            waitShowing(false);
            testCapsuleDrag();
            long now = SystemClock.uptimeMillis();
            sendKeySync(new KeyEvent(now, now, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_P, 0, KeyEvent.META_CTRL_ON | KeyEvent.META_ALT_ON));
            sendKeySync(new KeyEvent(now, now + 1, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_P, 0, KeyEvent.META_CTRL_ON | KeyEvent.META_ALT_ON));
            waitShowing(true); status("Keyboard recall passed on device");
            for (int cycle = 0; cycle < 3; cycle++) {
                for (String panel : new String[] { "apps", null, "files", null }) {
                    onMain(() -> { try { overlay.getMethod("setActivePanel", String.class).invoke(overlay.getField("INSTANCE").get(null), panel); }
                        catch (Exception e) { throw new RuntimeException(e); } });
                    SystemClock.sleep(cycle == 2 ? 60 : 500);
                }
            }
            SystemClock.sleep(600);
            status("Apps/Files capsule morphs and rapid open/close transitions passed");

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
