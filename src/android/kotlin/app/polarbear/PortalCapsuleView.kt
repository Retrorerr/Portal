package app.polarbear

import android.annotation.SuppressLint
import android.content.Context
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import android.widget.FrameLayout
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandHorizontally
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import app.polarbear.setup.*
import app.polarbear.setup.components.*

/** A bounded native touch host; Compose owns the capsule's surface and actions. */
@SuppressLint("ViewConstructor")
internal class PortalCapsuleView(
    context: Context,
    private val onPositionChanged: (View) -> Unit,
    private val onSizeChanged: () -> Unit,
    private val onOpen: (Boolean) -> Unit,
    private val onGameOptions: () -> Unit,
    private val onToggleController: () -> Unit,
) : FrameLayout(context) {
    private val density = resources.displayMetrics.density
    private val slop = ViewConfiguration.get(context).scaledTouchSlop
    private var downX = 0f
    private var downY = 0f
    private var startX = 0f
    private var startY = 0f
    private var canceledChord = false
    var dragging = false; private set

    init {
        clipChildren = false
        clipToPadding = false
        addView(ComposeView(context).apply { setContent {
            PortalCapsule(onOpen, onGameOptions, onToggleController) {
                place(0.5f, -1f); onPositionChanged(this@PortalCapsuleView)
            }
        } }, LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT))
        ViewCompat.setOnApplyWindowInsetsListener(this) { _, insets ->
            post { if (!dragging) onSizeChanged() }
            insets
        }
    }

    private fun safeBounds(): CapsulePosition.Bounds {
        val parentView = parent as? View
        val insets = ViewCompat.getRootWindowInsets(this)
        val bars = insets?.getInsetsIgnoringVisibility(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
        val keyboard = insets?.getInsets(WindowInsetsCompat.Type.ime())?.bottom ?: 0
        return CapsulePosition.bounds((parentView?.width ?: 0).toFloat(), (parentView?.height ?: 0).toFloat(),
            width.toFloat(), height.toFloat(), (bars?.left ?: 0).toFloat(), (bars?.top ?: 0).toFloat(),
            (bars?.right ?: 0).toFloat(), maxOf(bars?.bottom ?: 0, keyboard).toFloat(), 8 * density)
    }
    fun place(centerFraction: Float, topFraction: Float) {
        val parentView = parent as? View ?: return
        val (x, y) = CapsulePosition.remembered(safeBounds(), parentView.width.toFloat(), parentView.height.toFloat(),
            width.toFloat(), centerFraction, topFraction)
        translationX = x; translationY = y
    }
    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        if (dragging) move(translationX, translationY) else onSizeChanged()
    }
    private fun move(x: Float, y: Float) {
        val (boundedX, boundedY) = safeBounds().clamp(x, y)
        translationX = boundedX; translationY = boundedY
    }
    override fun onInterceptTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                downX = event.rawX; downY = event.rawY
                startX = translationX; startY = translationY
                dragging = false; canceledChord = false
            }
            MotionEvent.ACTION_POINTER_DOWN -> { canceledChord = true; return true }
            MotionEvent.ACTION_MOVE -> {
                val dx = event.rawX - downX; val dy = event.rawY - downY
                if (!canceledChord && dx * dx + dy * dy > slop * slop) {
                    dragging = true
                    parent?.requestDisallowInterceptTouchEvent(true)
                    animate().scaleX(1.035f).scaleY(1.035f).setDuration(160).start()
                    return true
                }
            }
        }
        return dragging || canceledChord
    }
    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_MOVE -> if (dragging && !canceledChord) move(startX + event.rawX - downX, startY + event.rawY - downY)
            MotionEvent.ACTION_POINTER_DOWN -> canceledChord = true
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                if (dragging) {
                    if (event.actionMasked == MotionEvent.ACTION_CANCEL || canceledChord) move(startX, startY)
                    else onPositionChanged(this)
                }
                dragging = false; canceledChord = false
                parent?.requestDisallowInterceptTouchEvent(false)
                animate().scaleX(1f).scaleY(1f).setDuration(220).start()
            }
        }
        return true
    }
    override fun onDetachedFromWindow() {
        animate().cancel()
        dragging = false
        super.onDetachedFromWindow()
    }
}

@Composable
private fun PortalCapsule(onOpen: (Boolean) -> Unit, onGameOptions: () -> Unit,
    onToggleController: () -> Unit, onCenter: () -> Unit) {
    val palette = resolvePalette(AppearanceMode.System)
    val active = FileTransfers.items.filter { it.phase in setOf(TransferPhase.Queued, TransferPhase.Running, TransferPhase.Canceling) }
    val attention = FileTransfers.items.asReversed().distinctBy { it.kind to it.source }
        .any { it.phase in setOf(TransferPhase.Failed, TransferPhase.Interrupted) }
    val label = when { active.isNotEmpty() -> "Copying ${active.size}"; attention -> "Files"; else -> "Portal" }
    val game = TouchControls.gameRunning.value
    val steam = TouchControls.steamRunning.value
    val shape = RoundedCornerShape(50)
    Row(Modifier.padding(6.dp)
        .shadow(8.dp, shape, ambientColor = palette.pillShadow, spotColor = palette.pillShadow)
        .clip(shape).background(palette.background.copy(alpha = 0.96f))
        .background(Brush.verticalGradient(listOf(palette.surfaceTop, palette.surfaceBottom)))
        .border(1.dp, palette.surfaceBorder, shape)
        .animateContentSize(spring(dampingRatio = Spring.DampingRatioNoBouncy, stiffness = 380f)),
        verticalAlignment = Alignment.CenterVertically) {
        CapsuleAction("Open Portal controls", palette,
            modifier = Modifier.semantics {
                stateDescription = when { active.isNotEmpty() -> "$label files"; attention -> "A file transfer needs attention"; else -> "Desktop running. Drag to move." }
                customActions = listOf(
                    CustomAccessibilityAction("Open files") { onOpen(true); true },
                    CustomAccessibilityAction("Center capsule") { onCenter(); true },
                )
            }, onClick = { onOpen(active.isNotEmpty() || attention) }) {
            Box(Modifier.size(18.dp), contentAlignment = Alignment.Center) {
                if (active.isNotEmpty()) {
                    val item = active.firstOrNull { it.phase == TransferPhase.Running }
                    val fraction = item?.takeIf { it.total > 0 }?.let { (it.bytes.toDouble() / it.total).toFloat().coerceIn(0f, 1f) }
                    // Unknown-length streams remain a quiet dot, rather than an
                    // endlessly spinning decoration over the live desktop.
                    Canvas(Modifier.size(16.dp)) {
                        drawCircle(palette.textMuted.copy(alpha = 0.2f), style = Stroke(1.5.dp.toPx()))
                        if (fraction != null) drawArc(palette.accent, -90f, 360f * fraction, false,
                            style = Stroke(1.5.dp.toPx(), cap = StrokeCap.Round))
                        else drawCircle(palette.accent, radius = 2.dp.toPx())
                    }
                } else Image(portalMarkPainter(palette.logoMain, palette.logoThreshold), null, Modifier.size(18.dp))
            }
            Spacer(Modifier.width(8.dp))
            AnimatedContent(label,
                transitionSpec = { fadeIn(tween(300, delayMillis = 110, easing = PortalEmphasizedDecelerate))
                    .togetherWith(fadeOut(tween(140, easing = PortalEmphasizedAccelerate))) }, label = "capsule status") {
                Text(it, color = palette.textPrimary, fontSize = 12.sp, fontWeight = FontWeight.Medium, maxLines = 1)
            }
            if (attention && active.isEmpty()) Box(Modifier.padding(start = 6.dp).size(4.dp).background(palette.accent, shape))
        }
        AnimatedVisibility(steam || game,
            enter = expandHorizontally(tween(420, easing = PortalEmphasized)) + fadeIn(tween(300, delayMillis = 110)),
            exit = shrinkHorizontally(tween(420, easing = PortalEmphasized)) + fadeOut(tween(140))) {
            Row(Modifier.dissolveBlur(this, radius = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.width(1.dp).height(16.dp).background(palette.surfaceBorder))
                AnimatedVisibility(game,
                    enter = expandHorizontally(tween(420, easing = PortalEmphasized)) + fadeIn(tween(300)),
                    exit = shrinkHorizontally(tween(420, easing = PortalEmphasized)) + fadeOut(tween(140))) {
                    CapsuleAction(if (TouchControls.controllerVisible) "Hide controller" else "Show controller", palette,
                        onClick = onToggleController) { CapsuleGameGlyph(palette, controller = true, selected = TouchControls.controllerVisible) }
                }
                CapsuleAction("Game options", palette, onClick = onGameOptions) { CapsuleGameGlyph(palette, controller = false) }
            }
        }
    }
}

@Composable
private fun CapsuleAction(description: String, palette: PortalPalette, modifier: Modifier = Modifier,
    onClick: () -> Unit, content: @Composable RowScope.() -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val scale by animateFloatAsState(if (pressed) 0.96f else 1f, tween(110), label = "capsule press")
    Row(modifier.graphicsLayer { scaleX = scale; scaleY = scale }
        .semantics { contentDescription = description }
        .clickable(interactionSource = interaction, indication = null, role = Role.Button, onClick = onClick)
        .heightIn(min = 36.dp).padding(horizontal = 12.dp, vertical = 7.dp),
        verticalAlignment = Alignment.CenterVertically, content = content)
}

@Composable
private fun CapsuleGameGlyph(palette: PortalPalette, controller: Boolean, selected: Boolean = false) {
    Canvas(Modifier.size(20.dp)) {
        val ink = if (selected) palette.accent else palette.textPrimary
        val stroke = Stroke(1.4.dp.toPx(), cap = StrokeCap.Round)
        if (controller) {
            val rectSize = Size(size.width * 0.9f, size.height * 0.65f)
            val origin = Offset((size.width - rectSize.width) / 2, (size.height - rectSize.height) / 2)
            drawRoundRect(ink, origin, rectSize, CornerRadius(4.dp.toPx()), style = stroke)
            drawCircle(ink, 1.5.dp.toPx(), Offset(size.width * 0.32f, size.height / 2))
            drawCircle(ink, 1.5.dp.toPx(), Offset(size.width * 0.68f, size.height / 2))
        } else {
            listOf(-1 to 0.45f, 0 to -0.4f, 1 to 0.15f).forEach { (row, knob) ->
                val y = size.height / 2 + row * 5.dp.toPx()
                drawLine(ink, Offset(2.dp.toPx(), y), Offset(size.width - 2.dp.toPx(), y), stroke.width, StrokeCap.Round)
                drawCircle(palette.background, 2.5.dp.toPx(), Offset(size.width / 2 + knob * size.width * 0.35f, y))
                drawCircle(ink, 2.dp.toPx(), Offset(size.width / 2 + knob * size.width * 0.35f, y), style = stroke)
            }
        }
    }
}
