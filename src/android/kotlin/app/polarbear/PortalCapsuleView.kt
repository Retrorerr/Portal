package app.polarbear

import android.annotation.SuppressLint
import android.content.Context
import android.view.MotionEvent
import android.view.VelocityTracker
import android.view.View
import android.view.ViewConfiguration
import android.widget.FrameLayout
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.VectorConverter
import androidx.compose.animation.core.spring
import androidx.compose.runtime.*
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.platform.AndroidUiDispatcher
import androidx.compose.ui.platform.ComposeView
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import kotlinx.coroutines.*

internal enum class UtilityPage { Dock, Menu, Keyboard }

/** The host is transparent outside its actual controls; guest touches pass through. */
@SuppressLint("ViewConstructor")
internal class PortalCapsuleView(
    context: Context,
    private val onPositionChanged: (Float, Float) -> Unit,
    private val onSizeChanged: () -> Unit,
    private val onOpen: (String?) -> Unit,
    private val onGameOptions: () -> Unit,
    private val onToggleController: () -> Unit,
) : FrameLayout(context) {
    private val density get() = resources.displayMetrics.density
    private val slop get() = ViewConfiguration.get(context).scaledTouchSlop
    private val scope = CoroutineScope(AndroidUiDispatcher.Main + SupervisorJob())
    private val anchor = Animatable(Offset.Zero, Offset.VectorConverter)
    private var settle: Job? = null
    private var tracker: VelocityTracker? = null
    private var down = Offset.Zero
    private var dragStart = Offset.Zero
    private var chord = false
    private var dragEligible = false
    private var touchOwned = false
    private var settledPage: UtilityPage? = null
    private var pendingAction: (() -> Unit)? = null
    private var dragPoint by mutableStateOf(Offset.Zero)
    private var freeMotion by mutableStateOf(false)
    private val actionBounds = mutableMapOf<String, android.graphics.RectF>()
    private var ready by mutableStateOf(false)
    private var page by mutableStateOf(UtilityPage.Dock)
    private var area by mutableStateOf(UtilityMenuGeometry.Area(0f,0f,0f,0f))
    private var dockRight by mutableStateOf(true)
    var dragging by mutableStateOf(false); private set
    private var highRefresh: Boolean? = null
    private var hubBounds = android.graphics.RectF()
    private val composition = ComposeView(context).apply {
        setContent { app.polarbear.setup.components.PortalUiDensity {
            UtilityRoundtable(ready, page, { if (dragging) dragPoint else anchor.value }, area,
                dockRight, dragging, directPosition = freeMotion,
                onHub = { if (page == UtilityPage.Dock) reveal() else dismiss() },
                onAction = { action -> when (action) {
                    "keyboard" -> changePage(UtilityPage.Keyboard)
                    "controller" -> onToggleController()
                    "game" -> handoff(onGameOptions)
                    "files", "apps" -> handoff { onOpen(action) }
                    else -> handoff { onOpen(null) }
                } },
                onBack = { changePage(UtilityPage.Menu) }, onOutside = { dismiss() },
                onAnimating = { updateFrameRate(it || SessionControls.isUtilityVisible()) },
                onHubBounds = { hubBounds = it }, onActionBounds = { id, rect -> actionBounds[id] = rect }, onSettled = { settled ->
                    settledPage = settled
                    if (settled == UtilityPage.Dock && !dragging) {
                        val action = pendingAction.also { pendingAction = null }
                        if (action != null) action() else releaseInput()
                    }
                })
        } }
    }
    init {
        clipChildren = false; clipToPadding = false
        addView(composition, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
        ViewCompat.setOnApplyWindowInsetsListener(this) { _, insets -> post { onSizeChanged() }; insets }
    }
    private fun updateArea() {
        val insets = ViewCompat.getRootWindowInsets(this)
        val bars = insets?.getInsetsIgnoringVisibility(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
        val bottom = maxOf(bars?.bottom ?: 0, insets?.getInsets(WindowInsetsCompat.Type.ime())?.bottom ?: 0)
        area = UtilityMenuGeometry.Area((bars?.left ?: 0).toFloat(), (bars?.top ?: 0).toFloat(),
            (width - (bars?.right ?: 0)).toFloat(), (height-bottom).coerceAtLeast(bars?.top ?: 0).toFloat())
    }
    fun place(centerFraction: Float, topFraction: Float) {
        if (width <= 0 || height <= 0 || dragging) return
        updateArea()
        freeMotion = false
        dockRight = !centerFraction.isFinite() || centerFraction >= 0.5f
        val y = if (topFraction.isFinite() && topFraction >= 0f) topFraction * height else area.top + area.height * 0.24f
        val target = UtilityMenuGeometry.dock(area, dockRight, y, density, resources.configuration.fontScale)
        settle?.cancel()
        settle = scope.launch {
            val destination = Offset(target.x,target.y)
            if (!ready) { anchor.snapTo(destination); ready = true }
            else anchor.animateTo(destination, spring(dampingRatio = 0.88f, stiffness = 230f))
        }
    }
    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w,h,oldw,oldh)
        if (dragging) { dragging = false; recycleTracker() }
        updateArea(); onSizeChanged()
    }
    override fun onConfigurationChanged(configuration: android.content.res.Configuration) {
        super.onConfigurationChanged(configuration)
        updateArea()
        if (!dragging) onSizeChanged()
    }
    private fun changePage(next: UtilityPage) {
        pendingAction = null
        settledPage = null
        if (next != UtilityPage.Dock && !SessionControls.setUtilityVisible(true)) return
        freeMotion = false
        page = next
        if (next != UtilityPage.Dock) composition.requestFocus()
    }
    fun reveal() { changePage(UtilityPage.Menu) }
    fun dismiss(): Boolean {
        if (!SessionControls.isUtilityVisible()) return false
        pendingAction = null
        page = if (page == UtilityPage.Keyboard) UtilityPage.Menu else UtilityPage.Dock
        return true
    }
    fun openVeil() { handoff { onOpen(null) } }
    private fun handoff(action: () -> Unit) {
        pendingAction = action
        page = UtilityPage.Dock
    }
    private fun releaseInput() {
        SessionControls.setUtilityVisible(false)
        composition.clearFocus()
        updateFrameRate(false)
    }
    override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean {
        if (!SessionControls.isUtilityVisible() && !hubBounds.contains(event.x,event.y)) return false
        return super.dispatchGenericMotionEvent(event)
    }
    override fun dispatchTouchEvent(event: MotionEvent): Boolean {
        if (event.actionMasked == MotionEvent.ACTION_DOWN) {
            touchOwned = SessionControls.isUtilityVisible() || hubBounds.contains(event.x,event.y)
            if (!touchOwned) return false
        }
        if (!touchOwned) return false
        val result = super.dispatchTouchEvent(event)
        if (event.actionMasked == MotionEvent.ACTION_UP || event.actionMasked == MotionEvent.ACTION_CANCEL) touchOwned = false
        return result || SessionControls.isUtilityVisible()
    }
    override fun onInterceptTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                down = Offset(event.x,event.y); chord = false
                dragEligible = page == UtilityPage.Dock && !SessionControls.isUtilityVisible() && hubBounds.contains(event.x,event.y)
                dragStart = Offset(hubBounds.centerX(),hubBounds.centerY())
                settle?.cancel()
                recycleTracker(); tracker = VelocityTracker.obtain().also { it.addMovement(event) }
            }
            MotionEvent.ACTION_POINTER_DOWN -> { chord = true; return dragEligible }
            MotionEvent.ACTION_MOVE -> {
                tracker?.addMovement(event)
                val delta = Offset(event.x,event.y)-down
                if (dragEligible && !chord && delta.getDistance() > slop) {
                    dragPoint = dragStart + delta
                    freeMotion = true; dragging = true; page = UtilityPage.Dock; pendingAction = null
                    parent?.requestDisallowInterceptTouchEvent(true)
                    return true
                }
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> if (!dragging) recycleTracker()
        }
        return dragging || chord && dragEligible
    }
    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        tracker?.addMovement(event)
        when (event.actionMasked) {
            MotionEvent.ACTION_MOVE -> if (dragging && !chord) {
                val x = dragStart.x + event.x-down.x
                val y = dragStart.y + event.y-down.y
                dragPoint = Offset(x.coerceIn(area.left+24*density,(area.right-24*density).coerceAtLeast(area.left+24*density)),
                    y.coerceIn(area.top+24*density,(area.bottom-24*density).coerceAtLeast(area.top+24*density)))
            }
            MotionEvent.ACTION_POINTER_DOWN -> chord = true
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                if (dragging) {
                    tracker?.computeCurrentVelocity(1000)
                    val velocity = if (chord || event.actionMasked == MotionEvent.ACTION_CANCEL) Offset.Zero
                        else Offset(tracker?.xVelocity ?: 0f, tracker?.yVelocity ?: 0f)
                    val current = if (event.actionMasked == MotionEvent.ACTION_CANCEL || chord) dragStart else dragPoint
                    val (target, right) = UtilityMenuGeometry.released(area,
                        UtilityMenuGeometry.Point(current.x,current.y), UtilityMenuGeometry.Point(velocity.x,velocity.y),density,resources.configuration.fontScale)
                    dockRight = right
                    settle?.cancel()
                    settle = scope.launch {
                        anchor.snapTo(current); dragging = false
                        anchor.animateTo(Offset(target.x,target.y), spring(dampingRatio = 0.86f, stiffness = 210f),
                            initialVelocity = velocity.let { Offset(it.x.coerceIn(-2400*density,2400*density),it.y.coerceIn(-2000*density,2000*density)) })
                        freeMotion = false
                        onPositionChanged(if (dockRight) 1f else 0f, anchor.value.y/height.coerceAtLeast(1))
                        if (page == UtilityPage.Dock && pendingAction == null) releaseInput()
                    }
                }
                parent?.requestDisallowInterceptTouchEvent(false)
                recycleTracker(); chord = false
            }
        }
        return true
    }
    private fun recycleTracker() { tracker?.recycle(); tracker = null }
    private fun updateFrameRate(active: Boolean) {
        if (highRefresh == active) return
        highRefresh = active
        (context as? PortalActivity)?.setUtilityHighRefresh(active)
        if (android.os.Build.VERSION.SDK_INT >= 35) post {
            val rate = if (active) ((context as? PortalActivity)?.highRefreshTargetMillihz() ?: 0)/1000f else View.REQUESTED_FRAME_RATE_CATEGORY_NO_PREFERENCE
            ComposeOverlay.requestFrameRate(this,if (active && rate <= 0f) View.REQUESTED_FRAME_RATE_CATEGORY_HIGH else rate)
        }
    }
    override fun onDetachedFromWindow() {
        settle?.cancel(); recycleTracker(); scope.cancel(); releaseInput(); dragging = false
        super.onDetachedFromWindow()
    }
}
