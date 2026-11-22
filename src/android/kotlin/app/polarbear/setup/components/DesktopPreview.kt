package app.polarbear.setup.components

// A small picture of the desktop setup installs: Plasma's floating panel with
// apps every install has, a file manager and a browser over a Portal
// wallpaper. It follows the look being chosen (dark or light, and the
// interface size) as it changes, so a choice is seen rather than described.
// It is drawn rather than a screenshot, so it stays sharp at any size.

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
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
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.RoundRect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.APERTURE_PATH
import app.polarbear.setup.PortalColors
import app.polarbear.setup.PortalPalette
import app.polarbear.setup.THRESHOLD_PATH
import kotlin.math.floor
import kotlin.math.roundToInt
import androidx.compose.foundation.layout.widthIn

/** The panel's launchers, as Portal's Plasma pins them. */
private val PANEL_APPS = listOf("firefox", "dolphin", "konsole", "kate", "systemsettings")

/** A desktop's proportions, near enough to every tablet's and phone's on its side. */
internal const val DESKTOP_ASPECT = 16f / 10f

internal val DesktopPreviewShape = RoundedCornerShape(6)

/**
 * The desktop as it will look. [interfaceScale] is the interface size's
 * multiplier (0.85 to 1.15): the panel, title bars and icons grow with it.
 * [windows] false shows a desktop nobody has used yet.
 */
@Composable
internal fun DesktopPreview(
    dark: Boolean,
    interfaceScale: Float,
    icons: Map<String, ImageBitmap>,
    palette: PortalPalette,
    modifier: Modifier = Modifier,
    windows: Boolean = true,
) {
    val darkness by animateFloatAsState(
        targetValue = if (dark) 1f else 0f,
        animationSpec = tween(560, easing = PortalEmphasized),
        label = "desktop preview theme",
    )
    val scale by animateFloatAsState(
        targetValue = interfaceScale,
        animationSpec = spring(dampingRatio = 0.86f, stiffness = 240f),
        label = "desktop preview scale",
    )
    val mark = remember { PathParser().parsePathString(APERTURE_PATH).toPath() }
    val threshold = remember { PathParser().parsePathString(THRESHOLD_PATH).toPath() }
    Canvas(
        modifier
            .aspectRatio(DESKTOP_ASPECT)
            .clip(DesktopPreviewShape)
            .border(1.dp, palette.surfaceBorder, DesktopPreviewShape)
            .semantics {
                contentDescription = "Preview of the Plasma desktop in its ${if (dark) "dark" else "light"} look"
            },
    ) {
        val ink = DesktopInk(darkness)
        val u = size.width / 100f
        drawWallpaper(ink, mark, threshold)
        val panelHeight = 3.6f * u * scale
        val margin = 0.75f * u * scale
        val panelTop = size.height - margin - panelHeight
        if (windows) {
            // Windows keep most of their size as the interface grows, as apps do.
            val grow = 1f + (scale - 1f) * 0.7f
            val files = Rect(
                offset = Offset(size.width * 0.055f, size.height * 0.075f),
                size = Size(size.width * 0.50f * grow, size.height * 0.58f * grow),
            )
            drawShadow(files, 0.9f * u * scale, u, ink.shadow * 0.7f)
            drawFileManager(files, ink, icons, u, scale)
            val browserSize = Size(size.width * 0.60f * grow, size.height * 0.60f * grow)
            val browser = Rect(
                offset = Offset(
                    (size.width * 0.345f - (grow - 1f) * size.width * 0.16f)
                        .coerceAtMost(size.width - browserSize.width - size.width * 0.03f),
                    (size.height * 0.215f - (grow - 1f) * size.height * 0.10f)
                        .coerceAtMost(panelTop - browserSize.height - size.height * 0.035f),
                ),
                size = browserSize,
            )
            drawShadow(browser, 0.9f * u * scale, u, ink.shadow)
            drawBrowser(browser, ink, icons, u, scale)
        }
        drawPanel(
            Rect(margin, panelTop, size.width - margin, panelTop + panelHeight),
            ink, icons, u, scale, palette.accent, activeFirst = windows,
        )
    }
}

/**
 * Two desktops as the Workspaces pill draws them, offset panes: the one
 * Portal came from, behind, as it was left, and the new one in front, fresh.
 */
@Composable
internal fun WorkspacePair(
    newName: String,
    currentName: String,
    dark: Boolean,
    interfaceScale: Float,
    icons: Map<String, ImageBitmap>,
    palette: PortalPalette,
    modifier: Modifier = Modifier,
    // Applied to the new desktop alone, which carries on into the next stage.
    frontModifier: Modifier = Modifier,
) {
    Box(modifier.aspectRatio(DESKTOP_ASPECT / 1.16f)) {
        Box(
            Modifier
                .fillMaxWidth(0.84f)
                .align(Alignment.TopEnd)
                .graphicsLayer { alpha = 0.5f },
        ) {
            DesktopPreview(dark, interfaceScale, icons, palette, Modifier.fillMaxWidth())
            NameTag(currentName, Modifier.align(Alignment.TopEnd))
        }
        Box(
            Modifier
                .fillMaxWidth(0.84f)
                .align(Alignment.BottomStart)
                .then(frontModifier),
        ) {
            DesktopPreview(dark, interfaceScale, icons, palette, Modifier.fillMaxWidth(), windows = false)
            Box(
                Modifier
                    .matchParentSize()
                    .border(1.5.dp, palette.accent.copy(alpha = 0.85f), DesktopPreviewShape),
            )
            NameTag(newName, Modifier.align(Alignment.TopStart))
        }
    }
}

/** A workspace's name on its desktop, like the Open chip on the veil's tiles. */
@Composable
private fun NameTag(name: String, modifier: Modifier = Modifier) {
    Row(
        modifier
            .padding(8.dp)
            .clip(RoundedCornerShape(50))
            .background(Color.Black.copy(alpha = 0.5f))
            .padding(horizontal = 9.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = name,
            modifier = Modifier.widthIn(max = 160.dp),
            fontSize = 11.sp,
            fontWeight = FontWeight.SemiBold,
            color = Color.White,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** Breeze's colours, light to dark by [d]. */
private class DesktopInk(private val d: Float) {
    private fun mix(light: Long, dark: Long) = lerp(Color(light), Color(dark), d)
    val wallTop = mix(0xFFF2E9DA, 0xFF1F2330)
    val wallBottom = mix(0xFFE8DAC3, 0xFF131518)
    val warmGlow = PortalColors.Orange.copy(alpha = 0.30f - 0.08f * d)
    val coolGlow = mix(0xFFC2D3EA, 0xFF34507E).copy(alpha = 0.60f - 0.26f * d)
    val markInk = lerp(PortalColors.Charcoal.copy(alpha = 0.07f), PortalColors.Ivory.copy(alpha = 0.06f), d)
    val markThreshold = PortalColors.Orange.copy(alpha = 0.16f)
    val window = mix(0xFFEFF0F1, 0xFF202326)
    val titlebar = mix(0xFFDEE0E2, 0xFF2A2E32)
    val view = mix(0xFFFFFFFF, 0xFF1B1E20)
    val text = mix(0xFF232627, 0xFFFCFCFC)
    val outline = Color.Black.copy(alpha = 0.10f + 0.30f * d)
    val shadow = 0.55f + 0.45f * d
    val selection = Color(0xFF3DAEE9).copy(alpha = 0.22f + 0.08f * d)
    val folder = Color(0xFF48A7E2)
    val folderTab = Color(0xFF2B8BCB)
    val panel = mix(0xFFF2F3F4, 0xFF1D2023).copy(alpha = 0.95f)
    val tabStrip = mix(0xFFE8E8EC, 0xFF1C1B22)
    val toolbar = mix(0xFFF9F9FB, 0xFF2B2A33)
    val urlbar = mix(0xFFEDEDF1, 0xFF1C1B22)
    val page = mix(0xFFFFFFFF, 0xFF1C1B22)
    val card = mix(0xFFF3F1EE, 0xFF26252D)
}

private fun DrawScope.drawWallpaper(ink: DesktopInk, mark: Path, threshold: Path) {
    drawRect(Brush.verticalGradient(listOf(ink.wallTop, ink.wallBottom)))
    val warm = Offset(size.width * 0.10f, size.height * 1.08f)
    drawCircle(
        Brush.radialGradient(listOf(ink.warmGlow, Color.Transparent), center = warm, radius = size.width * 0.80f),
        radius = size.width * 0.80f,
        center = warm,
    )
    val cool = Offset(size.width * 0.94f, -size.height * 0.12f)
    drawCircle(
        Brush.radialGradient(listOf(ink.coolGlow, Color.Transparent), center = cool, radius = size.width * 0.72f),
        radius = size.width * 0.72f,
        center = cool,
    )
    // The aperture, large and faint, as Portal's wallpaper carries it.
    val height = size.height * 0.56f
    val scale = height / 514f
    withTransform({
        translate(size.width * 0.5f - 345f * scale / 2f, size.height * 0.42f - height / 2f)
        scale(scale, scale, Offset.Zero)
    }) {
        drawPath(mark, ink.markInk, style = Stroke(54f))
        drawPath(threshold, ink.markThreshold, style = Stroke(54f))
    }
}

/** Breeze's soft shadow, as a few widening layers. */
private fun DrawScope.drawShadow(rect: Rect, radius: Float, u: Float, strength: Float) {
    for (i in 1..5) {
        val spread = i * 0.55f * u
        drawRoundRect(
            color = Color.Black.copy(alpha = 0.045f * strength * (6 - i) / 5f),
            topLeft = Offset(rect.left - spread, rect.top - spread * 0.6f + 0.5f * u),
            size = Size(rect.width + spread * 2f, rect.height + spread * 2f),
            cornerRadius = CornerRadius(radius + spread),
        )
    }
}

private fun DrawScope.windowFrame(rect: Rect, radius: Float, block: DrawScope.() -> Unit) {
    val path = Path().apply { addRoundRect(RoundRect(rect, CornerRadius(radius))) }
    clipPath(path) { block() }
}

private fun DrawScope.bar(x: Float, y: Float, width: Float, height: Float, color: Color) {
    drawRoundRect(color, Offset(x, y - height / 2f), Size(width, height), CornerRadius(height / 2f))
}

private fun DrawScope.icon(icons: Map<String, ImageBitmap>, id: String, center: Offset, side: Float, fallback: Color) {
    val image = icons[id]
    val px = side.roundToInt().coerceAtLeast(1)
    if (image == null) {
        drawRoundRect(fallback, Offset(center.x - side / 2f, center.y - side / 2f), Size(side, side), CornerRadius(side * 0.22f))
        return
    }
    drawImage(
        image = image,
        dstOffset = IntOffset((center.x - side / 2f).roundToInt(), (center.y - side / 2f).roundToInt()),
        dstSize = IntSize(px, px),
        filterQuality = FilterQuality.Medium,
    )
}

/** Title bar buttons: minimise, maximise, close. */
private fun DrawScope.windowButtons(right: Float, centerY: Float, u: Float, s: Float, ink: DesktopInk) {
    val r = 0.36f * u * s
    for (i in 0..2) {
        drawCircle(ink.text.copy(alpha = if (i == 2) 0.55f else 0.32f), r, Offset(right - i * 1.45f * u * s, centerY))
    }
}

/** Dolphin: places on the left, a few folders on the right. */
private fun DrawScope.drawFileManager(rect: Rect, ink: DesktopInk, icons: Map<String, ImageBitmap>, u: Float, s: Float) {
    val radius = 0.9f * u * s
    windowFrame(rect, radius) {
        drawRect(ink.window, rect.topLeft, rect.size)
        val titleHeight = 2.5f * u * s
        drawRect(ink.titlebar, rect.topLeft, Size(rect.width, titleHeight))
        icon(icons, "dolphin", Offset(rect.left + 1.4f * u * s, rect.top + titleHeight / 2f), 1.5f * u * s, ink.folder)
        bar(rect.center.x - 4.5f * u * s, rect.top + titleHeight / 2f, 9f * u * s, 0.5f * u * s, ink.text.copy(alpha = 0.45f))
        windowButtons(rect.right - 1.2f * u * s, rect.top + titleHeight / 2f, u, s, ink)
        // Toolbar: back, forward and the path.
        val toolbarY = rect.top + titleHeight + 1.3f * u * s
        drawCircle(ink.text.copy(alpha = 0.35f), 0.4f * u * s, Offset(rect.left + 1.4f * u * s, toolbarY))
        drawCircle(ink.text.copy(alpha = 0.2f), 0.4f * u * s, Offset(rect.left + 2.7f * u * s, toolbarY))
        drawRoundRect(
            ink.view.copy(alpha = 0.85f),
            Offset(rect.left + 4.2f * u * s, toolbarY - 0.75f * u * s),
            Size(rect.width * 0.42f, 1.5f * u * s),
            CornerRadius(0.4f * u * s),
        )
        bar(rect.left + 5.0f * u * s, toolbarY, 5.5f * u * s, 0.42f * u * s, ink.text.copy(alpha = 0.4f))
        val bodyTop = rect.top + titleHeight + 2.6f * u * s
        val sidebar = rect.width * 0.27f
        // Places.
        for (row in 0 until 6) {
            val y = bodyTop + 1.4f * u * s + row * 2.2f * u * s
            if (y > rect.bottom - u * s) break
            if (row == 0) {
                drawRoundRect(
                    ink.selection,
                    Offset(rect.left + 0.6f * u * s, y - 0.95f * u * s),
                    Size(sidebar - 1.2f * u * s, 1.9f * u * s),
                    CornerRadius(0.35f * u * s),
                )
            }
            drawRoundRect(
                if (row < 4) ink.folder else ink.text.copy(alpha = 0.35f),
                Offset(rect.left + 1.3f * u * s, y - 0.5f * u * s),
                Size(1.0f * u * s, 1.0f * u * s),
                CornerRadius(0.22f * u * s),
            )
            bar(rect.left + 2.9f * u * s, y, (3.4f + (row * 7 % 4)) * u * s, 0.45f * u * s, ink.text.copy(alpha = 0.42f))
        }
        // The folder view.
        val view = Rect(rect.left + sidebar, bodyTop, rect.right, rect.bottom)
        drawRect(ink.view, view.topLeft, view.size)
        val side = 4.2f * u * s
        val gap = 2.4f * u * s
        val columns = floor((view.width - gap) / (side + gap)).toInt().coerceAtLeast(1)
        val start = view.left + (view.width - columns * side - (columns - 1) * gap) / 2f
        var row = 0
        while (true) {
            val y = view.top + 1.6f * u * s + row * (side + 2.6f * u * s)
            if (y + side + 1.4f * u * s > view.bottom) break
            for (column in 0 until columns) {
                val x = start + column * (side + gap)
                drawRoundRect(ink.folderTab, Offset(x, y + side * 0.06f), Size(side * 0.46f, side * 0.24f), CornerRadius(side * 0.08f))
                drawRoundRect(ink.folder, Offset(x, y + side * 0.18f), Size(side, side * 0.70f), CornerRadius(side * 0.1f))
                bar(x + side * 0.12f, y + side + 0.75f * u * s, side * 0.76f, 0.42f * u * s, ink.text.copy(alpha = 0.38f))
            }
            row++
        }
    }
    drawRoundRect(ink.outline, rect.topLeft, rect.size, CornerRadius(radius), style = Stroke(0.12f * u))
}

/** Firefox: one tab, the address bar and a page. */
private fun DrawScope.drawBrowser(rect: Rect, ink: DesktopInk, icons: Map<String, ImageBitmap>, u: Float, s: Float) {
    val radius = 0.9f * u * s
    windowFrame(rect, radius) {
        drawRect(ink.tabStrip, rect.topLeft, rect.size)
        val stripHeight = 2.7f * u * s
        val tab = Rect(Offset(rect.left + 1.0f * u * s, rect.top + 0.55f * u * s), Size(15f * u * s, stripHeight - 0.55f * u * s))
        drawRoundRect(ink.toolbar, tab.topLeft, Size(tab.width, tab.height + u), CornerRadius(0.6f * u * s))
        icon(icons, "firefox", Offset(tab.left + 1.2f * u * s, tab.center.y), 1.25f * u * s, PortalColors.Orange)
        bar(tab.left + 2.4f * u * s, tab.center.y, 7.5f * u * s, 0.45f * u * s, ink.text.copy(alpha = 0.55f))
        // New tab.
        val plus = Offset(tab.right + 1.3f * u * s, tab.center.y)
        val arm = 0.42f * u * s
        drawLine(ink.text.copy(alpha = 0.45f), Offset(plus.x - arm, plus.y), Offset(plus.x + arm, plus.y), 0.16f * u * s, StrokeCap.Round)
        drawLine(ink.text.copy(alpha = 0.45f), Offset(plus.x, plus.y - arm), Offset(plus.x, plus.y + arm), 0.16f * u * s, StrokeCap.Round)
        windowButtons(rect.right - 1.2f * u * s, rect.top + stripHeight / 2f, u, s, ink)
        // Toolbar.
        val toolbarTop = rect.top + stripHeight
        val toolbarHeight = 2.8f * u * s
        drawRect(ink.toolbar, Offset(rect.left, toolbarTop), Size(rect.width, toolbarHeight))
        val toolbarY = toolbarTop + toolbarHeight / 2f
        drawCircle(ink.text.copy(alpha = 0.4f), 0.38f * u * s, Offset(rect.left + 1.4f * u * s, toolbarY))
        drawCircle(ink.text.copy(alpha = 0.22f), 0.38f * u * s, Offset(rect.left + 2.7f * u * s, toolbarY))
        val url = Rect(
            Offset(rect.left + 4.4f * u * s, toolbarY - 0.9f * u * s),
            Size(rect.width - 8.8f * u * s, 1.8f * u * s),
        )
        drawRoundRect(ink.urlbar, url.topLeft, url.size, CornerRadius(0.5f * u * s))
        drawCircle(ink.text.copy(alpha = 0.35f), 0.3f * u * s, Offset(url.left + 0.9f * u * s, toolbarY))
        bar(url.left + 1.8f * u * s, toolbarY, url.width * 0.34f, 0.42f * u * s, ink.text.copy(alpha = 0.42f))
        for (i in 0..2) {
            bar(rect.right - 2.0f * u * s, toolbarY - 0.45f * u * s + i * 0.45f * u * s, 0.9f * u * s, 0.14f * u * s, ink.text.copy(alpha = 0.45f))
        }
        // The page.
        val page = Rect(rect.left, toolbarTop + toolbarHeight, rect.right, rect.bottom)
        drawRect(ink.page, page.topLeft, page.size)
        val inset = 2.2f * u
        val hero = Rect(Offset(page.left + inset, page.top + inset), Size(page.width - inset * 2f, page.height * 0.40f))
        drawRoundRect(
            Brush.linearGradient(
                listOf(Color(0xFFF07949), Color(0xFFC4467E)),
                start = hero.topLeft,
                end = hero.bottomRight,
            ),
            hero.topLeft,
            hero.size,
            CornerRadius(0.8f * u),
        )
        bar(hero.left + 2.0f * u, hero.top + hero.height * 0.38f, hero.width * 0.42f, 1.15f * u, Color.White.copy(alpha = 0.92f))
        bar(hero.left + 2.0f * u, hero.top + hero.height * 0.62f, hero.width * 0.58f, 0.5f * u, Color.White.copy(alpha = 0.62f))
        val columnWidth = (page.width - inset * 3f) / 2f
        val cardsTop = hero.bottom + inset
        for (column in 0..1) {
            val x = page.left + inset + column * (columnWidth + inset)
            val cardHeight = (page.bottom - inset - cardsTop).coerceAtLeast(0f)
            if (cardHeight < 2f * u) continue
            drawRoundRect(ink.card, Offset(x, cardsTop), Size(columnWidth, cardHeight), CornerRadius(0.7f * u))
            for (line in 0..2) {
                val y = cardsTop + 1.4f * u + line * 1.2f * u
                if (y > cardsTop + cardHeight - 0.8f * u) break
                bar(x + 1.2f * u, y, columnWidth * (if (line == 2) 0.45f else 0.72f), 0.45f * u, ink.text.copy(alpha = 0.3f))
            }
        }
    }
    drawRoundRect(ink.outline, rect.topLeft, rect.size, CornerRadius(radius), style = Stroke(0.12f * u))
}

/** Plasma's floating panel: the launcher, pinned apps, the tray and clock. */
private fun DrawScope.drawPanel(
    rect: Rect,
    ink: DesktopInk,
    icons: Map<String, ImageBitmap>,
    u: Float,
    s: Float,
    accent: Color,
    activeFirst: Boolean,
) {
    val radius = CornerRadius(0.9f * u * s)
    drawRoundRect(ink.panel, rect.topLeft, rect.size, radius)
    drawRoundRect(ink.text.copy(alpha = 0.08f), rect.topLeft, rect.size, radius, style = Stroke(0.1f * u))
    val y = rect.center.y
    // The launcher: four small tiles.
    val tile = 0.62f * u * s
    val launcherX = rect.left + 1.5f * u * s
    for (i in 0..3) {
        val dx = if (i % 2 == 0) -tile * 0.62f else tile * 0.62f
        val dy = if (i < 2) -tile * 0.62f else tile * 0.62f
        drawRoundRect(
            ink.text.copy(alpha = 0.82f),
            Offset(launcherX + dx - tile / 2f, y + dy - tile / 2f),
            Size(tile, tile),
            CornerRadius(tile * 0.28f),
        )
    }
    val side = rect.height * 0.64f
    var x = launcherX + 2.2f * u * s + side / 2f
    PANEL_APPS.forEachIndexed { index, id ->
        if (index == 0 && activeFirst) {
            drawRoundRect(
                ink.text.copy(alpha = 0.08f),
                Offset(x - side * 0.68f, rect.top + rect.height * 0.09f),
                Size(side * 1.36f, rect.height * 0.82f),
                CornerRadius(0.5f * u * s),
            )
            bar(x - side * 0.3f, rect.bottom - rect.height * 0.12f, side * 0.6f, 0.2f * u * s, accent)
        }
        icon(icons, id, Offset(x, y), side, ink.folder)
        x += side + 1.0f * u * s
    }
    // Clock, and the tray beside it.
    val clockRight = rect.right - 1.4f * u * s
    bar(clockRight - 3.4f * u * s, y - 0.45f * u * s, 3.4f * u * s, 0.55f * u * s, ink.text.copy(alpha = 0.8f))
    bar(clockRight - 3.0f * u * s, y + 0.55f * u * s, 2.6f * u * s, 0.36f * u * s, ink.text.copy(alpha = 0.45f))
    for (i in 0..2) {
        drawCircle(ink.text.copy(alpha = 0.5f), 0.36f * u * s, Offset(clockRight - 5.2f * u * s - i * 1.5f * u * s, y))
    }
}
