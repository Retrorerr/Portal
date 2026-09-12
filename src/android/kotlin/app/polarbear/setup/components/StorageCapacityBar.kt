package app.polarbear.setup.components

import android.os.StatFs
import android.util.Log
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.*
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.Alignment
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
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
    Box(
        modifier
            .height(58.dp)
            .semantics(mergeDescendants = true) {
                contentDescription = "$line. Portal footprint is estimated."
            },
    ) {
        Canvas(
            Modifier
                .fillMaxWidth()
                .height(12.dp)
                .align(Alignment.TopStart)
                .offset(y = 20.dp),
        ) {
            val gap = 2.dp.toPx()
            val drawableWidth = (size.width - gap * 2f).coerceAtLeast(0f)
            val usedWidth = drawableWidth * (usedGb / totalGb).toFloat()
            val portalWidth = drawableWidth * (portalGb / totalGb).toFloat()
            val freeWidth = (drawableWidth - usedWidth - portalWidth).coerceAtLeast(0f)
            val radius = androidx.compose.ui.geometry.CornerRadius(size.height / 2f)
            if (usedWidth > 0f) {
                drawRoundRect(
                    palette.textSecondary.copy(alpha = 0.28f),
                    size = Size(usedWidth, size.height),
                    cornerRadius = radius,
                )
            }
            if (portalWidth > 0f) {
                drawRoundRect(
                    PortalColors.Orange,
                    topLeft = Offset(usedWidth + gap, 0f),
                    size = Size(portalWidth, size.height),
                    cornerRadius = radius,
                )
            }
            if (freeWidth > 0f) {
                drawRoundRect(
                    palette.textPrimary.copy(alpha = 0.055f),
                    topLeft = Offset(usedWidth + portalWidth + gap * 2f, 0f),
                    size = Size(freeWidth, size.height),
                    cornerRadius = radius,
                )
            }
        }
        Text(
            line,
            color = if (projected.shortfallBytes > 0) palette.accent else palette.textMuted,
            fontSize = 11.sp,
            lineHeight = 16.sp,
            modifier = Modifier.align(Alignment.BottomStart),
        )
    }
}
