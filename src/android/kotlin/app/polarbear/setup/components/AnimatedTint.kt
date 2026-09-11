package app.polarbear.setup.components

// SPIKE-ONLY: smooth color transitions. (Compose 1.7 hides the legacy
// single-value animateColorAsState entry point, so this uses the supported
// generic animateValueAsState with an explicit Color converter.)

import androidx.compose.animation.core.AnimationVector4D
import androidx.compose.animation.core.TwoWayConverter
import androidx.compose.animation.core.animateValueAsState
import androidx.compose.animation.core.tween
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.ui.graphics.Color

@Composable
fun animatedTint(target: Color, durationMillis: Int = 200): State<Color> {
    return animateValueAsState(
        targetValue = target,
        typeConverter = TwoWayConverter(
            convertToVector = { color ->
                AnimationVector4D(color.red, color.green, color.blue, color.alpha)
            },
            convertFromVector = { vector ->
                Color(vector.v1, vector.v2, vector.v3, vector.v4)
            },
        ),
        animationSpec = tween(durationMillis),
        label = "tint",
    )
}
