package app.polarbear.setup

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CheckboxDefaults
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.FileTransfers
import app.polarbear.GuestFile
import app.polarbear.TransferKind
import app.polarbear.TransferPhase
import app.polarbear.setup.components.*
import java.util.Locale

@Composable
internal fun FilesCapsule(palette: PortalPalette, onClick: () -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val scale by animateFloatAsState(if (pressed) 0.975f else 1f, tween(110), label = "files capsule press")
    Row(Modifier.graphicsLayer { scaleX = scale; scaleY = scale }
        .clickable(interactionSource = interaction, indication = null, role = Role.Button,
            onClickLabel = "Transfer files", onClick = onClick)
        .padding(horizontal = 14.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
        FileGlyph(palette.textPrimary, folder = true)
        Text(if (FileTransfers.busy) "Copying files…" else "Files", color = palette.textPrimary,
            fontSize = 13.sp, fontWeight = FontWeight.Medium)
        Text("›", color = palette.textMuted, fontSize = 16.sp)
    }
}

@Composable
internal fun ReturnFilesPanel(palette: PortalPalette, onClose: () -> Unit) {
    val veil = LocalPortalVeil.current
    val key = remember { Any() }
    val choosing = FileTransfers.savingFiles
    val height = (LocalConfiguration.current.screenHeightDp - 160).coerceIn(180, 480).dp
    DisposableEffect(veil) { onDispose {
        veil.gestureExclusions.remove(key)
        FileTransfers.dropBounds = android.graphics.RectF()
    } }
    LaunchedEffect(choosing) { if (choosing) FileTransfers.dropBounds = android.graphics.RectF() }
    Column(Modifier.widthIn(max = 360.dp).fillMaxWidth().heightIn(max = height)
        .onGloballyPositioned {
            val bounds = it.boundsInRoot()
            veil.gestureExclusions[key] = bounds
            if (!FileTransfers.savingFiles) FileTransfers.dropBounds =
                android.graphics.RectF(bounds.left, bounds.top, bounds.right, bounds.bottom)
        }
        .background(if (FileTransfers.dragInside && !choosing) palette.accent.copy(alpha = 0.09f) else Color.Transparent)
        .padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 12.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f).padding(start = 4.dp)) {
                Text("Files", color = palette.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                Text(if (FileTransfers.dragInside) "Release to add to Downloads" else "Between Android and your desktop",
                    color = palette.textMuted, fontSize = 11.sp, fontWeight = FontWeight.Medium)
            }
            PanelButton(if (choosing) "Cancel" else "Done", false, palette) {
                if (choosing) FileTransfers.closeSave() else onClose()
            }
        }
        Spacer(Modifier.height(8.dp))
        AnimatedContent(choosing,
            transitionSpec = {
                fadeIn(tween(300, delayMillis = 110, easing = PortalEmphasizedDecelerate))
                    .togetherWith(fadeOut(tween(140, easing = PortalEmphasizedAccelerate)))
                    .using(SizeTransform(clip = true) { _, _ -> spring(
                        dampingRatio = Spring.DampingRatioNoBouncy, stiffness = 380f) })
            }, label = "files chooser") { selection ->
            if (selection) SaveFileChooser(palette, height - 90.dp)
            else Column(Modifier.verticalScroll(rememberScrollState())) {
                FileAction("Add files", "Copy from Android into Downloads", palette,
                    enabled = FileTransfers.importDirectory.isNotEmpty(), onClick = FileTransfers::chooseFiles)
                FileAction("Save files", "Choose desktop files to save to Android", palette,
                    enabled = FileTransfers.importDirectory.isNotEmpty(), onClick = FileTransfers::beginSave)
                if (FileTransfers.importDirectory.isEmpty() && FileTransfers.message == null) {
                    Text("Opening desktop files…", Modifier.padding(4.dp), color = palette.textMuted, fontSize = 11.sp)
                }
                TransferStatus(palette)
            }
        }
        FileTransfers.message?.let {
            Text(it, Modifier.padding(horizontal = 4.dp, vertical = 8.dp), color = palette.accent, fontSize = 12.sp)
            if (FileTransfers.importDirectory.isEmpty()) PanelButton("Retry", false, palette, onClick = FileTransfers::open)
        }
    }
}

@Composable
private fun FileAction(title: String, subtitle: String, palette: PortalPalette, enabled: Boolean, onClick: () -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val scale by animateFloatAsState(if (pressed) 0.98f else 1f, tween(110), label = "file action press")
    Row(Modifier.fillMaxWidth().heightIn(min = 64.dp)
        .graphicsLayer { scaleX = scale; scaleY = scale; alpha = if (enabled) 1f else 0.45f }
        .clickable(interactionSource = interaction, indication = null, enabled = enabled, role = Role.Button, onClick = onClick)
        .padding(horizontal = 4.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title, color = palette.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
            Text(subtitle, color = palette.textSecondary, fontSize = 11.sp)
        }
        Spacer(Modifier.width(12.dp))
        Text("›", color = palette.textMuted, fontSize = 18.sp)
    }
}

@Composable
private fun SaveFileChooser(palette: PortalPalette, maxHeight: androidx.compose.ui.unit.Dp) {
    val folder = FileTransfers.folder
    Column(Modifier.fillMaxWidth().heightIn(max = maxHeight), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            if (folder.name != ".") Box(Modifier.semantics(mergeDescendants = true) { contentDescription = "Parent folder" }) {
                PanelButton("‹", false, palette, enabled = !folder.loading) { FileTransfers.browse(folder.parent) }
            }
            Text(if (folder.name == ".") "Home" else folder.name, Modifier.weight(1f),
                color = palette.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium,
                maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (folder.name != ".") PanelButton("Home", false, palette, enabled = !folder.loading, onClick = FileTransfers::home)
        }
        if (folder.loading) LinearProgressIndicator(Modifier.fillMaxWidth(), color = palette.accent, trackColor = palette.trackFill)
        folder.error?.let {
            Text(it, color = palette.accent, fontSize = 12.sp)
            PanelButton("Retry", false, palette) { FileTransfers.browse(folder.id) }
        }
        if (!folder.loading && folder.error == null && folder.entries.isEmpty()) {
            Text("This folder is empty", Modifier.padding(vertical = 20.dp), color = palette.textMuted, fontSize = 12.sp)
        }
        LazyColumn(Modifier.fillMaxWidth().weight(1f, fill = false)) {
            items(folder.entries, key = { it.id }) { entry -> GuestFileRow(entry, palette, enabled = !folder.loading) }
            if (folder.more) item { PanelButton("More files", false, palette, enabled = !folder.loading) {
                FileTransfers.browse(folder.id, append = true)
            } }
        }
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(if (FileTransfers.selected.isEmpty()) "Choose files to save" else "${FileTransfers.selected.size} selected",
                Modifier.weight(1f), color = palette.textMuted, fontSize = 11.sp)
            PanelButton("Save to Android", true, palette, enabled = FileTransfers.selected.isNotEmpty() && !folder.loading,
                onClick = FileTransfers::saveSelection)
        }
    }
}

@Composable
private fun GuestFileRow(entry: GuestFile, palette: PortalPalette, enabled: Boolean) {
    val selected = entry.id in FileTransfers.selected
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp)
        .then(if (entry.directory) Modifier.clickable(enabled = enabled, role = Role.Button) { FileTransfers.browse(entry.id) }
            else Modifier.toggleable(value = selected, enabled = enabled, role = Role.Checkbox) { FileTransfers.toggle(entry) })
        .padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        FileGlyph(if (selected) palette.accent else palette.textMuted, entry.directory)
        Column(Modifier.weight(1f)) {
            Text(entry.name, color = palette.textPrimary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (!entry.directory) Text(formatBytes(entry.size), color = palette.textMuted, fontSize = 10.sp)
        }
        if (entry.directory) Text("›", color = palette.textMuted, fontSize = 16.sp)
        else Checkbox(selected, onCheckedChange = null, colors = CheckboxDefaults.colors(checkedColor = palette.accent,
            uncheckedColor = palette.textMuted, checkmarkColor = PortalColors.Charcoal))
    }
}

@Composable
private fun TransferStatus(palette: PortalPalette) {
    // A successful retry replaces the old failure in this presentation. Keep
    // simultaneous active copies distinct, including intentional duplicates.
    val history = FileTransfers.items
    val items = history.filterIndexed { index, item ->
        item.phase !in setOf(TransferPhase.Failed, TransferPhase.Interrupted) ||
            history.drop(index + 1).none { it.kind == item.kind && it.source == item.source && it.phase != TransferPhase.Canceled }
    }
    if (items.isEmpty()) return
    var details by remember { mutableStateOf(false) }
    val active = items.filter { it.phase in setOf(TransferPhase.Queued, TransferPhase.Running, TransferPhase.Canceling) }
    val failed = items.filter { it.phase in setOf(TransferPhase.Failed, TransferPhase.Interrupted) }
    val latest = items.last()
    val status = when {
        active.isNotEmpty() -> "Copying ${active.size} ${if (active.size == 1) "file" else "files"}…"
        failed.isNotEmpty() -> "${failed.size} ${if (failed.size == 1) "file needs" else "files need"} attention"
        latest.phase == TransferPhase.Done -> if (latest.kind == TransferKind.Import) "Added to Downloads" else "Saved to Android"
        else -> "Transfer canceled"
    }
    Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(status, Modifier.weight(1f), color = palette.textMuted, fontSize = 11.sp)
        PanelButton(if (details) "Hide" else "Details", false, palette) { details = !details }
    }
    if (active.isNotEmpty()) {
        val running = active.firstOrNull { it.phase == TransferPhase.Running }
        if (running != null && running.total > 0) LinearProgressIndicator(
            progress = { (running.bytes.toDouble() / running.total).toFloat().coerceIn(0f, 1f) },
            modifier = Modifier.fillMaxWidth(), color = palette.accent, trackColor = palette.trackFill)
        else LinearProgressIndicator(Modifier.fillMaxWidth(), color = palette.accent, trackColor = palette.trackFill)
    }
    AnimatedVisibility(details,
        enter = expandVertically(tween(420, easing = PortalEmphasized)) + fadeIn(tween(300, delayMillis = 110)),
        exit = shrinkVertically(tween(420, easing = PortalEmphasized)) + fadeOut(tween(140))) {
        Column(Modifier.dissolveBlur(this, radius = 8.dp)) {
            (active + failed + items.takeLast(5)).distinctBy { it.id }.forEach { item ->
                Column(Modifier.fillMaxWidth().padding(vertical = 6.dp)) {
                    Text(item.name, color = palette.textPrimary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(when (item.phase) {
                            TransferPhase.Queued -> "Waiting"
                            TransferPhase.Running -> if (item.total > 0) "${formatBytes(item.bytes)} / ${formatBytes(item.total)}" else formatBytes(item.bytes)
                            TransferPhase.Done -> if (item.kind == TransferKind.Import) "Added to Downloads" else "Saved to Android"
                            TransferPhase.Failed -> item.error ?: "Could not copy this file"
                            TransferPhase.Interrupted -> "Interrupted · retry to continue"
                            TransferPhase.Canceling -> "Canceling…"
                            else -> "Canceled"
                        }, Modifier.weight(1f), color = palette.textMuted, fontSize = 11.sp)
                        when (item.phase) {
                            TransferPhase.Queued, TransferPhase.Running -> PanelButton("Cancel", false, palette) { FileTransfers.cancel(item) }
                            TransferPhase.Failed, TransferPhase.Interrupted -> PanelButton("Retry", false, palette) { FileTransfers.retry(item) }
                            else -> Unit
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun FileGlyph(color: Color, folder: Boolean) {
    Canvas(Modifier.size(16.dp)) {
        val path = Path().apply {
            if (folder) {
                moveTo(1f, 4f); lineTo(7f, 4f); lineTo(9f, 6f); lineTo(17f, 6f); lineTo(17f, 15f); lineTo(1f, 15f); close()
            } else {
                moveTo(4f, 1f); lineTo(11f, 1f); lineTo(15f, 5f); lineTo(15f, 17f); lineTo(4f, 17f); close()
            }
        }
        with(drawContext.canvas) { save(); scale(size.width / 18, size.height / 18); drawPath(path, androidx.compose.ui.graphics.Paint().apply {
            this.color = color; style = androidx.compose.ui.graphics.PaintingStyle.Stroke; strokeWidth = 1.3f
        }); restore() }
    }
}

private fun formatBytes(bytes: Long): String = when {
    bytes < 0 -> "Unknown size"
    bytes < 1024 -> "$bytes B"
    bytes < 1024 * 1024 -> String.format(Locale.ROOT, "%.1f KB", bytes / 1024.0)
    bytes < 1024 * 1024 * 1024 -> String.format(Locale.ROOT, "%.1f MB", bytes / (1024.0 * 1024 * 1024))
    else -> String.format(Locale.ROOT, "%.1f GB", bytes / (1024.0 * 1024 * 1024))
}
