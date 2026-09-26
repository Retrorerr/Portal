package app.polarbear.setup.components

// Desktop app icons shown by setup. The artwork is the exact icon each app
// shows inside Portal's Plasma: Breeze for KDE apps, the upstream hicolor
// icon for the rest (res/drawable-nodpi/portal_app_*.png, 144px).
//
// Icons decode off the main thread as soon as setup composes, so the picker
// never decodes on its first expansion. Resolved by name, like ps_noise, so
// no generated R class is involved.

import android.graphics.BitmapFactory
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.State
import androidx.compose.runtime.produceState
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** An app that ships in every Portal install. */
@Immutable
data class IncludedApp(val id: String, val name: String)

val INCLUDED_APPS = listOf(
    IncludedApp("firefox", "Firefox"),
    IncludedApp("dolphin", "Dolphin files"),
    IncludedApp("konsole", "Konsole terminal"),
    IncludedApp("kate", "Kate text editor"),
    IncludedApp("okular", "Okular documents"),
    IncludedApp("gwenview", "Gwenview images"),
    IncludedApp("ark", "Ark archives"),
    IncludedApp("kcalc", "KCalc calculator"),
    IncludedApp("systemsettings", "System Settings"),
)

/** Every setup icon by app id; fills in once decoded (empty until then). */
@Composable
fun rememberAppIcons(ids: List<String>): State<Map<String, ImageBitmap>> {
    val context = LocalContext.current.applicationContext
    return produceState(initialValue = emptyMap(), ids) {
        value = withContext(Dispatchers.IO) {
            val resources = context.resources
            ids.mapNotNull { id ->
                val resId = resources.getIdentifier("portal_app_$id", "drawable", context.packageName)
                if (resId == 0) return@mapNotNull null
                val bitmap = BitmapFactory.decodeResource(resources, resId) ?: return@mapNotNull null
                // Upload ahead of first draw, not during an animation frame.
                bitmap.prepareToDraw()
                id to bitmap.asImageBitmap()
            }.toMap()
        }
    }
}
