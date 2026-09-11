package app.polarbear.setup.components

// SPIKE-ONLY: frosted Essentials picker. Custom floating surface (not an
// AlertDialog); entrance is fade + subtle scale/translation (~280ms).

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CheckboxDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.ESSENTIAL_APPS
import app.polarbear.setup.PortalDimens
import app.polarbear.setup.PortalPalette
import app.polarbear.setup.formatExtrasDelta

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
        enter = fadeIn(tween(280)) + scaleIn(
            initialScale = 0.96f,
            animationSpec = tween(280),
        ) + slideInVertically(
            initialOffsetY = { 48 },
            animationSpec = tween(280),
        ),
        exit = fadeOut(tween(240)) + scaleOut(
            targetScale = 0.97f,
            animationSpec = tween(240),
        ) + slideOutVertically(
            targetOffsetY = { 32 },
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
                    verticalArrangement = Arrangement.spacedBy(2.dp),
                ) {
                    ESSENTIAL_APPS.forEach { app ->
                        val rowTap = remember { MutableInteractionSource() }
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .clip(RoundedCornerShape(14.dp))
                                .clickable(
                                    interactionSource = rowTap,
                                    indication = null,
                                    onClick = { onToggle(app.id) },
                                )
                                .padding(vertical = 9.dp, horizontal = 4.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Checkbox(
                                checked = app.id in selectedIds,
                                onCheckedChange = { onToggle(app.id) },
                                colors = CheckboxDefaults.colors(
                                    checkedColor = palette.accent,
                                    uncheckedColor = palette.textMuted,
                                    checkmarkColor = Color.White,
                                ),
                            )
                            Column(modifier = Modifier.weight(1f)) {
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
                Spacer(modifier = Modifier.height(12.dp))
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
                    TextButton(onClick = onDismiss) {
                        Text(
                            text = "Done",
                            fontSize = 15.sp,
                            fontWeight = FontWeight.SemiBold,
                            color = palette.accentSoft,
                        )
                    }
                }
            }
        }
    }
}

private fun palettePickerFill(palette: PortalPalette): Color {
    return if (palette.isDark) Color(0xF225282A) else Color(0xF2FDFCF8)
}
