package app.polarbear.setup.components

import android.os.StatFs
import android.util.Log
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.LifecycleOwner
import app.polarbear.setup.*
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.util.Locale

/** One snapshot of the actual private app/data volume; refresh on resume, not per frame. */
@Composable
fun rememberStorageCapacity(): StorageCapacity? {
    val context = LocalContext.current
    var refresh by remember { mutableIntStateOf(0) }
    DisposableEffect(context) {
        val lifecycle = (context as LifecycleOwner).lifecycle
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) refresh++
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    return produceState<StorageCapacity?>(null, context, refresh) {
        value = withContext(Dispatchers.IO) {
            runCatching {
                val fs = StatFs(context.filesDir.absolutePath)
                StorageCapacity(fs.totalBytes, fs.availableBytes).also {
                    Log.i("PortalSetupStorage", "app-data total=${it.totalBytes} available=${it.availableBytes}")
                }
            }.getOrElse {
                Log.w("PortalSetupStorage", "Capacity unavailable", it)
                null
            }
        }
    }.value
}

@Composable
fun StorageCapacityBar(capacity: StorageCapacity?, selectedIds: Set<String>, palette: PortalPalette, modifier: Modifier = Modifier) {
    if (capacity == null) {
        Text("Storage capacity unavailable", modifier = modifier, color = palette.textSecondary, fontSize = 13.sp)
        return
    }
    val projected = capacity.projection(projectedInstallBytes(selectedIds))
    // The bar and numbers consume the SAME animated value. Free-after is always
    // derived as the remainder; it cannot drift independently of the orange bar.
    val animatedPortalGb by animateFloatAsState(projected.portalBytes / 1_000_000_000f,
        tween(420), label = "installedFootprint")
    val portalGb = animatedPortalGb.coerceIn(0f, capacity.availableBytes / 1_000_000_000f)
    val usedGb = projected.usedBytes / 1_000_000_000.0
    val totalGb = capacity.totalBytes / 1_000_000_000.0
    val freeGb = (totalGb - usedGb - portalGb).coerceAtLeast(0.0)
    fun format(value: Double) = String.format(Locale.getDefault(), "%.1f", value)
    val line = "${format(usedGb)} GB used · ${format(portalGb.toDouble())} GB Portal · ${format(freeGb)} GB free after"
    Column(modifier.semantics(mergeDescendants = true) { contentDescription = "$line. Portal footprint is estimated." }) {
        Canvas(Modifier.fillMaxWidth().height(9.dp).clip(RoundedCornerShape(5.dp)).background(palette.trackFill)) {
            val usedWidth = size.width * (usedGb / totalGb).toFloat()
            val portalWidth = size.width * (portalGb / totalGb).toFloat()
            drawRect(palette.textSecondary.copy(alpha = 0.28f), size = Size(usedWidth, size.height))
            // No minimum width: the segment remains proportional on large volumes.
            // Its saturated fill and fine ivory top glint give it contrast instead.
            drawRect(PortalColors.Orange, topLeft = Offset(usedWidth, 0f), size = Size(portalWidth, size.height))
            drawLine(PortalColors.Ivory.copy(alpha = 0.65f), Offset(usedWidth, 0.7.dp.toPx()),
                Offset(usedWidth + portalWidth, 0.7.dp.toPx()), strokeWidth = 1.dp.toPx(), cap = StrokeCap.Butt)
        }
        Spacer(Modifier.height(10.dp))
        Text(line, color = palette.textSecondary, fontSize = 12.sp, lineHeight = 18.sp)
        Text(if (projected.shortfallBytes > 0) "Not enough space for this selection" else "Portal footprint estimated",
            color = if (projected.shortfallBytes > 0) palette.accent else palette.textMuted,
            fontSize = 11.sp, modifier = Modifier.padding(top = 3.dp))
    }
}
