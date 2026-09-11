package app.polarbear.setup.components

// SPIKE-ONLY: frosted Essentials picker. Custom floating surface (not an
// AlertDialog) with a Portal-native selection indicator. Entrance rises
// slightly from below (~260ms) to feel connected to the Essentials row.

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.ESSENTIAL_APPS
import app.polarbear.setup.PortalDimens
import app.polarbear.setup.PortalPalette
import app.polarbear.setup.formatExtrasDelta
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically

@Composable
fun EssentialsPicker(
    visible: Boolean,
    selectedIds: Set<String>,
    onToggle: (String) -> Unit,
    onDismiss: () -> Unit,
    palette: PortalPalette,
) {
    AnimatedVisibility(
        visible = visible,
        enter = fadeIn(tween(260)) + scaleIn(
            initialScale = 0.97f,
            animationSpec = tween(260),
        ) + slideInVertically(
            initialOffsetY = { it / 4 + 120 },
            animationSpec = tween(260),
        ),
        exit = fadeOut(tween(240)) + scaleOut(
            targetScale = 0.98f,
            animationSpec = tween(240),
        ) + slideOutVertically(
            targetOffsetY = { it / 4 + 80 },
            animationSpec = tween(240),
        ),
    ) {
        Box(
            modifier = Modifier.fillMaxSize(),
            contentAlignment = Alignment.Center,
        ) {
            val scrimTap = remember { MutableInteractionSource() }
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .background(palette.scrim)
                    .clickable(
                        interactionSource = scrimTap,
                        indication = null,
                        onClick = onDismiss,
                    ),
            )
            Column(
                modifier = Modifier
                    .widthIn(max = PortalDimens.PickerMaxWidth)
                    .fillMaxWidth(0.92f)
                    .shadow(32.dp, RoundedCornerShape(28.dp), ambientColor = palette.surfaceShadow, spotColor = palette.surfaceShadow)
                    .clip(RoundedCornerShape(28.dp))
                    .background(palettePickerFill(palette))
                    .padding(horizontal = 28.dp, vertical = 24.dp),
            ) {
                Text(
                    text = "Essentials",
                    fontSize = 20.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = palette.textPrimary,
                )
                Text(
                    text = "Optional apps",
                    fontSize = 13.sp,
                    color = palette.textMuted,
                )
                Spacer(modifier = Modifier.height(12.dp))
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    ESSENTIAL_APPS.forEach { app ->
                        val checked = app.id in selectedIds
                        val rowTap = remember { MutableInteractionSource() }
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .clip(RoundedCornerShape(16.dp))
                                .clickable(
                                    interactionSource = rowTap,
                                    indication = null,
                                    role = Role.Checkbox,
                                    onClick = { onToggle(app.id) },
                                )
                                .padding(vertical = 12.dp, horizontal = 6.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            PortalCheck(checked = checked, palette = palette)
                            Column(
                                modifier = Modifier
                                    .weight(1f)
                                    .padding(start = 14.dp),
                            ) {
                                Text(
                                    text = app.name,
                                    fontSize = 15.sp,
                                    fontWeight = FontWeight.Medium,
                                    color = palette.textPrimary,
                                )
                                Text(
                                    text = app.blurb,
                                    fontSize = 12.sp,
                                    color = palette.textMuted,
                                )
                            }
                            Text(
                                text = "${app.sizeMb} MB",
                                fontSize = 13.sp,
                                color = palette.textSecondary,
                            )
                        }
                    }
                }
                Spacer(modifier = Modifier.height(14.dp))
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        text = "${selectedIds.size} selected · ${formatExtrasDelta(selectedIds)}",
                        fontSize = 13.sp,
                        color = palette.textSecondary,
                        modifier = Modifier.weight(1f),
                    )
                    PickerDoneAction(onDismiss = onDismiss, palette = palette)
                }
            }
        }
    }
}

@Composable
private fun PortalCheck(checked: Boolean, palette: PortalPalette) {
    val fill by animatedTint(
        target = if (checked) palette.accent else Color.Transparent,
    )
    val border by animatedTint(
        target = if (checked) palette.accent else palette.textMuted.copy(alpha = 0.55f),
    )
    val markAlpha by animateFloatAsState(
        targetValue = if (checked) 1f else 0f,
        animationSpec = tween(180),
        label = "checkMark",
    )
    Box(
        modifier = Modifier
            .size(22.dp)
            .border(1.5.dp, border, CircleShape)
            .background(fill, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        Canvas(
            modifier = Modifier
                .size(12.dp)
                .graphicsLayer { alpha = markAlpha },
        ) {
            val stroke = 2.dp.toPx()
            val path = Path().apply {
                moveTo(size.width * 0.18f, size.height * 0.54f)
                lineTo(size.width * 0.44f, size.height * 0.78f)
                lineTo(size.width * 0.84f, size.height * 0.24f)
            }
            drawPath(path, Color.White, style = Stroke(width = stroke, cap = StrokeCap.Round))
        }
    }
}

@Composable
private fun PickerDoneAction(onDismiss: () -> Unit, palette: PortalPalette) {
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    Text(
        text = "Done",
        fontSize = 15.sp,
        fontWeight = FontWeight.SemiBold,
        color = palette.accentSoft,
        modifier = Modifier
            .graphicsLayer { alpha = if (pressed) 0.55f else 1f }
            .clip(RoundedCornerShape(10.dp))
            .clickable(
                interactionSource = tap,
                indication = null,
                role = Role.Button,
                onClick = onDismiss,
            )
            .padding(horizontal = 12.dp, vertical = 8.dp),
    )
}

private fun palettePickerFill(palette: PortalPalette): Color {
    return if (palette.isDark) Color(0xF225282A) else Color(0xF2FDFCF8)
}
