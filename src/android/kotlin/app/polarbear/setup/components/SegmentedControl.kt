package app.polarbear.setup.components

// SPIKE-ONLY: sliding segmented control with ONE inset selection surface
// that glides between options. The pill moves like a drop of liquid: its
// leading edge springs ahead and the trailing edge follows, so it stretches
// toward the new option and settles back to size. Both edges are critically
// damped (no bounce). Text colour cross-fades with the movement.

import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.PortalPalette
import androidx.compose.animation.core.spring
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.graphicsLayer

@Composable
fun <T> SlidingSegmentedControl(
    options: List<T>,
    selected: T,
    onSelect: (T) -> Unit,
    label: (T) -> String,
    palette: PortalPalette,
    modifier: Modifier = Modifier,
    height: Dp = 48.dp,
    enabled: Boolean = true,
) {
    val trackShape = RoundedCornerShape(24.dp)
    val pillShape = RoundedCornerShape(20.dp)
    val inset = 4.dp
    BoxWithConstraints(
        modifier = modifier
            .fillMaxWidth()
            .height(height)
            .clip(trackShape)
            .background(palette.trackFill)
            .border(1.dp, palette.surfaceBorder, trackShape),
    ) {
        val segmentWidth = (maxWidth - inset * 2) / options.size
        val selectedIndex = options.indexOf(selected).coerceAtLeast(0)
        val previousIndex = remember { intArrayOf(selectedIndex) }
        val movingRight = remember(selectedIndex) {
            (selectedIndex >= previousIndex[0]).also { previousIndex[0] = selectedIndex }
        }
        val leading = spring<Dp>(dampingRatio = 1f, stiffness = 1_100f)
        val trailing = spring<Dp>(dampingRatio = 1f, stiffness = 300f)
        val targetLeft = inset + segmentWidth * selectedIndex
        val pillLeft by animateDpAsState(
            targetValue = targetLeft,
            animationSpec = if (movingRight) trailing else leading,
            label = "segment left edge",
        )
        val pillRight by animateDpAsState(
            targetValue = targetLeft + segmentWidth,
            animationSpec = if (movingRight) leading else trailing,
            label = "segment right edge",
        )
        val pillWidth = (pillRight - pillLeft).coerceAtLeast(0.dp)
        // Stretching thins the drop a little, as surface tension would.
        val stretch = ((pillWidth - segmentWidth) / segmentWidth).coerceIn(0f, 1f)
        Box(
            modifier = Modifier
                .offset(x = pillLeft)
                .width(pillWidth)
                .graphicsLayer { scaleY = 1f - 0.1f * stretch }
                .padding(vertical = inset)
                .fillMaxHeight()
                .shadow(6.dp, pillShape, ambientColor = palette.pillShadow, spotColor = palette.pillShadow)
                .clip(pillShape)
                .background(palette.selectionPill),
        ) {
            // Faint top highlight for subtle selected-state depth.
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .background(
                        Brush.verticalGradient(
                            colors = listOf(
                                palette.selectionHighlight,
                                Color.Transparent,
                            ),
                        ),
                    ),
            )
        }
        Row(modifier = Modifier.fillMaxSize()) {
            options.forEachIndexed { index, option ->
                val interaction = remember { MutableInteractionSource() }
                val isSelected = option == selected
                // Each label lights exactly as much as the pill covers it, so
                // the text brightens under the glass as it slides past.
                val segmentLeft = inset + segmentWidth * index
                val covered = (
                    (minOf(pillRight, segmentLeft + segmentWidth) - maxOf(pillLeft, segmentLeft)) /
                        segmentWidth
                    ).coerceIn(0f, 1f)
                val textColor = lerp(palette.optionText, palette.selectionText, covered)
                Box(
                    modifier = Modifier
                        .weight(1f)
                        .fillMaxHeight()
                        .clip(trackShape)
                        .clickable(
                            enabled = enabled,
                            interactionSource = interaction,
                            indication = null,
                            role = Role.RadioButton,
                            onClick = { onSelect(option) },
                        ),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        text = label(option),
                        fontSize = 14.sp,
                        fontWeight = if (isSelected) FontWeight.SemiBold else FontWeight.Medium,
                        color = textColor,
                    )                }
            }
        }
    }
}
