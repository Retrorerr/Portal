package app.polarbear

// Microphone and camera access for Linux apps.
//
// The guest always sees "Portal Microphone" and the Portal cameras, but
// Android's RECORD_AUDIO / CAMERA prompt appears only when a Linux app
// actually starts recording or opens a camera (the native capture helper
// reports that through utils/capture_permissions.rs). The answer goes back to
// native code; without a grant, apps get silence or black frames instead of
// an error, and a short note says how to allow it later.

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.os.Build
import android.os.SystemClock
import android.util.Log
import android.view.Surface
import android.widget.Toast

object CapturePermissions {
    private const val TAG = "PortalCapture"
    private const val MIC = 0
    private const val CAMERA = 1
    private const val REQUEST_BASE = 2100
    private const val DENIED_NOTE_INTERVAL_MS = 60_000L

    private val pending = BooleanArray(2)
    private val lastDeniedNote = LongArray(2) { -DENIED_NOTE_INTERVAL_MS }

    @JvmStatic private external fun nativeCapturePermission(kind: Int, granted: Boolean)
    @JvmStatic private external fun nativeDisplayRotation(degrees: Int)

    // The native library may not be loaded yet (or failed to load); capture
    // is optional, so never let that take the activity down.
    private fun tell(kind: Int, allowed: Boolean) {
        try {
            nativeCapturePermission(kind, allowed)
        } catch (e: UnsatisfiedLinkError) {
            Log.w(TAG, "native capture bridge unavailable", e)
        }
    }

    private fun permission(kind: Int) =
        if (kind == MIC) Manifest.permission.RECORD_AUDIO else Manifest.permission.CAMERA

    private fun label(kind: Int) = if (kind == MIC) "microphone" else "camera"

    private fun granted(activity: Activity, kind: Int): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.M ||
            activity.checkSelfPermission(permission(kind)) == PackageManager.PERMISSION_GRANTED

    /** A Linux app started using the microphone or a camera. */
    fun request(activity: Activity, kind: Int) {
        if (kind != MIC && kind != CAMERA) return
        if (granted(activity, kind)) {
            tell(kind, true)
            return
        }
        if (pending[kind] || Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return
        pending[kind] = true
        Log.i(TAG, "asking for the ${label(kind)}")
        Toast.makeText(activity, "A Linux app wants to use the ${label(kind)}", Toast.LENGTH_SHORT).show()
        activity.requestPermissions(arrayOf(permission(kind)), REQUEST_BASE + kind)
    }

    /** Returns true when the result belonged to a capture request. */
    fun onResult(activity: Activity, requestCode: Int, grantResults: IntArray): Boolean {
        val kind = requestCode - REQUEST_BASE
        if (kind != MIC && kind != CAMERA) return false
        pending[kind] = false
        val allowed = grantResults.isNotEmpty() && grantResults[0] == PackageManager.PERMISSION_GRANTED
        Log.i(TAG, "${label(kind)} allowed=$allowed")
        tell(kind, allowed)
        if (!allowed) {
            val now = SystemClock.elapsedRealtime()
            if (now - lastDeniedNote[kind] >= DENIED_NOTE_INTERVAL_MS) {
                lastDeniedNote[kind] = now
                val what = if (kind == MIC) "record silence" else "see a black picture"
                Toast.makeText(
                    activity,
                    "No ${label(kind)} access: Linux apps $what. Allow it in Android Settings › Apps › Portal › Permissions.",
                    Toast.LENGTH_LONG,
                ).show()
            }
        }
        return true
    }

    /** Back in Portal: pick up a permission the user granted in Settings. */
    fun refresh(activity: Activity) {
        for (kind in intArrayOf(MIC, CAMERA)) {
            if (granted(activity, kind)) tell(kind, true)
        }
        reportRotation(activity)
    }

    /** The display rotation, so camera frames stay upright. */
    fun reportRotation(activity: Activity) {
        val rotation = try {
            @Suppress("DEPRECATION")
            activity.windowManager.defaultDisplay.rotation
        } catch (e: Exception) {
            Surface.ROTATION_0
        }
        val degrees = when (rotation) {
            Surface.ROTATION_90 -> 90
            Surface.ROTATION_180 -> 180
            Surface.ROTATION_270 -> 270
            else -> 0
        }
        try {
            nativeDisplayRotation(degrees)
        } catch (e: UnsatisfiedLinkError) {
            Log.w(TAG, "native capture bridge unavailable", e)
        }
    }
}
