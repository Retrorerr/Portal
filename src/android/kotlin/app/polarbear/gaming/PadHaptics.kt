package app.polarbear.gaming

import android.content.Context
import android.os.Build
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager

/**
 * Presses you can feel ([PadFeel]). Buttons click, the d-pad ticks on each
 * new direction, a stick ticks softly as the thumb takes it and again at its
 * rim, and a stick click lands harder. Built from the vibrator's primitives
 * where it has them (crisp on a good motor), else Android's predefined
 * effects. Silent while [quiet] says the game is rumbling: a tick would cut
 * the rumble short. One for the app: both homes of the controller share it.
 */
internal class PadHaptics(context: Context, private val quiet: () -> Boolean) {
    var enabled = true

    private val vibrator: Vibrator? = try {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            context.getSystemService(VibratorManager::class.java)?.defaultVibrator
        } else {
            @Suppress("DEPRECATION")
            context.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
        }
    } catch (_: Exception) {
        null
    }?.takeIf { it.hasVibrator() }

    /** Old motors: a plain pulse per feel, ms; 0 stays silent. */
    private val pulses = longArrayOf(14L, 8L, 6L, 0L, 22L)

    // A VibrationEffect per feel from Android 8; null where that feel stays silent.
    private val effects = arrayOfNulls<Any>(PadFeel.entries.size)

    init {
        val vibrator = vibrator
        if (vibrator != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            for (feel in PadFeel.entries) effects[feel.ordinal] = try { effectFor(vibrator, feel) } catch (_: Exception) { null }
        }
    }

    private fun effectFor(vibrator: Vibrator, feel: PadFeel): Any? {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R &&
            vibrator.areAllPrimitivesSupported(VibrationEffect.Composition.PRIMITIVE_CLICK, VibrationEffect.Composition.PRIMITIVE_TICK)
        ) {
            val low = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S &&
                vibrator.areAllPrimitivesSupported(VibrationEffect.Composition.PRIMITIVE_LOW_TICK)
            val primitive: Int
            val scale: Float
            when (feel) {
                PadFeel.Press -> { primitive = VibrationEffect.Composition.PRIMITIVE_CLICK; scale = 0.7f }
                PadFeel.Step -> { primitive = VibrationEffect.Composition.PRIMITIVE_TICK; scale = 0.75f }
                PadFeel.Grab -> if (low) { primitive = VibrationEffect.Composition.PRIMITIVE_LOW_TICK; scale = 0.8f }
                    else { primitive = VibrationEffect.Composition.PRIMITIVE_TICK; scale = 0.4f }
                PadFeel.Rim -> if (low) { primitive = VibrationEffect.Composition.PRIMITIVE_LOW_TICK; scale = 0.45f }
                    else { primitive = VibrationEffect.Composition.PRIMITIVE_TICK; scale = 0.2f }
                PadFeel.Click -> { primitive = VibrationEffect.Composition.PRIMITIVE_CLICK; scale = 1f }
            }
            return VibrationEffect.startComposition().addPrimitive(primitive, scale).compose()
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            return when (feel) {
                PadFeel.Press -> VibrationEffect.createPredefined(VibrationEffect.EFFECT_CLICK)
                PadFeel.Step, PadFeel.Grab -> VibrationEffect.createPredefined(VibrationEffect.EFFECT_TICK)
                // A buzz at every rim would tire on a plain motor.
                PadFeel.Rim -> null
                PadFeel.Click -> VibrationEffect.createPredefined(VibrationEffect.EFFECT_HEAVY_CLICK)
            }
        }
        val length = pulses[feel.ordinal]
        if (length <= 0L) return null
        val amplitude = if (vibrator.hasAmplitudeControl()) (60 + length * 9).toInt().coerceAtMost(255) else VibrationEffect.DEFAULT_AMPLITUDE
        return VibrationEffect.createOneShot(length, amplitude)
    }

    fun play(feel: PadFeel) {
        if (!enabled) return
        val vibrator = vibrator ?: return
        if (quiet()) return
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                (effects[feel.ordinal] as? VibrationEffect)?.let(vibrator::vibrate)
            } else {
                val length = pulses[feel.ordinal]
                @Suppress("DEPRECATION")
                if (length > 0L) vibrator.vibrate(length)
            }
        } catch (_: Exception) {
        }
    }
}
