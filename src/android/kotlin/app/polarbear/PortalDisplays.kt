package app.polarbear

import android.app.Activity
import android.app.ActivityManager
import android.app.ActivityOptions
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.hardware.display.DisplayManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.OutcomeReceiver
import android.provider.Settings
import android.util.Log
import android.view.Display
import android.view.Surface
import android.widget.Toast
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

internal enum class TabletEdge { Left, Right }

/** Moves the running Portal activity between Android displays. */
internal class PortalDisplays(private val host: PortalActivity) {
    data class Target(val id: Int, val name: String, val resolution: String, val current: Boolean)

    private val displayManager = host.getSystemService(Context.DISPLAY_SERVICE) as DisplayManager
    private val activityManager = host.getSystemService(Context.ACTIVITY_SERVICE) as ActivityManager
    private val handler = Handler(Looper.getMainLooper())
    private val preferences = host.getPreferences(Context.MODE_PRIVATE)
    private var movePending = false

    var tabletEdge by mutableStateOf(
        if (preferences.getString(PREF_TABLET_EDGE, null) == TabletEdge.Right.name) TabletEdge.Right else TabletEdge.Left,
    )
        private set

    fun setTabletPosition(edge: TabletEdge) {
        tabletEdge = edge
        preferences.edit().putString(PREF_TABLET_EDGE, edge.name).apply()
        DesktopControlActivity.placeHandle()
    }

    private var floatingWanted by mutableStateOf(preferences.getBoolean(PREF_FLOATING_CONTROLS, true))
    private var overlayAllowed by mutableStateOf(canDrawOverlays())

    /** Keep Control desktop floating on the tablet (Display over other apps). */
    val floatingControls: Boolean get() = floatingWanted && overlayAllowed

    fun setFloatingControls(enabled: Boolean) {
        floatingWanted = enabled
        preferences.edit().putBoolean(PREF_FLOATING_CONTROLS, enabled).apply()
        if (enabled && !overlayAllowed) openOverlaySettings(host)
        DesktopControlActivity.displayChanged()
    }

    /** UI thread, on resume: Android's permission page may have changed the answer. */
    fun refreshFloatingControls() {
        overlayAllowed = canDrawOverlays()
    }

    private fun canDrawOverlays() = Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && Settings.canDrawOverlays(host)

    /** The release keys are shown on the first few grabs only. */
    fun takeReleaseHint(): Boolean {
        val shown = preferences.getInt(PREF_RELEASE_HINTS, 0)
        if (shown >= RELEASE_HINTS) return false
        preferences.edit().putInt(PREF_RELEASE_HINTS, shown + 1).apply()
        return true
    }

    val supported = Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && host.packageManager.hasSystemFeature(
        PackageManager.FEATURE_ACTIVITIES_ON_SECONDARY_DISPLAYS,
    )

    fun targets(): List<Target> {
        val currentId = host.currentDisplay()?.displayId
        val intent = launchIntent()
        val displays = displayManager.displays.filter {
            it.displayId == currentId || canMoveTo(it, intent)
        }.sortedBy { it.displayId }
        val names = displays.associateWith(::displayName)
        val nameCounts = names.values.groupingBy { it }.eachCount()
        return displays.map { display ->
            val id = display.displayId
            val name = names.getValue(display)
            val label = when {
                id == Display.DEFAULT_DISPLAY -> "Built-in screen"
                nameCounts.getValue(name) > 1 -> "$name ($id)"
                else -> name
            }
            val mode = display.mode
            val rotated = display.rotation == Surface.ROTATION_90 || display.rotation == Surface.ROTATION_270
            val width = if (rotated) mode.physicalHeight else mode.physicalWidth
            val height = if (rotated) mode.physicalWidth else mode.physicalHeight
            Target(id, label, "$width × $height", id == currentId)
        }
    }

    /** The monitor's own name (EDID) where Android reports it, otherwise Android's label. */
    fun displayName(display: Display): String {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            display.deviceProductInfo?.name?.trim()?.takeIf { it.isNotEmpty() }?.let { return it }
        }
        return display.name
    }

    fun observe(onChange: () -> Unit): () -> Unit {
        val listener = object : DisplayManager.DisplayListener {
            override fun onDisplayAdded(displayId: Int) = onChange()
            override fun onDisplayChanged(displayId: Int) = onChange()
            override fun onDisplayRemoved(displayId: Int) = onChange()
        }
        displayManager.registerDisplayListener(listener, handler)
        return { displayManager.unregisterDisplayListener(listener) }
    }

    fun moveTo(displayId: Int): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O || movePending) return false
        val display = displayManager.displays.firstOrNull { it.displayId == displayId }
        val intent = launchIntent()
        if (!host.isTaskRoot || display == null || !canMoveTo(display, intent)) {
            showMoveError()
            return false
        }

        try {
            host.startActivity(intent, displayOptions(displayId))
        } catch (e: RuntimeException) {
            // The display can disappear after Android's permission check.
            Log.w(TAG, "Display move to $displayId failed", e)
            showMoveError()
            return false
        }

        // Some window managers silently ignore a display request. Check the destination
        // after Android delivers the window move and its new configuration.
        movePending = true
        handler.postDelayed({
            movePending = false
            if (!host.isFinishing && !host.isDestroyed) {
                if (host.currentDisplay()?.displayId == displayId) {
                    DesktopControlActivity.displayChanged()
                    SessionService.refreshNotification()
                    if (displayId != Display.DEFAULT_DISPLAY) {
                        requestFullscreen(displayId)
                        DesktopControlActivity.showOnTablet(host)
                    }
                } else showMoveError()
            }
        }, MOVE_TIMEOUT_MS)
        return true
    }

    private fun launchIntent(): Intent = moveIntent(host)

    private fun canMoveTo(display: Display, intent: Intent): Boolean {
        // Android may destroy a task when the owner of a private display removes it.
        if (!display.isValid || display.flags and Display.FLAG_PRIVATE != 0) return false
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return true
        return activityManager.isActivityStartAllowedOnDisplay(host, display.displayId, intent)
    }

    private fun requestFullscreen(displayId: Int) {
        if (displayId == Display.DEFAULT_DISPLAY || Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE ||
            !host.isInMultiWindowMode
        ) return
        host.requestFullscreenMode(Activity.FULLSCREEN_MODE_REQUEST_ENTER, object : OutcomeReceiver<Void, Throwable> {
            override fun onResult(result: Void?) = Unit

            override fun onError(error: Throwable) {
                Log.w(TAG, "Fullscreen request on display $displayId failed", error)
            }
        })
    }

    private fun showMoveError() {
        Toast.makeText(host, "Couldn’t move Portal to that display", Toast.LENGTH_SHORT).show()
    }

    companion object {
        internal const val EXTRA_DISPLAY_MOVE = "app.polarbear.extra.DISPLAY_MOVE"
        private const val PREF_FLOATING_CONTROLS = "floating_tablet_controls"
        private const val PREF_RELEASE_HINTS = "tablet_release_hints"
        private const val RELEASE_HINTS = 3

        /** Starts Portal's task on [displayId]; Android moves the running activity there. */
        fun moveIntent(context: Context): Intent = Intent.makeMainActivity(
            android.content.ComponentName(context, PortalActivity::class.java),
        )
            // Match the launcher task and deliver to the live GameActivity at its top.
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
            .putExtra(EXTRA_DISPLAY_MOVE, true)

        fun displayOptions(displayId: Int): android.os.Bundle? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            ActivityOptions.makeBasic().setLaunchDisplayId(displayId).toBundle()
        } else null

        fun openOverlaySettings(context: Context): Boolean = try {
            context.startActivity(
                Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION, android.net.Uri.fromParts("package", context.packageName, null))
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
            true
        } catch (error: RuntimeException) {
            Log.w(TAG, "Could not open Android's Display over other apps settings", error)
            false
        }
        private const val PREF_TABLET_EDGE = "tablet_edge"
        private const val TAG = "PortalDisplays"
        private const val MOVE_TIMEOUT_MS = 1500L
    }
}
