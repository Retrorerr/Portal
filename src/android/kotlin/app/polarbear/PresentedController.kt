package app.polarbear

import android.content.pm.ActivityInfo
import android.content.res.Configuration
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.gaming.Gamepad
import app.polarbear.gaming.GamepadView
import app.polarbear.gaming.PadColors
import app.polarbear.gaming.PadSink
import app.polarbear.setup.AppearanceMode
import app.polarbear.setup.PortalColors
import app.polarbear.setup.PortalPalette
import app.polarbear.setup.PortalTheme
import app.polarbear.setup.components.PortalEmphasized
import app.polarbear.setup.components.PortalEmphasizedAccelerate
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import app.polarbear.setup.components.PortalGlyph
import app.polarbear.setup.components.PortalIcon
import app.polarbear.setup.components.PortalUiDensity
import app.polarbear.setup.resolvePalette

/**
 * The touchpad page's Controller ([PresentedDesktopPad]): the whole screen as
 * Portal's game controller for the desktop ([Gamepad], the same one that
 * floats over the desktop on this screen), under a chip at the top that
 * brings the touchpad back or opens Game settings. The screen turns sideways
 * while it shows, and back after. Its views join [page], above the page's
 * own, the first time it shows. UI thread.
 */
internal class PresentedController(
    private val host: PortalActivity,
    private val page: FrameLayout,
    /** The page's top inset, px. Compose-observable. */
    private val safeTop: () -> Int,
) {
    private var gamepad: Gamepad? = null
    private var screen: GamepadView? = null
    private var chip: ComposeView? = null
    /** Where the chip's settings button is; Game settings opens out of it. */
    private var settingsAt: LayoutCoordinates? = null
    /** What the activity asked for before the controller turned the screen sideways. */
    private var orientationBefore: Int? = null
    private val origin = IntArray(2)

    /** Showing, or on its way in. Compose-observable. */
    private var wanted by mutableStateOf(false)

    /** Comes in over the page; the screen turns sideways first. */
    fun show() {
        val pad = gamepad ?: create()
        wanted = true
        sync(pad)
        screen?.visibility = View.VISIBLE
        chip?.visibility = View.VISIBLE
        pad.show(waitForTurn = turnSideways())
    }

    /** Goes, and the screen may turn back. */
    fun hide() {
        wanted = false
        restoreOrientation()
        val pad = gamepad ?: return
        pad.hide {
            // Shown again meanwhile.
            if (wanted) return@hide
            screen?.visibility = View.GONE
            chip?.visibility = View.GONE
        }
    }

    /** Every touch on it ends now. */
    fun release() {
        gamepad?.release()
    }

    /** The page left the window: nothing is held, and the screen may turn again. */
    fun detach() {
        wanted = false
        restoreOrientation()
        gamepad?.release()
    }

    private fun create(): Gamepad {
        val pad = Gamepad(host, PadSink { buttons, axes -> TouchControls.screenPad(buttons, axes) },
            TouchControls.padHaptics(host), wholeScreen = true)
        pad.colors = systemPalette().padColors()
        val view = pad.screenView()
        page.addView(view, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        gamepad = pad
        screen = view
        // Its own window-sized view would take every touch: wrapped to the
        // chip, a touch beside it falls through to the controller.
        val layer = ComposeView(host).apply { setContent { Layer() } }
        page.addView(layer, FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT,
            Gravity.TOP or Gravity.CENTER_HORIZONTAL,
        ))
        chip = layer
        return pad
    }

    /** Game settings as they are now. */
    private fun sync(pad: Gamepad) {
        pad.userScale = TouchControls.screenScale.value
        pad.analogTriggers = TouchControls.analog.value
        // A Portal page over it (the menu, Game settings) ends every touch and dims it.
        pad.suspended = SessionControls.utilityOpen.value
    }

    /** Before Compose has a palette: the one it will resolve. */
    private fun systemPalette(): PortalPalette {
        val night = host.resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK == Configuration.UI_MODE_NIGHT_YES
        return if (night) PortalColors.dark() else PortalColors.light()
    }

    /** Asks Android for landscape; true while the screen is still upright. */
    private fun turnSideways(): Boolean {
        if (orientationBefore == null) orientationBefore = host.requestedOrientation
        host.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        return page.height > page.width
    }

    private fun restoreOrientation() {
        val before = orientationBefore ?: return
        orientationBefore = null
        host.requestedOrientation = before
    }

    private fun openSettings() {
        val at = settingsAt?.takeIf { it.isAttached }?.boundsInWindow()?.center ?: return
        page.getLocationInWindow(origin)
        TouchControls.openFromPad(UtilityPage.Gaming, at.x - origin[0], at.y - origin[1])
    }

    /** The controller keeps the middle of its top row clear of the chip. */
    private fun placeChip(coordinates: LayoutCoordinates) {
        val view = screen ?: return
        view.getLocationInWindow(origin)
        gamepad?.chipBottom = coordinates.boundsInWindow().bottom - origin[1]
    }

    @Composable
    private fun Layer() {
        PortalTheme {
            PortalUiDensity {
                val palette = resolvePalette(AppearanceMode.System)
                val colors = palette.padColors()
                LaunchedEffect(colors) { gamepad?.colors = colors }
                // Game settings can change while it shows, from the page that opens over it.
                LaunchedEffect(Unit) {
                    snapshotFlow {
                        listOf(TouchControls.screenScale.value, TouchControls.analog.value, SessionControls.utilityOpen.value)
                    }.collect { gamepad?.let(::sync) }
                }
                Chip(palette)
            }
        }
    }

    @Composable
    private fun Chip(palette: PortalPalette) {
        val density = LocalDensity.current
        val top = with(density) { safeTop().toDp() }
        // It steps aside for a page opened over the controller, as the touchpad's buttons do.
        val open = wanted && !SessionControls.utilityOpen.value
        // Read in the layer only: the chip comes and goes without recomposing.
        val shown = animateFloatAsState(
            if (open) 1f else 0f,
            if (open) tween(320, delayMillis = 140, easing = PortalEmphasizedDecelerate)
            else tween(140, easing = PortalEmphasizedAccelerate),
            label = "controller chip",
        )
        val shape = RoundedCornerShape(50)
        Row(
            Modifier.padding(top = top + 10.dp, start = 16.dp, end = 16.dp, bottom = 8.dp)
                .onGloballyPositioned(::placeChip)
                .graphicsLayer {
                    val t = shown.value
                    alpha = t
                    translationY = (1f - t) * -10.dp.toPx()
                    val settle = 0.96f + 0.04f * t
                    scaleX = settle; scaleY = settle
                }
                .clip(shape).background(palette.background).background(palette.surfaceTop)
                .border(1.dp, palette.surfaceBorder, shape)
                .padding(3.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            ChipAction(PortalGlyph.Touchpad, "Touchpad", "Back to the touchpad", palette, open) {
                TouchControls.setScreenController(false)
            }
            Box(Modifier.padding(horizontal = 2.dp).width(1.dp).height(18.dp).background(palette.surfaceBorder))
            ChipAction(PortalGlyph.Options, null, "Controller settings", palette, open, onPlaced = { settingsAt = it }) {
                openSettings()
            }
        }
    }
}

/** One of the controller chip's two: a glyph and, for the touchpad, its word. */
@Composable
private fun ChipAction(
    glyph: PortalGlyph,
    label: String?,
    description: String,
    palette: PortalPalette,
    enabled: Boolean,
    onPlaced: (LayoutCoordinates) -> Unit = {},
    onClick: () -> Unit,
) {
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val scale by animateFloatAsState(if (pressed) 0.92f else 1f, spring(dampingRatio = 0.82f, stiffness = 650f), label = "chip press")
    val fill by animateColorAsState(if (pressed) palette.selectionHighlight else Color.Transparent,
        tween(180, easing = PortalEmphasized), label = "chip fill")
    Row(
        Modifier.height(40.dp)
            .onGloballyPositioned(onPlaced)
            .graphicsLayer { scaleX = scale; scaleY = scale }
            .clip(RoundedCornerShape(50)).background(fill)
            .clickable(interactionSource = interaction, indication = null, enabled = enabled, role = Role.Button,
                onClickLabel = description, onClick = onClick)
            .semantics(mergeDescendants = true) { contentDescription = description }
            .padding(horizontal = if (label != null) 14.dp else 11.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        PortalIcon(glyph, palette.textPrimary, size = 18.dp)
        if (label != null) {
            Spacer(Modifier.width(8.dp))
            Text(label, color = palette.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium, maxLines = 1)
        }
    }
}

/** Portal's palette as the controller paints it: the touchpad page's own buttons. */
private fun PortalPalette.padColors() = PadColors(
    surface = surfaceTop.toArgb(),
    // The outline the touchpad page's controls had: its muted ink, faint.
    edge = textMuted.copy(alpha = (if (isDark) 58f else 64f) / 255f).toArgb(),
    ink = textPrimary.toArgb(),
    muted = textMuted.toArgb(),
    accent = accent.toArgb(),
    accentInk = PortalColors.Charcoal.toArgb(),
    dark = isDark,
)
