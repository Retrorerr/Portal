package app.polarbear

import androidx.compose.animation.*
import androidx.compose.animation.core.*
import androidx.compose.foundation.*
import androidx.compose.foundation.interaction.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.*
import app.polarbear.setup.PanelButton
import app.polarbear.setup.PortalPalette
import app.polarbear.setup.components.*

/** Interactive layout preview only. It deliberately sends no keys to Linux. */
@Composable
internal fun UtilityKeyboardPreview(palette: PortalPalette,width: Dp,height: Dp,onBack: () -> Unit) {
    val active = LocalUtilityContentActive.current
    var selection by remember { mutableStateOf(emptySet<String>()) }
    var description by remember { mutableStateOf("Choose a key or shortcut to preview") }
    val fontScale = LocalDensity.current.fontScale
    val keyboardWidth = (820 * fontScale.coerceAtLeast(1f)).dp
    fun preview(label: String, keys: Set<String>) { selection = keys; description = label }
    Column(Modifier.widthIn(max = 940.dp).width(width).heightIn(max = height).verticalScroll(rememberScrollState(),enabled = active).padding(18.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically,horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            PortalIcon(PortalGlyph.Keyboard,palette.textMuted)
            Column(Modifier.weight(1f)) {
                Text("Keyboard preview",color = palette.textPrimary,fontSize = 15.sp,fontWeight = FontWeight.SemiBold)
                Text("Keys are not connected to the desktop yet.",color = palette.textMuted,fontSize = 11.sp)
            }
            PanelButton("Back",false,palette,enabled = active,onClick = onBack)
        }
        Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Spacer(Modifier.height(2.dp))
            Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState(),enabled = active),horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                listOf("Ctrl+C" to "Copy","Ctrl+V" to "Paste","Ctrl+Z" to "Undo","Ctrl+S" to "Save","Alt+Tab" to "Switch apps").forEach { (keys,action) ->
                    KeyboardKey("$keys  $action",palette,selection == keys.split('+').toSet(),Modifier.heightIn(min = 34.dp)) {
                        preview("$keys — $action",keys.split('+').toSet())
                    }
                }
            }
            if (width < keyboardWidth+36.dp) Text("Scroll to explore the full keyboard",color = palette.textMuted,fontSize = 11.sp)
            Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState(),enabled = active)) {
                Row(Modifier.width(keyboardWidth),horizontalArrangement = Arrangement.spacedBy(12.dp),verticalAlignment = Alignment.Top) {
                    Column(Modifier.weight(15f),verticalArrangement = Arrangement.spacedBy(5.dp)) {
                        PreviewKeyRow(listOf("Esc","F1","F2","F3","F4","F5","F6","F7","F8","F9","F10","F11","F12"),palette,selection) { preview(it,setOf(it)) }
                        Spacer(Modifier.height(3.dp))
                        PreviewKeyRow(listOf("`","1","2","3","4","5","6","7","8","9","0","−","=","Backspace"),palette,selection) { preview(it,setOf(it)) }
                        PreviewKeyRow(listOf("Tab","Q","W","E","R","T","Y","U","I","O","P","[","]","\\"),palette,selection) { preview(it,setOf(it)) }
                        PreviewKeyRow(listOf("Caps","A","S","D","F","G","H","J","K","L",";","'","Enter"),palette,selection) { preview(it,setOf(it)) }
                        PreviewKeyRow(listOf("Shift","Z","X","C","V","B","N","M",",",".","/","Shift"),palette,selection) { preview(it,setOf(it)) }
                        PreviewKeyRow(listOf("Ctrl","Super","Alt","Space","Alt","Menu","Ctrl"),palette,selection) { preview(it,setOf(it)) }
                    }
                    Column(Modifier.weight(3.7f),verticalArrangement = Arrangement.spacedBy(5.dp)) {
                        PreviewKeyRow(listOf("Print","Scroll","Pause"),palette,selection) { preview(it,setOf(it)) }
                        Spacer(Modifier.height(3.dp))
                        PreviewKeyRow(listOf("Ins","Home","PgUp"),palette,selection) { preview(it,setOf(it)) }
                        PreviewKeyRow(listOf("Del","End","PgDn"),palette,selection) { preview(it,setOf(it)) }
                        Spacer(Modifier.height((39*fontScale.coerceAtLeast(1f)).dp))
                        Row(Modifier.fillMaxWidth(),horizontalArrangement = Arrangement.Center) {
                            KeyboardKey("↑",palette,"↑" in selection,Modifier.width((32*fontScale.coerceAtLeast(1f)).dp).height((34*fontScale.coerceAtLeast(1f)).dp)) { preview("Up arrow",setOf("↑")) }
                        }
                        PreviewKeyRow(listOf("←","↓","→"),palette,selection) { preview(when(it) { "←" -> "Left arrow"; "↓" -> "Down arrow"; else -> "Right arrow" },setOf(it)) }
                    }
                    BoxWithConstraints(Modifier.weight(4.5f)) {
                        val gap = 4.dp
                        val key = (maxWidth-gap*3)/4
                        val keyHeight = (34*fontScale.coerceAtLeast(1f)).dp
                        fun numberKeyName(value: String) = "Num$value"
                        Column(verticalArrangement = Arrangement.spacedBy(5.dp)) {
                            Spacer(Modifier.height(keyHeight+8.dp))
                            Row(horizontalArrangement = Arrangement.spacedBy(gap)) {
                                listOf("Num","÷","×","−").forEach { value ->
                                    KeyboardKey(value,palette,numberKeyName(value) in selection,Modifier.width(key).height(keyHeight)) {
                                        preview("Numpad $value",setOf(numberKeyName(value)))
                                    }
                                }
                            }
                            listOf(listOf("7","8","9","4","5","6") to "+",listOf("1","2","3","0",".") to "Enter").forEach { (values,side) ->
                                Row(horizontalArrangement = Arrangement.spacedBy(gap)) {
                                    Column(Modifier.width(key*3+gap*2),verticalArrangement = Arrangement.spacedBy(5.dp)) {
                                        Row(horizontalArrangement = Arrangement.spacedBy(gap)) {
                                            values.take(3).forEach { value -> KeyboardKey(value,palette,numberKeyName(value) in selection,Modifier.width(key).height(keyHeight)) {
                                                preview("Numpad $value",setOf(numberKeyName(value)))
                                            } }
                                        }
                                        Row(horizontalArrangement = Arrangement.spacedBy(gap)) {
                                            values.drop(3).forEach { value -> KeyboardKey(value,palette,numberKeyName(value) in selection,
                                                Modifier.width(if (value == "0") key*2+gap else key).height(keyHeight)) {
                                                preview("Numpad $value",setOf(numberKeyName(value)))
                                            } }
                                        }
                                    }
                                    KeyboardKey(side,palette,numberKeyName(side) in selection,Modifier.width(key).height(keyHeight*2+5.dp)) {
                                        preview("Numpad $side",setOf(numberKeyName(side)))
                                    }
                                }
                            }
                        }
                    }
                }
            }
            AnimatedContent(description,
                transitionSpec = { fadeIn(tween(260,delayMillis = 70,easing = PortalEmphasizedDecelerate))
                    .togetherWith(fadeOut(tween(120,easing = PortalEmphasizedAccelerate)))
                    .using(SizeTransform(clip = false) { _, _ -> spring(dampingRatio = 1f,stiffness = 380f) }) },
                label = "keyboard preview selection") { label ->
                Text(label,color = palette.textSecondary,fontSize = 12.sp)
            }
        }
    }
}

@Composable
private fun PreviewKeyRow(keys: List<String>,palette: PortalPalette,selection: Set<String>,onKey: (String)->Unit) {
    Row(Modifier.fillMaxWidth(),horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        keys.forEach { key ->
            val weight = when (key) { "Space" -> 6f; "Backspace" -> 2.8f; "Enter","Shift" -> 2.1f; "Caps" -> 1.8f; "Ctrl","Alt","Super","Menu","Tab" -> 1.3f; else -> 1f }
            KeyboardKey(key,palette,key in selection,Modifier.weight(weight).heightIn(min = (34*LocalDensity.current.fontScale.coerceAtLeast(1f)).dp)) { onKey(key) }
        }
    }
}

@Composable
private fun KeyboardKey(label: String,palette: PortalPalette,selected: Boolean,modifier: Modifier = Modifier,onClick: () -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val scale by animateFloatAsState(if (pressed) 0.92f else 1f,spring(dampingRatio = 0.8f,stiffness = 650f),label = "preview key travel")
    val depression by animateDpAsState(if (pressed) 1.dp else 0.dp, spring(dampingRatio = 0.85f, stiffness = 650f), label = "key depression")
    val fill by animateColorAsState(if (pressed || selected) palette.selectionHighlight else palette.surfaceTop,
        tween(180,easing = PortalEmphasized),label = "preview key fill")
    val border by animateColorAsState(if (pressed || selected) palette.accent.copy(alpha = 0.65f) else palette.surfaceBorder,
        tween(180,easing = PortalEmphasized),label = "preview key border")
    val shape = RoundedCornerShape(7.dp)
    Box(modifier.graphicsLayer { scaleX = scale; scaleY = scale; translationY = depression.toPx() }
        .clip(shape).background(fill).border(1.dp,border,shape)
        .semantics { contentDescription = "$label preview" }
        .clickable(interactionSource = interaction,indication = null,enabled = LocalUtilityContentActive.current,role = Role.Button,onClick = onClick)
        .padding(horizontal = 5.dp,vertical = 7.dp),contentAlignment = Alignment.Center) {
        Text(if (label == "Space") "Space" else label,color = palette.textPrimary,fontSize = 10.sp,fontWeight = FontWeight.Medium,textAlign = TextAlign.Center,maxLines = 1,softWrap = false)
    }
}
