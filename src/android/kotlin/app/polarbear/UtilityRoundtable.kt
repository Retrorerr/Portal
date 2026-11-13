package app.polarbear

import androidx.compose.animation.*
import androidx.compose.animation.core.*
import androidx.compose.foundation.*
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Outline
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.*
import app.polarbear.setup.*
import app.polarbear.setup.components.*
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlin.math.roundToInt

private data class UtilityAction(
    val id: String, val title: String, val glyph: PortalGlyph, val detail: String = "", val available: Boolean = true,
)

@Composable
internal fun UtilityRoundtable(
    host: PortalActivity, ready: Boolean, page: UtilityPage, presentationId: Int, anchor: () -> Offset, area: UtilityMenuGeometry.Area,
    // Motion arrives as lambdas read only in layout, draw and effects: a drag
    // starting, ending or settling used to rebuild the whole roundtable,
    // stalling the UI thread for a frame or two each time.
    dockRight: Boolean, dragging: () -> Boolean, directPosition: () -> Boolean = dragging,
    onHub: () -> Unit, onAction: (String) -> Unit, onBack: () -> Unit, onOutside: () -> Unit,
    onHubBounds: (android.graphics.RectF) -> Unit, onActionBounds: (String, android.graphics.RectF) -> Unit = { _, _ -> },
    onAnimating: (Boolean) -> Unit = {}, onSettled: (UtilityPage,Int) -> Unit,
    onKeyboardBounds: (android.graphics.RectF) -> Unit = {},
    onEditController: () -> Unit = {},
    keyboardMinimized: Boolean = false, onKeyboardMinimized: (Boolean) -> Unit = {}, onClose: () -> Unit = onBack,
) {
    if (!ready) return
    val palette = resolvePalette(AppearanceMode.System)
    val density = LocalDensity.current
    val active = FileTransfers.items.count { it.phase in setOf(TransferPhase.Queued, TransferPhase.Running, TransferPhase.Canceling) }
    val failed = FileTransfers.items.asReversed().distinctBy { it.kind to it.source }
        .any { it.phase in setOf(TransferPhase.Failed, TransferPhase.Interrupted) }
    val suggestion = UtilitySuggestion.forContext(SessionControls.textInputFocused.value,
        TouchControls.steamRunning.value || TouchControls.gameRunning.value, active > 0 || failed)
    val suggestedGlyph = when (suggestion) {
        UtilitySuggestion.Keyboard -> PortalGlyph.Keyboard
        UtilitySuggestion.Gaming -> PortalGlyph.Options
        UtilitySuggestion.Files -> PortalGlyph.Files
        UtilitySuggestion.Veil -> PortalGlyph.Veil
    }
    val actions = buildList {
        add(UtilityAction("report", "Report", PortalGlyph.Report))
        add(UtilityAction("files", "Files", PortalGlyph.Files, if (active > 0) "$active copying" else if (failed) "Needs attention" else ""))
        add(UtilityAction("apps", "Apps", PortalGlyph.Apps))
        add(UtilityAction("keyboard", "Keyboard", PortalGlyph.Keyboard))
        add(UtilityAction("game", "Game settings", PortalGlyph.Options))
        add(UtilityAction("settings", "Portal settings", PortalGlyph.Settings))
        add(UtilityAction("controller", if (TouchControls.controllerVisible) "Hide controls" else "Show controls", PortalGlyph.Controller, available = TouchControls.gameRunning.value))
    }
    val available = actions.filter { it.available }
    val geometryAnchor = remember(page,area,dockRight,density.density,density.fontScale) { anchor() }
    val geometry = UtilityMenuGeometry.menu(area, UtilityMenuGeometry.Point(geometryAnchor.x,geometryAnchor.y), available.size, density.density, density.fontScale)
    val radialTarget = if (geometry.compact) 0f else 1f
    val radial by animateFloatAsState(radialTarget,spring(dampingRatio = 1f,stiffness = 380f),label = "responsive roundtable")
    var surfaceResizing by remember { mutableStateOf(false) }
    val keyboardScope = rememberCoroutineScope()
    val keyboardMotion = remember { UtilityKeyboardMotion(keyboardScope) }
    SideEffect {
        keyboardMotion.updateArea(area, density.density)
        keyboardMotion.onFlickDown = if (page == UtilityPage.Keyboard && !keyboardMinimized) ({ onKeyboardMinimized(true) }) else null
    }
    LaunchedEffect(page) { keyboardMotion.stop() }
    val transition = updateTransition(page, label = "utility presentation")
    val dockCaptureBounds = remember { android.graphics.RectF() }
    val keyboardCaptureBounds = remember { android.graphics.RectF() }
    val moving = { dragging() || directPosition() || transition.isRunning || surfaceResizing || keyboardMotion.isMoving }
    var resting by remember { mutableStateOf(false) }
    val parked = { resting && page == UtilityPage.Dock }
    val frost = rememberUtilityFrost(ready,
        crop = {
            if (page == UtilityPage.Keyboard && !moving() && !keyboardCaptureBounds.isEmpty)
                androidx.compose.ui.geometry.Rect(keyboardCaptureBounds.left,keyboardCaptureBounds.top,keyboardCaptureBounds.right,keyboardCaptureBounds.bottom)
            else if (page != UtilityPage.Dock || moving() || dockCaptureBounds.isEmpty) null
            else androidx.compose.ui.geometry.Rect(dockCaptureBounds.left,dockCaptureBounds.top,dockCaptureBounds.right,dockCaptureBounds.bottom)
        },moving = moving, parked = parked)
    val hasFrost by remember(frost) { derivedStateOf { frost.current != null } }
    val surfaceBase by animateColorAsState(palette.background.copy(alpha = if (hasFrost) PortalFrostStyle.tint(palette.isDark) else 0.96f),
        tween(320,easing = PortalEmphasized),label = "utility glass tint")
    val centering by transition.animateFloat(transitionSpec = { spring(dampingRatio = 1f, stiffness = 230f) }, label = "utility travel") {
        if (it == UtilityPage.Dock) 0f else 1f
    }
    val keyboardTravel by transition.animateFloat(transitionSpec = { spring(dampingRatio = 1f, stiffness = 230f) }, label = "keyboard travel") {
        if (it == UtilityPage.Keyboard) 1f else 0f
    }
    val settingsTravel by transition.animateFloat(transitionSpec = { spring(dampingRatio = 1f, stiffness = 230f) }, label = "settings travel") {
        if (it.isSettings) 1f else 0f
    }
    val menuReveal by transition.animateFloat(transitionSpec = { spring(dampingRatio = 0.88f, stiffness = 300f) }, label = "bubble orbit") {
        if (it == UtilityPage.Menu) 1f else 0f
    }
    val targetCenter = Offset(geometry.center.x,geometry.center.y)
    val menuCenter by animateOffsetAsState(targetCenter, spring(dampingRatio = 0.88f, stiffness = 230f), label = "utility viewport")
    // Read motion in placement/layer blocks. Rebuilding the controls on every
    // animation frame needlessly competes with the running desktop for UI time.
    val center = {
        val position = anchor()
        val settingsCenter = Offset((area.left + area.right) / 2, (area.top + area.bottom) / 2)
        if (directPosition()) position else position * (1f - centering) +
            menuCenter * (centering - keyboardTravel - settingsTravel) +
            keyboardMotion.center() * keyboardTravel + settingsCenter * settingsTravel
    }
    val arrival = remember { Animatable(0f) }
    LaunchedEffect(Unit) { arrival.animateTo(1f, tween(520, easing = PortalEmphasizedDecelerate)) }
    val interaction = remember { MutableInteractionSource() }
    val hovered by interaction.collectIsHoveredAsState()
    val focused by interaction.collectIsFocusedAsState()
    val draggingNow by rememberUpdatedState(dragging)
    val directNow by rememberUpdatedState(directPosition)
    LaunchedEffect(page, hovered, focused) {
        snapshotFlow { draggingNow() }.collectLatest { held ->
            resting = false
            if (page == UtilityPage.Dock && !held && !hovered && !focused) { delay(2200); resting = true }
        }
    }
    // Parking and lift animate from effects, never from composition: reading
    // them there rebuilt the roundtable as the tab parked or a drag began.
    val parkedNow by rememberUpdatedState(parked)
    val opacity = remember { Animatable(1f) }
    LaunchedEffect(opacity) {
        snapshotFlow { parkedNow() }.collectLatest { opacity.animateTo(if (it) 0.48f else 1f, tween(620, easing = PortalEmphasized)) }
    }
    val lift = remember { Animatable(1f) }
    LaunchedEffect(lift) {
        snapshotFlow { if (draggingNow()) 1.045f else if (keyboardMotion.dragging) 1.006f else 1f }
            .collectLatest { lift.animateTo(it, spring(dampingRatio = 0.86f, stiffness = 400f)) }
    }
    val currentAnimating by rememberUpdatedState(onAnimating)
    LaunchedEffect(transition, radialTarget, surfaceResizing) {
        snapshotFlow { transition.isRunning || arrival.isRunning || opacity.isRunning || radial != radialTarget || draggingNow() || directNow() || surfaceResizing || keyboardMotion.isMoving }
            .distinctUntilChanged().collect { currentAnimating(it) }
    }
    val currentSettled by rememberUpdatedState(onSettled)
    LaunchedEffect(presentationId, transition.currentState, transition.targetState, transition.isRunning, arrival.isRunning, surfaceResizing) {
        if (transition.isRunning || transition.currentState != transition.targetState || arrival.isRunning || arrival.value != 1f || surfaceResizing) return@LaunchedEffect
        snapshotFlow { !draggingNow() && !directNow() }.distinctUntilChanged()
            .collect { if (it) currentSettled(transition.currentState,presentationId) }
    }
    Box(Modifier.fillMaxSize().recordUtilityFrost(frost).pointerInput(page) {
        detectTapGestures {
            if (page == UtilityPage.Menu) onOutside()
            else if (page.isSettings) onBack()
        }
    }) {
        Box(Modifier.fillMaxSize().graphicsLayer { alpha = settingsTravel.coerceIn(0f, 1f) }
            .background(androidx.compose.ui.graphics.Color.Black.copy(alpha = 0.18f)))
        actions.forEach { action ->
            key(action.id) {
                val index = available.indexOfFirst { it.id == action.id }
                val target = geometry.offsets.getOrNull(index)?.let { Offset(it.x,it.y) } ?: Offset.Zero
                val orbit by animateOffsetAsState(target,spring(dampingRatio = 0.88f,stiffness = 300f),label = "${action.id} orbit")
                val present by animateFloatAsState(if (action.available) 1f else 0f,spring(dampingRatio = 1f,stiffness = 380f),label = "${action.id} availability")
                UtilityBubble(action, palette, frost, page == UtilityPage.Menu && action.available && !geometry.compact,
                    Modifier.offset {
                        // A closed menu's bubbles are invisible; leaving them
                        // parked keeps a dragged tab from laying out and
                        // redrawing hidden surfaces every frame.
                        val reveal = menuReveal*radial
                        if (reveal == 0f) return@offset IntOffset.Zero
                        val tileCenter = center() + orbit*reveal
                        IntOffset((tileCenter.x-geometry.tileWidth/2).roundToInt(), (tileCenter.y-geometry.tileHeight/2).roundToInt())
                    }
                        .width(with(density) { geometry.tileWidth.toDp() }).height(with(density) { geometry.tileHeight.toDp() })
                        .graphicsLayer {
                            alpha = menuReveal.coerceIn(0f,1f)*arrival.value*present*radial
                            val scale = 0.35f+0.65f*menuReveal.coerceIn(0f,1f)*present*radial
                            scaleX = scale; scaleY = scale
                        }
                        .then(if (page != UtilityPage.Menu || !action.available || geometry.compact) Modifier.clearAndSetSemantics {} else Modifier)
                        .onGloballyPositioned { if (page == UtilityPage.Menu && action.available && !geometry.compact) onActionBounds(action.id,it.unclippedRect()) },
                    onClick = { onAction(action.id) })
            }
        }
        val corner = animateDpAsState(when (page) {
            UtilityPage.Dock -> (24+8*(density.fontScale-1f).coerceAtLeast(0f)).dp
            UtilityPage.Menu -> if (geometry.compact) 26.dp else (32+6*(density.fontScale-1f).coerceAtLeast(0f)).dp
            UtilityPage.Keyboard -> if (keyboardMinimized) 28.dp else 18.dp
            UtilityPage.Gaming, UtilityPage.Settings -> 24.dp
        },
            tween(420, easing = PortalEmphasized), label = "utility surface corner")
        val elevation = animateDpAsState(if (page == UtilityPage.Dock) 3.dp else 12.dp, tween(420, easing = PortalEmphasized), label = "utility lift")
        val shape = remember {
            object : Shape {
                override fun createOutline(size: Size, layoutDirection: LayoutDirection, density: Density): Outline =
                    RoundedCornerShape(corner.value).createOutline(size,layoutDirection,density)
            }
        }
        Layout(content = {
            UtilitySurfaceContent(page to (page == UtilityPage.Menu && geometry.compact || page == UtilityPage.Keyboard && keyboardMinimized),
                prepareKeyboard = page == UtilityPage.Menu && !transition.isRunning && !surfaceResizing,
                onResizing = { surfaceResizing = it },
                onTargetSize = { shown, size -> if (shown == UtilityPage.Keyboard) keyboardMotion.resize(size, keyboardMinimized) },
                modifier = Modifier.graphicsLayer {
                    alpha = arrival.value * opacity.value
                    scaleX = lift.value; scaleY = lift.value
                    shadowElevation = elevation.value.toPx()
                    this.shape = RoundedCornerShape(corner.value); clip = true
                    ambientShadowColor = palette.pillShadow; spotShadowColor = palette.pillShadow
                }.utilityFrost(frost).background(surfaceBase)
                    .background(Brush.verticalGradient(listOf(palette.surfaceTop,palette.surfaceBottom)))
                    .border(1.dp,palette.surfaceBorder,shape)
                    .onGloballyPositioned {
                        val bounds = it.unclippedRect()
                        if (page == UtilityPage.Keyboard) { keyboardCaptureBounds.set(bounds); onKeyboardBounds(bounds) }
                        if (page == UtilityPage.Dock) dockCaptureBounds.set(bounds)
                        if (page == UtilityPage.Dock || page == UtilityPage.Menu && !geometry.compact) onHubBounds(bounds)
                    },
            ) { (shown, compact) ->
                when {
                    shown == UtilityPage.Keyboard && compact -> UtilityKeyboardChip(palette,
                        keyboardMotion.grip(page == UtilityPage.Keyboard && LocalUtilityContentActive.current, chip = true)) { onKeyboardMinimized(false) }
                    shown == UtilityPage.Keyboard -> UtilityKeyboard(palette,
                        with(density) { (area.width-24*density.density).coerceAtLeast(120*density.density).toDp() },
                        with(density) { (area.height-24*density.density).coerceAtLeast(120*density.density).toDp() }, onClose,
                        onMinimize = { onKeyboardMinimized(true) },
                        grip = keyboardMotion.grip(page == UtilityPage.Keyboard && LocalUtilityContentActive.current))
                    shown == UtilityPage.Gaming -> app.polarbear.gaming.GameSettingsContent(palette,
                        with(density) { (area.width-24*density.density).coerceAtLeast(1f).toDp() },
                        with(density) { (area.height-24*density.density).coerceAtLeast(1f).toDp() },
                        onBack = onBack, onEditController = onEditController)
                    shown == UtilityPage.Settings -> PortalSettingsContent(host, palette,
                        with(density) { (area.width-24*density.density).coerceAtLeast(1f).toDp() },
                        with(density) { (area.height-24*density.density).coerceAtLeast(1f).toDp() },
                        onBack = onBack)
                    shown == UtilityPage.Menu && compact -> CompactRoundtable(actions,palette,frost,
                        with(density) { (area.width-24*density.density).coerceAtLeast(120*density.density).toDp() },
                        with(density) { (area.height-24*density.density).coerceAtLeast(120*density.density).toDp() },
                        onHub,onAction,onHubBounds,onActionBounds)
                    else -> {
                        val docked = shown == UtilityPage.Dock
                        val pressed by interaction.collectIsPressedAsState()
                        val press by animateFloatAsState(if (pressed) 0.94f else 1f, spring(dampingRatio = 0.85f, stiffness = 650f), label = "utility hub press")
                        val dockWidth = (72+36*(density.fontScale-1f).coerceAtLeast(0f)).dp
                        val dockHeight = (48+16*(density.fontScale-1f).coerceAtLeast(0f)).dp
                        val hub = (64+12*(density.fontScale-1f).coerceAtLeast(0f)).dp
                        val hiddenEdge = dockWidth/2-20.dp
                        val labelInset by animateDpAsState(
                            if (!docked || dragging()) 0.dp else if (dockRight) -hiddenEdge/2 else hiddenEdge/2,
                            spring(dampingRatio = 0.9f,stiffness = 420f),label = "tab label docking")
                        Box(Modifier.size(width = if (docked) dockWidth else hub, height = if (docked) dockHeight else hub)
                            .graphicsLayer { scaleX = press; scaleY = press }
                            .hoverable(interaction)
                            .clickable(interactionSource = interaction, indication = null, enabled = page == shown && LocalUtilityContentActive.current, role = Role.Button,
                                onClickLabel = if (docked) "Open Portal menu" else "Close Portal menu", onClick = onHub)
                            .semantics {
                                contentDescription = if (docked) "Open Portal menu" else "Close Portal menu"
                                if (docked) stateDescription = when (suggestion) {
                                    UtilitySuggestion.Keyboard -> "Keyboard suggested"
                                    UtilitySuggestion.Gaming -> "Game options suggested"
                                    UtilitySuggestion.Files -> "Files suggested"
                                    UtilitySuggestion.Veil -> "No suggestion"
                                }
                            },
                            contentAlignment = Alignment.Center) {
                            if (docked) AnimatedContent(suggestedGlyph,
                                modifier = Modifier.graphicsLayer { translationX = labelInset.toPx() },
                                transitionSpec = {
                                    (fadeIn(tween(260,delayMillis = 60,easing = PortalEmphasizedDecelerate)) +
                                        scaleIn(tween(320,easing = PortalEmphasized),initialScale = 0.92f))
                                        .togetherWith(fadeOut(tween(140,easing = PortalEmphasizedAccelerate)))
                                },label = "suggested utility") { glyph ->
                                PortalIcon(glyph,palette.textPrimary,size = 20.dp)
                            }
                            else PortalIcon(PortalGlyph.Close,palette.textPrimary,size = 20.dp)
                        }
                    }
                }
            }
        }, modifier = Modifier.fillMaxSize()) { children, constraints ->
            val child = children.single().measure(Constraints(maxWidth = (area.width-24*density.density).toInt().coerceAtLeast(1),
                maxHeight = (area.height-24*density.density).toInt().coerceAtLeast(1)))
            layout(constraints.maxWidth,constraints.maxHeight) {
                val arrivalShift = (1f-arrival.value)*18*density.density*(if (dockRight) 1 else -1)
                val position = center()
                child.place((position.x-child.width/2+arrivalShift).roundToInt(),(position.y-child.height/2).roundToInt())
            }
        }
    }
}

private fun androidx.compose.ui.layout.LayoutCoordinates.unclippedRect(): android.graphics.RectF {
    val a = localToRoot(Offset.Zero)
    val b = localToRoot(Offset(size.width.toFloat(),size.height.toFloat()))
    return android.graphics.RectF(minOf(a.x,b.x),minOf(a.y,b.y),maxOf(a.x,b.x),maxOf(a.y,b.y))
}

@Composable
private fun UtilityBubble(action: UtilityAction,palette: PortalPalette,frost: UtilityFrost,enabled: Boolean,modifier: Modifier = Modifier,onClick: () -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val scale by animateFloatAsState(if (pressed) 0.91f else 1f,spring(dampingRatio = 0.82f,stiffness = 650f),label = "bubble press")
    val fill by animateColorAsState(if (pressed) palette.selectionHighlight else palette.surfaceTop,tween(180, easing = PortalEmphasized), label = "bubble fill")
    val hasFrost by remember(frost) { derivedStateOf { frost.current != null } }
    val base by animateColorAsState(palette.background.copy(alpha = if (hasFrost) PortalFrostStyle.tint(palette.isDark) else 0.96f),
        tween(320,easing = PortalEmphasized),label = "bubble glass tint")
    BoxWithConstraints(modifier.graphicsLayer { scaleX = scale; scaleY = scale }
        .semantics(mergeDescendants = true) { contentDescription = action.title + if (action.detail.isNotEmpty()) ", ${action.detail}" else "" },
        contentAlignment = Alignment.Center) {
        val diameter = minOf(maxWidth,(88+48*(LocalDensity.current.fontScale-1f).coerceAtLeast(0f)).dp)
        Column(Modifier.size(diameter).shadow(6.dp,CircleShape,ambientColor = palette.pillShadow,spotColor = palette.pillShadow)
            .clip(CircleShape).utilityFrost(frost).background(base).background(fill)
            .border(1.dp,palette.surfaceBorder,CircleShape)
            .clickable(interactionSource = interaction, indication = null, enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(8.dp),horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(4.dp,Alignment.CenterVertically)) {
            PortalIcon(action.glyph,palette.textPrimary,size = 20.dp)
            AnimatedContent(action.title,
                transitionSpec = { fadeIn(tween(240,delayMillis = 70)).togetherWith(fadeOut(tween(120))) },label = "bubble label") { label ->
                Text(label,color = palette.textPrimary,fontSize = 11.sp,fontWeight = FontWeight.Medium,
                    textAlign = TextAlign.Center,lineHeight = 14.sp,maxLines = 2)
            }
            AnimatedVisibility(action.detail.isNotEmpty(),
                enter = expandVertically(spring(dampingRatio = 1f,stiffness = 380f)) + fadeIn(tween(220)),
                exit = shrinkVertically(spring(dampingRatio = 1f,stiffness = 380f)) + fadeOut(tween(120))) {
                AnimatedContent(action.detail,transitionSpec = { fadeIn(tween(220)).togetherWith(fadeOut(tween(120))) },label = "bubble detail") { detail ->
                    Text(detail,color = palette.textMuted,fontSize = 9.sp,textAlign = TextAlign.Center,lineHeight = 12.sp,maxLines = 2)
                }
            }
        }
    }
}

@Composable
private fun CompactRoundtable(actions: List<UtilityAction>,palette: PortalPalette,frost: UtilityFrost,width: Dp,height: Dp,
    onClose: () -> Unit,onAction: (String)->Unit,onHubBounds: (android.graphics.RectF)->Unit,onBounds: (String,android.graphics.RectF)->Unit) {
    val tile = (88+48*(LocalDensity.current.fontScale-1f).coerceAtLeast(0f)).dp
    val columns = if (width < tile*2+48.dp) 1 else 2
    Column(Modifier.widthIn(max = 360.dp).width(width).heightIn(max = height).verticalScroll(rememberScrollState(),enabled = LocalUtilityContentActive.current).padding(16.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("Portal controls",Modifier.weight(1f),color = palette.textPrimary,fontSize = 15.sp,fontWeight = FontWeight.SemiBold)
            Box(Modifier.onGloballyPositioned { onHubBounds(it.unclippedRect()) }) {
                PanelButton("Close",false,palette,enabled = LocalUtilityContentActive.current,onClick = onClose)
            }
        }
        AnimatedContent(actions.filter { it.available }.map { it.id },
            transitionSpec = { fadeIn(tween(300,delayMillis = 100,easing = PortalEmphasizedDecelerate))
                .togetherWith(fadeOut(tween(140,easing = PortalEmphasizedAccelerate)))
                .using(SizeTransform(clip = true) { _, _ -> spring(dampingRatio = 1f,stiffness = 380f) }) },
            label = "compact controls") { ids ->
            Column(Modifier.padding(top = 8.dp),verticalArrangement = Arrangement.spacedBy(8.dp)) {
                ids.chunked(columns).forEach { row -> Row(Modifier.fillMaxWidth(),horizontalArrangement = Arrangement.SpaceEvenly) {
                    row.forEach { id ->
                        val action = actions.first { it.id == id }
                        UtilityBubble(action,palette,frost,action.available && LocalUtilityContentActive.current,Modifier.weight(1f).heightIn(min = tile+12.dp)
                            .onGloballyPositioned { if (action.available) onBounds(id,it.unclippedRect()) }) { onAction(id) }
                    }
                } }
            }
        }
    }
}
