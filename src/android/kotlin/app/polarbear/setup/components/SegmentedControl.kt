package app.polarbear.setup.components

// SPIKE-ONLY: sliding segmented control with ONE inset selection surface
// that glides between options (~220ms, position-based, no bounce). Text
// colour cross-fades with the movement.

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.VectorConverter
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
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

@Composable
fun <T> SlidingSegmentedControl(
    options: List<T>,
    selected: T,
    onSelect: (T) -> Unit,
    label: (T) -> String,
    palette: PortalPalette,
    modifier: Modifier = Modifier,
    height: Dp = 48.dp,
) {
    val trackShape = RoundedCornerShape(24.dp)
    val pillShape = RoundedCornerShape(20.dp)
    val inset = 4.dp
    BoxWithConstraints(
        modifier = modifier
            .fillMaxWidth()
            .height(height)
            .clip(trackShape)
            .background(palette.trackFill),
    ) {
        val segmentWidth = (maxWidth - inset * 2) / options.size
        val selectedIndex = options.indexOf(selected).coerceAtLeast(0)
        val pillOffset by animateDpAsState(
            targetValue = inset + segmentWidth * selectedIndex,
            animationSpec = tween(durationMillis = 220, easing = FastOutSlowInEasing),
            label = "segmentSlide",
        )
        Box(
            modifier = Modifier
                .offset(x = pillOffset)
                .width(segmentWidth)
                .padding(vertical = inset)
                .fillMaxHeight()
                .shadow(6.dp, pillShape, ambientColor = Color.Black.copy(alpha = 0.25f), spotColor = Color.Black.copy(alpha = 0.25f))
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
            options.forEach { option ->
                val interaction = remember { MutableInteractionSource() }
                val isSelected = option == selected
                val textColor by animatedTint(
                    target = if (isSelected) palette.selectionText else palette.optionText,
                )
                Box(
                    modifier = Modifier
                        .weight(1f)
                        .fillMaxHeight()
                        .clip(trackShape)
                        .clickable(
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
