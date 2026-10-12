package app.polarbear.gaming

import android.content.Context
import org.json.JSONObject

/** How a control reads touches and what it draws. */
enum class ControlKind { STICK, DPAD, FACE, BUMPER, TRIGGER, BUTTON }

/** Bits in xpad button order (src/core/gamepad.rs BUTTONS). */
object PadBits {
    const val A = 1 shl 0
    const val B = 1 shl 1
    const val X = 1 shl 2
    const val Y = 1 shl 3
    const val LB = 1 shl 4
    const val RB = 1 shl 5
    const val BACK = 1 shl 6
    const val START = 1 shl 7
    const val GUIDE = 1 shl 8
    const val L3 = 1 shl 9
    const val R3 = 1 shl 10
}

/**
 * Every on-screen control. Sizes are dp at 100%; axes index the pad state
 * laid out like GamepadBridge's (0..3 sticks, 4/5 triggers, 8/9 hat).
 */
enum class Control(
    val key: String,
    val title: String,
    val kind: ControlKind,
    val width: Float,
    val height: Float,
    val bit: Int = 0,
    val axisX: Int = -1,
    val axisY: Int = -1,
) {
    LEFT_STICK("lstick", "Left stick", ControlKind.STICK, 184f, 184f, bit = PadBits.L3, axisX = 0, axisY = 1),
    RIGHT_STICK("rstick", "Right stick", ControlKind.STICK, 172f, 172f, bit = PadBits.R3, axisX = 2, axisY = 3),
    DPAD("dpad", "D-pad", ControlKind.DPAD, 158f, 158f),
    FACE("face", "A B X Y", ControlKind.FACE, 212f, 212f),
    LB("lb", "LB", ControlKind.BUMPER, 112f, 50f, bit = PadBits.LB),
    RB("rb", "RB", ControlKind.BUMPER, 112f, 50f, bit = PadBits.RB),
    LT("lt", "LT", ControlKind.TRIGGER, 112f, 60f, axisX = 4),
    RT("rt", "RT", ControlKind.TRIGGER, 112f, 60f, axisX = 5),
    BACK("back", "View", ControlKind.BUTTON, 50f, 50f, bit = PadBits.BACK),
    GUIDE("guide", "Guide", ControlKind.BUTTON, 56f, 56f, bit = PadBits.GUIDE),
    START("start", "Menu", ControlKind.BUTTON, 50f, 50f, bit = PadBits.START),
    L3("l3", "L3", ControlKind.BUTTON, 50f, 50f, bit = PadBits.L3),
    R3("r3", "R3", ControlKind.BUTTON, 50f, 50f, bit = PadBits.R3);

    companion object {
        fun byKey(key: String): Control? = values().firstOrNull { it.key == key }
    }
}

/**
 * Where one control sits once the user moved, resized or hid it. Positions
 * are the control's centre as a fraction of the screen, so a layout keeps
 * its shape through rotation; NaN means the default spot.
 */
data class ElementState(
    val x: Float = Float.NaN,
    val y: Float = Float.NaN,
    val scale: Float = 1f,
    val visible: Boolean = true,
) {
    val placed: Boolean get() = !x.isNaN() && !y.isNaN()
}

data class ControllerLayout(
    val opacity: Float = DEFAULT_OPACITY,
    val scale: Float = 1f,
    val elements: Map<Control, ElementState> = emptyMap(),
) {
    fun element(control: Control): ElementState = elements[control] ?: ElementState()

    fun with(control: Control, state: ElementState): ControllerLayout =
        copy(elements = elements + (control to state))

    fun toJson(): String {
        val items = JSONObject()
        for ((control, state) in elements) {
            val item = JSONObject()
            if (state.placed) {
                item.put("x", state.x.toDouble())
                item.put("y", state.y.toDouble())
            }
            item.put("s", state.scale.toDouble())
            item.put("show", state.visible)
            items.put(control.key, item)
        }
        return JSONObject()
            .put("v", 1)
            .put("opacity", opacity.toDouble())
            .put("scale", scale.toDouble())
            .put("e", items)
            .toString()
    }

    companion object {
        const val DEFAULT_OPACITY = 0.8f
        const val MIN_SCALE = 0.6f
        const val MAX_SCALE = 1.6f
        const val MIN_ELEMENT_SCALE = 0.5f
        const val MAX_ELEMENT_SCALE = 2f

        fun fromJson(text: String?): ControllerLayout? {
            if (text.isNullOrBlank()) return null
            return try {
                val root = JSONObject(text)
                val items = root.optJSONObject("e")
                val elements = HashMap<Control, ElementState>()
                if (items != null) {
                    for (key in items.keys()) {
                        val control = Control.byKey(key) ?: continue
                        val item = items.optJSONObject(key) ?: continue
                        elements[control] = ElementState(
                            x = if (item.has("x")) item.optDouble("x").toFloat() else Float.NaN,
                            y = if (item.has("y")) item.optDouble("y").toFloat() else Float.NaN,
                            scale = item.optDouble("s", 1.0).toFloat()
                                .coerceIn(MIN_ELEMENT_SCALE, MAX_ELEMENT_SCALE),
                            visible = item.optBoolean("show", true),
                        )
                    }
                }
                ControllerLayout(
                    opacity = root.optDouble("opacity", DEFAULT_OPACITY.toDouble()).toFloat().coerceIn(0.15f, 1f),
                    scale = root.optDouble("scale", 1.0).toFloat().coerceIn(MIN_SCALE, MAX_SCALE),
                    elements = elements,
                )
            } catch (_: Exception) {
                null
            }
        }

        /**
         * Default centre of [control] in dp from the top-left of a
         * [width] x [height] dp screen: a full Xbox pad around the lower
         * corners where thumbs rest on a large tablet, clear of Android's
         * side panel on the right edge.
         */
        fun defaultCenter(control: Control, width: Float, height: Float): Pair<Float, Float> {
            val right = width - 56f
            val mid = width / 2f
            return when (control) {
                Control.LEFT_STICK -> 190f to height - 236f
                Control.DPAD -> 424f to height - 112f
                Control.L3 -> 318f to height - 352f
                Control.LT -> 110f to height - 440f
                Control.LB -> 250f to height - 440f
                Control.FACE -> right - 150f to height - 300f
                Control.RIGHT_STICK -> right - 384f to height - 134f
                Control.R3 -> right - 520f to height - 236f
                Control.RB -> right - 210f to height - 470f
                Control.RT -> right - 70f to height - 470f
                Control.BACK -> mid - 92f to height - 52f
                Control.GUIDE -> mid to height - 52f
                Control.START -> mid + 92f to height - 52f
            }
        }
    }
}

/**
 * Saved layouts: one shared by every game, plus an optional one per Steam
 * app id that replaces it while that game runs.
 */
object LayoutStore {
    private const val PREFS = "portal_touch_controls"
    private const val GLOBAL = "layout"
    private const val APP_PREFIX = "layout_app_"

    private fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun hasAppLayout(context: Context, appId: String?): Boolean =
        appId != null && prefs(context).contains(APP_PREFIX + appId)

    /** The layout in effect: the game's own one, else the shared one. */
    fun load(context: Context, appId: String?): ControllerLayout {
        val prefs = prefs(context)
        if (appId != null) {
            ControllerLayout.fromJson(prefs.getString(APP_PREFIX + appId, null))?.let { return it }
        }
        return ControllerLayout.fromJson(prefs.getString(GLOBAL, null)) ?: ControllerLayout()
    }

    /** Save [layout] where [load] found it: the game's own one if it has one. */
    fun save(context: Context, appId: String?, layout: ControllerLayout) {
        val key = if (hasAppLayout(context, appId)) APP_PREFIX + appId else GLOBAL
        prefs(context).edit().putString(key, layout.toJson()).apply()
    }

    /** Give [appId] its own copy of the current layout, or drop it. */
    fun setAppLayout(context: Context, appId: String, own: Boolean) {
        val prefs = prefs(context)
        if (own) {
            val current = ControllerLayout.fromJson(prefs.getString(GLOBAL, null)) ?: ControllerLayout()
            prefs.edit().putString(APP_PREFIX + appId, current.toJson()).apply()
        } else {
            prefs.edit().remove(APP_PREFIX + appId).apply()
        }
    }
}
