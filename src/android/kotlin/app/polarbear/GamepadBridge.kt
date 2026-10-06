package app.polarbear

import android.content.Context
import android.hardware.input.InputManager
import android.os.Build
import android.os.CombinedVibration
import android.os.Handler
import android.os.Looper
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.util.Log
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent

/**
 * Game controllers for the guest. GameActivity only hands touch input to
 * native code, so the activity routes controller key and motion events
 * here first; Rust turns them into evdev events on a pty-backed
 * /dev/input/eventN (see src/android/gamepad.rs) and calls [rumble] back
 * when a game plays force feedback.
 *
 * Controller input reaches the guest only while the desktop is in front;
 * Portal's own screens keep Android's normal controller navigation.
 */
object GamepadBridge {
    private const val TAG = "PortalGamepad"

    /** Matches TOUCH_PAD_ID in src/android/gamepad.rs. */
    const val TOUCH_PAD_ID = -100

    @JvmStatic private external fun nativeKey(deviceId: Int, keyCode: Int, down: Boolean): Boolean
    @JvmStatic private external fun nativeAxes(deviceId: Int, axes: FloatArray)
    @JvmStatic private external fun nativeTouchPad(buttons: Int, axes: FloatArray)
    @JvmStatic private external fun nativeRemoved(deviceId: Int)

    private val AXES = intArrayOf(
        MotionEvent.AXIS_X,
        MotionEvent.AXIS_Y,
        MotionEvent.AXIS_Z,
        MotionEvent.AXIS_RZ,
        MotionEvent.AXIS_LTRIGGER,
        MotionEvent.AXIS_RTRIGGER,
        MotionEvent.AXIS_BRAKE,
        MotionEvent.AXIS_GAS,
        MotionEvent.AXIS_HAT_X,
        MotionEvent.AXIS_HAT_Y,
    )

    private var appContext: Context? = null
    private val known = HashSet<Int>()
    private val axisValues = FloatArray(AXES.size)

    fun attach(context: Context) {
        if (appContext != null) return
        val app = context.applicationContext
        appContext = app
        try {
            val input = app.getSystemService(InputManager::class.java)
            input?.registerInputDeviceListener(
                object : InputManager.InputDeviceListener {
                    override fun onInputDeviceAdded(deviceId: Int) = Unit
                    override fun onInputDeviceChanged(deviceId: Int) = Unit
                    override fun onInputDeviceRemoved(deviceId: Int) {
                        if (known.remove(deviceId)) removed(deviceId)
                    }
                },
                Handler(Looper.getMainLooper()),
            )
        } catch (e: Exception) {
            Log.w(TAG, "controller hotplug listener unavailable", e)
        }
    }

    private fun isController(device: InputDevice?): Boolean {
        if (device == null || device.isVirtual) return false
        val sources = device.sources
        return (sources and InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD ||
            (sources and InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK
    }

    private fun desktopInFront(): Boolean = !ComposeOverlay.isShowing()

    /** Activity.dispatchKeyEvent hook: true when the guest took the key. */
    fun dispatchKey(event: KeyEvent): Boolean {
        if (!desktopInFront()) return false
        val fromPad = (event.source and InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD ||
            (event.source and InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK ||
            ((event.source and InputDevice.SOURCE_DPAD) == InputDevice.SOURCE_DPAD && isController(event.device))
        if (!fromPad || !isController(event.device)) return false
        val down = when (event.action) {
            KeyEvent.ACTION_DOWN -> true
            KeyEvent.ACTION_UP -> false
            else -> return false
        }
        if (down && event.repeatCount > 0) return true
        return try {
            known.add(event.deviceId)
            nativeKey(event.deviceId, event.keyCode, down)
        } catch (e: UnsatisfiedLinkError) {
            false
        }
    }

    /** Activity.dispatchGenericMotionEvent hook for sticks, triggers, hat. */
    fun dispatchMotion(event: MotionEvent): Boolean {
        if (!desktopInFront()) return false
        if ((event.source and InputDevice.SOURCE_JOYSTICK) != InputDevice.SOURCE_JOYSTICK) return false
        if (event.action != MotionEvent.ACTION_MOVE || !isController(event.device)) return false
        // Batched samples are history; the latest one carries the state.
        for (index in AXES.indices) axisValues[index] = event.getAxisValue(AXES[index])
        return try {
            known.add(event.deviceId)
            nativeAxes(event.deviceId, axisValues)
            true
        } catch (e: UnsatisfiedLinkError) {
            false
        }
    }

    /** On-screen controls: the whole pad state, buttons in xpad order. */
    fun touchPad(buttons: Int, axes: FloatArray) {
        try {
            nativeTouchPad(buttons, axes)
        } catch (e: UnsatisfiedLinkError) {
            Log.w(TAG, "touch controller unavailable", e)
        }
    }

    fun removeTouchPad() = removed(TOUCH_PAD_ID)

    private fun removed(deviceId: Int) {
        try {
            nativeRemoved(deviceId)
        } catch (e: UnsatisfiedLinkError) {
            Log.w(TAG, "controller removal not delivered", e)
        }
    }

    /**
     * Native (rumble thread): play force feedback. Strong drives the
     * low-frequency motor, weak the high-frequency one; zero stops.
     */
    @JvmStatic
    fun rumble(deviceId: Int, strong: Int, weak: Int, durationMs: Int) {
        try {
            val amplitudes = intArrayOf(strong * 255 / 0xffff, weak * 255 / 0xffff)
            val length = durationMs.coerceIn(1, 0xffff).toLong()
            if (deviceId == TOUCH_PAD_ID) {
                rumbleVibrator(ownVibrator(), amplitudes.max(), length)
                return
            }
            val device = InputDevice.getDevice(deviceId) ?: return
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                val manager = device.vibratorManager
                val ids = manager.vibratorIds
                if (ids.isEmpty()) return
                if (amplitudes.max() == 0) {
                    manager.cancel()
                    return
                }
                val combined = CombinedVibration.startParallel()
                ids.forEachIndexed { index, id ->
                    val level = amplitudes[minOf(index, 1)].coerceIn(0, 255)
                    if (level > 0) {
                        combined.addVibrator(id, VibrationEffect.createOneShot(length, level))
                    }
                }
                manager.vibrate(combined.combine())
            } else {
                @Suppress("DEPRECATION")
                rumbleVibrator(device.vibrator, amplitudes.max(), length)
            }
        } catch (e: Exception) {
            Log.w(TAG, "rumble failed for device $deviceId", e)
        }
    }

    private fun ownVibrator(): Vibrator? {
        val context = appContext ?: return null
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            context.getSystemService(VibratorManager::class.java)?.defaultVibrator
        } else {
            @Suppress("DEPRECATION")
            context.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
        }
    }

    private fun rumbleVibrator(vibrator: Vibrator?, amplitude: Int, length: Long) {
        if (vibrator == null || !vibrator.hasVibrator()) return
        if (amplitude <= 0) {
            vibrator.cancel()
            return
        }
        val level = if (vibrator.hasAmplitudeControl()) amplitude.coerceIn(1, 255) else VibrationEffect.DEFAULT_AMPLITUDE
        vibrator.vibrate(VibrationEffect.createOneShot(length, level))
    }
}
