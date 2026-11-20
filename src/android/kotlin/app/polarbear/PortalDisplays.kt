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
import app.polarbear.setup.components.keyServiceEnabled

internal enum class TabletEdge { Left, Right }

/**
 * Moves the desktop between Android displays: the running Portal activity
 * where Android allows it, otherwise a [DesktopPresentation] on the monitor
 * with this screen as the touchpad.
 */
internal class PortalDisplays(private val host: PortalActivity) {
    data class Target(
        val id: Int,
        val name: String,
        val resolution: String,
        val current: Boolean,
        /** Shown through a Presentation; the activity stays here as the touchpad. */
        val presented: Boolean = false,
    )

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

    /** Switched on without Android's permission, the setup for it grows out of [origin]. */
    fun setFloatingControls(enabled: Boolean, origin: androidx.compose.ui.geometry.Rect? = null) {
        floatingWanted = enabled
        preferences.edit().putBoolean(PREF_FLOATING_CONTROLS, enabled).apply()
        if (enabled && !overlayAllowed && !setUp(DisplayPermission.Overlay, origin)) openOverlaySettings(host)
        DesktopControlActivity.displayChanged()
    }

    /** UI thread, on resume: Android's permission page may have changed the answer. */
    fun refreshFloatingControls() {
        overlayAllowed = canDrawOverlays()
    }

    private fun canDrawOverlays() = Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && Settings.canDrawOverlays(host)

    /**
     * What the desktop on a monitor works better with and Android doesn't allow
     * yet, leaving out what the user put off unless [all].
     */
    fun missingPermissions(all: Boolean = false): List<DisplayPermission> = DisplayPermission.entries.filter {
        it.applies && !granted(it) && (all || !preferences.getBoolean(putOffKey(it), false)) &&
            // Floating controls the user switched off need no permission.
            (it != DisplayPermission.Overlay || floatingWanted)
    }

    fun granted(permission: DisplayPermission): Boolean = when (permission) {
        DisplayPermission.Overlay -> canDrawOverlays()
        // Switched on is enough: Android may still be binding the service.
        DisplayPermission.Accessibility -> keyServiceEnabled(host)
    }

    fun putOff(permission: DisplayPermission) {
        preferences.edit().putBoolean(putOffKey(permission), true).apply()
    }

    private fun putOffKey(permission: DisplayPermission) = "display_setup_put_off_${permission.name.lowercase()}"

    /**
     * Moves the desktop to [target]; to a monitor, first through the setup for
     * what it lacks there, growing out of [origin] (window pixels).
     */
    fun choose(target: Target, origin: androidx.compose.ui.geometry.Rect?, moved: () -> Unit) {
        if (!target.current && target.id != Display.DEFAULT_DISPLAY) {
            val preview = DisplaySetup.previewNext.also { DisplaySetup.previewNext = false }
            val steps = if (preview) DisplaySetup.everyStep() else missingPermissions()
            if (DisplaySetup.show(host, target, steps, origin, moved, preview)) return
        }
        if (moveTo(target.id)) moved()
    }

    /** The setup's step for [permission] alone, growing out of [origin]; the desktop stays where it is. */
    fun setUp(permission: DisplayPermission, origin: androidx.compose.ui.geometry.Rect?): Boolean {
        val monitor = targets().firstOrNull { it.id != Display.DEFAULT_DISPLAY } ?: return false
        return DisplaySetup.show(host, monitor, listOf(permission), origin, moved = null)
    }

    /** The release keys are shown on the first few grabs only. */
    fun takeReleaseHint(): Boolean = takeHint(PREF_RELEASE_HINTS, RELEASE_HINTS)

    /** The touchpad's gesture guide is shown the first few times the phone becomes one. */
    fun takeTouchpadHint(): Boolean = takeHint(PREF_TOUCHPAD_HINTS, TOUCHPAD_HINTS)

    private fun takeHint(key: String, limit: Int): Boolean {
        val shown = preferences.getInt(key, 0)
        if (shown >= limit) return false
        preferences.edit().putInt(key, shown + 1).apply()
        return true
    }

    private val activitiesOnOtherDisplays = host.packageManager.hasSystemFeature(
        PackageManager.FEATURE_ACTIVITIES_ON_SECONDARY_DISPLAYS,
    )

    // Any app may present on a monitor, with or without activities there.
    val supported = Build.VERSION.SDK_INT >= Build.VERSION_CODES.O

    fun targets(): List<Target> {
        val activityId = host.currentDisplay()?.displayId
        val currentId = host.presentedDisplay?.displayId ?: activityId
        val intent = launchIntent()
        val displays = displayManager.displays.filter {
            it.displayId == activityId || it.displayId == currentId || canMoveTo(it, intent) || canPresentOn(it)
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
            val presented = id != activityId && !canMoveTo(display, intent) && canPresentOn(display)
            Target(id, label, "$width × $height", id == currentId, presented)
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
        // A presented desktop first comes back to the activity's own screen.
        host.presentedDisplay?.let { presented ->
            if (presented.displayId == displayId) return true
            host.endPresentation()
            if (displayId == host.currentDisplay()?.displayId) {
                // From the floating controls Portal may be behind other apps.
                bringPortalForward()
                return true
            }
        }
        val intent = launchIntent()
        if (display != null && display.displayId != host.currentDisplay()?.displayId &&
            !canMoveTo(display, intent) && canPresentOn(display)
        ) {
            if (host.presentDesktop(display)) return true
            showMoveError()
            return false
        }
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

    /** Shows Portal on its own screen when other apps cover it, as Return to phone and the touchpad need. */
    fun bringPortalForward() {
        if (host.onScreen || host.isFinishing || host.isDestroyed) return
        try {
            host.startActivity(launchIntent(), displayOptions(host.currentDisplay()?.displayId ?: Display.DEFAULT_DISPLAY))
        } catch (error: RuntimeException) {
            Log.w(TAG, "Could not bring Portal forward", error)
        }
    }

    private fun launchIntent(): Intent = moveIntent(host)

    private fun canMoveTo(display: Display, intent: Intent): Boolean {
        // Android may destroy a task when the owner of a private display removes it.
        if (!display.isValid || display.flags and Display.FLAG_PRIVATE != 0) return false
        if (!activitiesOnOtherDisplays && display.displayId != Display.DEFAULT_DISPLAY) return false
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return true
        return activityManager.isActivityStartAllowedOnDisplay(host, display.displayId, intent)
    }

    /**
     * A monitor apps may show a Presentation on. Phones that mirror a monitor
     * and keep activities off it (OxygenOS without desktop mode) still allow one.
     */
    private fun canPresentOn(display: Display): Boolean =
        display.isValid && display.displayId != Display.DEFAULT_DISPLAY &&
            display.flags and Display.FLAG_PRESENTATION != 0 && display.flags and Display.FLAG_PRIVATE == 0

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
        internal const val EXTRA_RETURN = "app.polarbear.extra.RETURN_TO_DEVICE"
        private const val PREF_FLOATING_CONTROLS = "floating_tablet_controls"
        private const val PREF_RELEASE_HINTS = "tablet_release_hints"
        private const val RELEASE_HINTS = 3
        private const val PREF_TOUCHPAD_HINTS = "touchpad_gesture_hints"
        private const val TOUCHPAD_HINTS = 4

        /** Starts Portal's task on [displayId]; Android moves the running activity there. */
        fun moveIntent(context: Context): Intent = Intent.makeMainActivity(
            android.content.ComponentName(context, PortalActivity::class.java),
        )
            // Match the launcher task and deliver to the live GameActivity at its top.
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
            .putExtra(EXTRA_DISPLAY_MOVE, true)

        /** Brings the desktop back to the device's own screen, moved or presented; start it there. */
        fun returnIntent(context: Context): Intent = moveIntent(context).putExtra(EXTRA_RETURN, true)

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
