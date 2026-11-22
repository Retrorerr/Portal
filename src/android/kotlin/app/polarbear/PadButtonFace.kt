package app.polarbear

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.components.PortalGlyph
import app.polarbear.setup.components.PortalIcon

/**
 * A touchpad button a capsule page grows out of. While the page is out, and
 * until it has closed back, the capsule wears this face in the button's place.
 */
@Immutable
internal data class PadFace(val glyph: PortalGlyph, val label: String, val size: Dp)

/**
 * A round button's glyph and word, inside [modifier]'s shape. The touchpad's
 * buttons and the capsule standing in for one draw the same face, so the
 * capsule hands over to the button without a visible change.
 */
@Composable
internal fun PadButtonFace(glyph: PortalGlyph, label: String, ink: Color, modifier: Modifier = Modifier) {
    Column(
        modifier.padding(6.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(3.dp, Alignment.CenterVertically),
    ) {
        PortalIcon(glyph, ink, size = 20.dp)
        Text(label, color = ink, fontSize = 11.sp, fontWeight = FontWeight.Medium, textAlign = TextAlign.Center,
            lineHeight = 13.sp, maxLines = 2)
    }
}
