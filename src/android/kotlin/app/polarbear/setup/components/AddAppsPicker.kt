package app.polarbear.setup.components

// The optional-app list of the setup Apps stage: category headings and a row
// per app with a checkbox. The heading and the size format are shared with
// the Apps card on the veil.

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathMeasure
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.EssentialApp
import app.polarbear.setup.PortalColors
import app.polarbear.setup.PortalPalette

/** Small-caps group heading inside an app list. */
@Composable
internal fun CategoryLabel(label: String, palette: PortalPalette, modifier: Modifier = Modifier) {
    Text(
        text = label.uppercase(),
        modifier = modifier.padding(start = 14.dp, top = 10.dp, bottom = 2.dp),
        color = palette.textSecondary.copy(alpha = 0.75f),
        fontSize = 10.sp,
        fontWeight = FontWeight.SemiBold,
        letterSpacing = 1.2.sp,
        maxLines = 1,
    )
}

/** One optional app: icon, name, what it is and its size, and a checkbox. */
@Composable
internal fun AppSelectionRow(
    app: EssentialApp,
    icon: ImageBitmap?,
    checked: Boolean,
    onToggle: () -> Unit,
    palette: PortalPalette,
    enabled: Boolean,
    modifier: Modifier = Modifier,
) {
    val fill by animateColorAsState(
        if (checked) palette.accent.copy(alpha = if (palette.isDark) 0.11f else 0.09f) else palette.accent.copy(alpha = 0f),
        tween(240),
        label = "selected row",
    )
    val check by animateFloatAsState(
        if (checked) 1f else 0f,
        if (checked) tween(360, easing = PortalEmphasized) else tween(200, easing = PortalEmphasizedAccelerate),
        label = "check",
    )
    val tap = remember { MutableInteractionSource() }
    Row(
        modifier
            .fillMaxWidth()
            .graphicsLayer { if (!app.available) alpha = 0.55f }
            .clip(RoundedCornerShape(18.dp))
            .background(fill)
            .clickable(
                enabled = enabled,
                interactionSource = tap,
                indication = null,
                role = Role.Checkbox,
                onClick = onToggle,
            )
            .semantics {
                stateDescription = when {
                    !app.available -> app.unavailableReason ?: "Unavailable"
                    checked -> "Selected"
                    else -> "Not selected"
                }
            }
            .padding(start = 12.dp, end = 12.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(30.dp), contentAlignment = Alignment.Center) {
            if (icon != null) {
                Image(bitmap = icon, contentDescription = null, modifier = Modifier.size(30.dp))
            }
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(app.name, color = palette.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
            Text(
                app.unavailableReason ?: "${app.blurb} · ${formatInstalledSize(app.installedMb)}",
                color = palette.textSecondary,
                fontSize = 11.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Spacer(Modifier.width(8.dp))
        Canvas(Modifier.size(20.dp)) {
            val corner = CornerRadius(6.dp.toPx())
            drawRoundRect(palette.textSecondary.copy(alpha = 0.45f), cornerRadius = corner, style = Stroke(1.dp.toPx()))
            // The box fills first, then the tick draws itself stroke by stroke.
            val boxFill = (check * 1.8f).coerceIn(0f, 1f)
            drawRoundRect(PortalColors.Orange, cornerRadius = corner, alpha = boxFill)
            val tick = ((check - 0.2f) / 0.8f).coerceIn(0f, 1f)
            if (tick > 0f) {
                val path = Path().apply {
                    moveTo(size.width * 0.25f, size.height * 0.52f)
                    lineTo(size.width * 0.44f, size.height * 0.70f)
                    lineTo(size.width * 0.76f, size.height * 0.31f)
                }
                val measure = PathMeasure().apply { setPath(path, false) }
                val drawn = Path()
                measure.getSegment(0f, measure.length * tick, drawn, true)
                drawPath(drawn, PortalColors.Charcoal, style = Stroke(2.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round))
            }
        }
    }
}

internal fun formatInstalledSize(mb: Int): String =
    if (mb >= 1000) "%.1f GB".format(mb / 1000f) else "$mb MB"
