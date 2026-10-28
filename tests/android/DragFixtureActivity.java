package app.polarbear.files.tests;

import android.app.Activity;
import android.content.ClipData;
import android.content.Intent;
import android.os.Bundle;
import android.view.DragEvent;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.View;
import android.view.WindowManager;
import android.widget.TextView;
import java.security.MessageDigest;

/** A separate UID supplies/receives real Android drags without changing split-screen settings. */
public final class DragFixtureActivity extends Activity {
    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        boolean receiving = getIntent().getBooleanExtra("receive", false);
        TextView view = new TextView(this);
        view.setText(receiving ? "Drop test file here" : "Drag test file");
        view.setBackgroundColor(0xfff1ebdd); view.setTextColor(0xff191b1c);
        view.setGravity(Gravity.CENTER); view.setTextSize(18); view.setPadding(16, 16, 16, 16);
        setContentView(view);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL);
        getWindow().clearFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND);
        getWindow().setGravity(receiving ? Gravity.BOTTOM | Gravity.RIGHT : Gravity.TOP | Gravity.LEFT);
        int dp = (int) getResources().getDisplayMetrics().density;
        WindowManager.LayoutParams attributes = getWindow().getAttributes();
        attributes.x = 24 * dp; attributes.y = 80 * dp; getWindow().setAttributes(attributes);
        getWindow().setLayout(220 * dp, 120 * dp);
        if (!receiving) view.setOnTouchListener((target, event) -> {
            if (event.getActionMasked() == MotionEvent.ACTION_DOWN) target.postDelayed(() -> {
                android.net.Uri uri = android.net.Uri.parse(getIntent().getStringExtra("uri"));
                boolean started = target.startDragAndDrop(ClipData.newUri(getContentResolver(), "Test file", uri),
                    new View.DragShadowBuilder(target), "fixture", View.DRAG_FLAG_GLOBAL | View.DRAG_FLAG_GLOBAL_URI_READ);
                android.util.Log.i("PortalDragFixture", "source drag started=" + started);
            }, 150);
            return true;
        });
        view.setOnDragListener((target, event) -> {
            if (event.getAction() == DragEvent.ACTION_DROP && receiving) {
                android.view.DragAndDropPermissions permission = requestDragAndDropPermissions(event);
                android.net.Uri uri = event.getClipData().getItemAt(0).getUri();
                new Thread(() -> {
                    Intent result = new Intent("app.polarbear.files.tests.DRAG_RESULT").setPackage("app.polarbear")
                        .putExtra("nonce", getIntent().getStringExtra("nonce"));
                    try (java.io.InputStream input = getContentResolver().openInputStream(uri)) {
                        MessageDigest digest = MessageDigest.getInstance("SHA-256");
                        byte[] buffer = new byte[256 * 1024]; int count; long size = 0;
                        while ((count = input.read(buffer)) != -1) { digest.update(buffer, 0, count); size += count; }
                        result.putExtra("size", size).putExtra("digest", digest.digest());
                    } catch (Exception error) { result.putExtra("error", error.toString()); }
                    finally { if (permission != null) permission.release(); }
                    sendBroadcast(result); runOnUiThread(this::finish);
                }, "drag-receiver").start();
            }
            if (event.getAction() == DragEvent.ACTION_DRAG_ENDED && !receiving) finish();
            return true;
        });
    }
}
