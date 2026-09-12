package app.polarbear.setup.components

import android.os.StatFs
import android.util.Log
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.*
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.Alignment
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.LifecycleOwner
import app.polarbear.setup.*
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.util.Locale
import kotlin.math.roundToInt

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
internal fun StorageCapacityBar(
    capacity: StorageCapacity?,
    selectedIds: Set<String>,
    palette: PortalPalette,
    modifier: Modifier = Modifier,
    phase: SetupPhase = SetupPhase.Configure,
    installProgress: State<Float>? = null,
    hasSelectedApps: Boolean = false,
) {
    if (capacity == null && phase == SetupPhase.Configure) {
        Text("Storage capacity unavailable", modifier = modifier, color = palette.textSecondary, fontSize = 13.sp)
        return
    }

    val projected = capacity?.projection(projectedInstallBytes(selectedIds))
    // The bar and numbers consume the SAME animated value. Free-after is always
    // derived as the remainder; it cannot drift independently of the orange bar.
    val animatedPortalGb by animateFloatAsState((projected?.portalBytes ?: 0L) / 1_000_000_000f,
        tween(420), label = "installedFootprint")
    val portalGb = animatedPortalGb.coerceIn(0f, (capacity?.availableBytes ?: 0L) / 1_000_000_000f)
    val usedGb = (projected?.usedBytes ?: 0L) / 1_000_000_000.0
    val totalGb = (capacity?.totalBytes ?: 1L) / 1_000_000_000.0
    val freeGb = (totalGb - usedGb - portalGb).coerceAtLeast(0.0)
    fun format(value: Double) = String.format(Locale.getDefault(), "%.1f", value)
    val line = "${format(usedGb)} GB used · ${format(portalGb.toDouble())} GB Portal · ${format(freeGb)} GB free after"
    val showInstall = phase != SetupPhase.Configure
    val morph by animateFloatAsState(
        targetValue = if (showInstall) 1f else 0f,
        animationSpec = tween(320),
        label = "capacity to install progress",
    )
    val idleProgress = remember { mutableFloatStateOf(0f) }
    val progressState = installProgress ?: idleProgress
    val componentHeight by animateDpAsState(
        targetValue = if (showInstall) 100.dp else 58.dp,
        animationSpec = tween(360),
        label = "install log room",
    )

    Box(
        modifier = modifier
            .fillMaxWidth()
            .height(componentHeight)
            .semantics(mergeDescendants = true) {
                contentDescription = if (showInstall) {
                    "Installing Portal"
                } else {
                    "$line. Portal footprint is estimated."
                }
            },
    ) {
        Box(
            Modifier
                .fillMaxWidth()
                .height(58.dp)
        ) {
            if (capacity != null && projected != null) {
                val capacityModifier = if (morph == 0f) {
                    Modifier
                } else {
                    Modifier.graphicsLayer { alpha = 1f - morph }
                }
                Canvas(
                    capacityModifier
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
                    modifier = capacityModifier.align(Alignment.BottomStart),
                )
            }
            if (showInstall || morph > 0f) {
                InstallProgressBar(
                    progressState = progressState,
                    palette = palette,
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(58.dp)
                        .graphicsLayer { alpha = morph },
                )
            }
        }
        AnimatedVisibility(
            modifier = Modifier
                .align(Alignment.TopStart)
                .padding(top = 42.dp),
            visible = showInstall,
            enter = fadeIn(tween(durationMillis = 260, delayMillis = 180)) + expandVertically(
                animationSpec = tween(360),
                expandFrom = Alignment.Top,
            ),
        ) {
            InstallLogLines(
                progressState = progressState,
                ready = phase == SetupPhase.Ready,
                hasSelectedApps = hasSelectedApps,
                palette = palette,
            )
        }
    }
}

@Composable
private fun InstallProgressBar(
    progressState: State<Float>,
    palette: PortalPalette,
    modifier: Modifier = Modifier,
) {
    val progress = progressState.value.coerceIn(0f, 1f)
    val percent = (progress * 100f).roundToInt().coerceIn(0, 100)
    Box(modifier = modifier) {
        Text(
            text = "Installing Portal · $percent%",
            color = palette.textPrimary,
            fontSize = 11.sp,
            lineHeight = 16.sp,
            fontWeight = FontWeight.Medium,
            modifier = Modifier.align(Alignment.TopStart),
        )
        Canvas(
            Modifier
                .fillMaxWidth()
                .height(12.dp)
                .align(Alignment.TopStart)
                .offset(y = 20.dp),
        ) {
            val radius = androidx.compose.ui.geometry.CornerRadius(size.height / 2f)
            drawRoundRect(
                color = palette.textPrimary.copy(alpha = 0.075f),
                cornerRadius = radius,
            )
            val progressWidth = size.width * progress
            if (progressWidth > 0f) {
                drawRoundRect(
                    color = PortalColors.Orange,
                    size = Size(progressWidth, size.height),
                    cornerRadius = androidx.compose.ui.geometry.CornerRadius(
                        minOf(size.height / 2f, progressWidth / 2f),
                    ),
                )
            }
        }
    }
}

@Composable
private fun InstallLogLines(
    progressState: State<Float>,
    ready: Boolean,
    hasSelectedApps: Boolean,
    palette: PortalPalette,
) {
    val stages = remember(hasSelectedApps) {
        buildList {
            add("Preparing system…")
            add("Installing Debian base…")
            add("Configuring KDE Plasma…")
            if (hasSelectedApps) add("Installing selected apps…")
            add("Finalizing Portal…")
        }
    }
    val activeIndex by remember(progressState, hasSelectedApps) {
        derivedStateOf { installStageIndex(progressState.value, hasSelectedApps) }
    }
    val lines = remember(stages, activeIndex, ready) {
        val history = if (ready) stages + "Portal is ready" else stages.take(activeIndex + 1)
        history.takeLast(4)
    }
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .height(55.dp),
        verticalArrangement = Arrangement.spacedBy(1.dp),
    ) {
        lines.forEachIndexed { index, line ->
            val age = lines.lastIndex - index
            val current = age == 0
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Canvas(Modifier.size(5.dp)) {
                    if (current) drawCircle(PortalColors.Orange.copy(alpha = 0.8f))
                }
                Spacer(Modifier.width(8.dp))
                Text(
                    text = line,
                    color = if (current) {
                        palette.textPrimary.copy(alpha = 0.78f)
                    } else {
                        palette.textSecondary.copy(
                            alpha = when (age) {
                                1 -> 0.44f
                                2 -> 0.34f
                                else -> 0.25f
                            },
                        )
                    },
                    fontFamily = FontFamily.Monospace,
                    fontSize = 10.sp,
                    lineHeight = 13.sp,
                    maxLines = 1,
                )
            }
        }
    }
}

private fun installStageIndex(progress: Float, hasSelectedApps: Boolean): Int = if (hasSelectedApps) {
    when {
        progress < 0.08f -> 0
        progress < 0.34f -> 1
        progress < 0.60f -> 2
        progress < 0.80f -> 3
        else -> 4
    }
} else {
    when {
        progress < 0.10f -> 0
        progress < 0.40f -> 1
        progress < 0.72f -> 2
        else -> 3
    }
}
