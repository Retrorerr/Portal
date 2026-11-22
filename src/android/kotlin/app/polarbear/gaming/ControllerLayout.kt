package app.polarbear.gaming

import android.content.Context
import org.json.JSONObject

/** The floating controller as the player arranged it: [ElementState] per control over [PadLayout]'s defaults. */
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
        const val MIN_OPACITY = 0.15f
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
                    opacity = root.optDouble("opacity", DEFAULT_OPACITY.toDouble()).toFloat().coerceIn(MIN_OPACITY, 1f),
                    scale = root.optDouble("scale", 1.0).toFloat().coerceIn(MIN_SCALE, MAX_SCALE),
                    elements = elements,
                )
            } catch (_: Exception) {
                null
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
