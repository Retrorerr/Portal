package app.polarbear.setup

// "Report" in the pill's menu: the way to tell the project about a bug, or
// how Portal runs on a device nobody has tried. The card shows everything the
// form will carry before anything leaves the device, and only the person's own
// post on GitHub ever sends it.

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.ComposeOverlay
import app.polarbear.PortalReport
import app.polarbear.setup.components.LocalPortalVeil
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

@Composable
internal fun ReturnReportPanel(palette: PortalPalette, onClose: () -> Unit) {
    val context = LocalContext.current
    val veil = LocalPortalVeil.current
    val key = remember { Any() }
    var summary by remember { mutableStateOf<String?>(null) }
    var notice by remember { mutableStateOf<String?>(null) }
    val export by ComposeOverlay.diagnosticsExportState()
    LaunchedEffect(Unit) {
        summary = withContext(Dispatchers.IO) { PortalReport.summary(context.applicationContext) }
    }
    DisposableEffect(veil) { onDispose {
        veil.gestureExclusions.remove(key)
        veil.removePanel(key)
    } }
    fun open(form: String) {
        val details = summary ?: return
        if (!PortalReport.openForm(context, form, details)) {
            PortalReport.copy(context, details)
            notice = "No browser found. The device details are copied."
        }
    }
    val height = (LocalConfiguration.current.screenHeightDp - 200).coerceIn(240, 560).dp
    Column(
        Modifier
            .widthIn(max = 360.dp)
            .fillMaxWidth()
            .heightIn(max = height)
            .verticalScroll(rememberScrollState())
            .onGloballyPositioned {
                val bounds = it.boundsInRoot()
                veil.gestureExclusions[key] = bounds
                if (ComposeOverlay.activePanel == "report") veil.placePanel(key, "report", bounds)
            }
            .padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 14.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Help improve Portal", color = palette.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                Text("Nothing leaves this device until you post it.", color = palette.textMuted,
                    fontSize = 11.sp, fontWeight = FontWeight.Medium)
            }
            PanelButton(text = "Done", primary = false, palette = palette, onClick = onClose)
        }
        Spacer(Modifier.height(12.dp))
        ReportHeading("Device details", palette)
        Text("Filled into the form for you. No serial numbers or accounts.", color = palette.textMuted, fontSize = 11.sp)
        Spacer(Modifier.height(6.dp))
        Text(
            summary ?: "Reading this device…",
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(10.dp))
                .background(palette.textPrimary.copy(alpha = 0.05f))
                .heightIn(max = 120.dp)
                .verticalScroll(rememberScrollState())
                .padding(8.dp),
            color = palette.textPrimary.copy(alpha = 0.8f),
            fontSize = 10.sp,
            lineHeight = 13.sp,
            fontFamily = FontFamily.Monospace,
        )
        ReportLink("Copy details", enabled = summary != null, palette) {
            summary?.let { PortalReport.copy(context, it); notice = "Copied." }
        }
        Spacer(Modifier.height(10.dp))
        ReportHeading("Logs, for a bug", palette)
        Text(
            "Saves a zip to Download/Portal for you to attach. File paths, web addresses, emails and IP " +
                "addresses are removed where Portal can spot them, but apps can print anything, so look it over first.",
            color = palette.textMuted,
            fontSize = 11.sp,
            lineHeight = 14.sp,
        )
        ReportLink(if (export?.running == true) "Saving logs…" else "Save logs", enabled = export?.running != true, palette) {
            ComposeOverlay.exportDiagnostics()
        }
        (export?.message?.takeIf { export?.running != true } ?: notice)?.let {
            Text(it, color = palette.textMuted, fontSize = 11.sp, modifier = Modifier.padding(start = 4.dp, top = 2.dp))
        }
        Spacer(Modifier.height(12.dp))
        Text("Posting opens GitHub in your browser and needs a free account.", color = palette.textMuted, fontSize = 11.sp)
        Spacer(Modifier.height(8.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            PanelButton(text = "Report a bug", primary = true, palette = palette, enabled = summary != null) {
                open(PortalReport.BUG_FORM)
            }
            PanelButton(text = "Share what works", primary = false, palette = palette, enabled = summary != null) {
                open(PortalReport.COMPATIBILITY_FORM)
            }
        }
    }
}

@Composable
private fun ReportHeading(text: String, palette: PortalPalette) {
    Text(text, color = palette.textPrimary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold,
        modifier = Modifier.padding(bottom = 2.dp))
}

@Composable
private fun ReportLink(text: String, enabled: Boolean, palette: PortalPalette, onClick: () -> Unit) {
    Text(
        "$text →",
        modifier = Modifier
            .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null,
                enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(top = 6.dp, bottom = 2.dp),
        color = palette.textPrimary.copy(alpha = if (enabled) 0.85f else 0.4f),
        fontSize = 12.sp,
        fontWeight = FontWeight.Medium,
    )
}
