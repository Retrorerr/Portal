package app.polarbear

// Recovery screen for a Plasma failure after installation, or a device or Android profile that
// cannot run the guest. Like the setup overlay, it is a ComposeView in a sibling FrameLayout of
// GameActivity's root, above the native SurfaceView in the same window, but it has its own
// frame and none of the veil's state machine: native code shows it, updates it in place, and
// hides it. Its buttons call native code directly (nativeRetryPlasma, nativeExportDiagnostics).

import android.app.Activity
import android.util.Log
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.AppearanceMode
import app.polarbear.setup.BeginInstallButton
import app.polarbear.setup.PortalAmbientBackground
import app.polarbear.setup.PortalDimens
import app.polarbear.setup.PortalPalette
import app.polarbear.setup.components.PortalEmphasized
import app.polarbear.setup.components.PortalEmphasizedAccelerate
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import app.polarbear.setup.components.dissolveBlur
import app.polarbear.setup.portalMarkPainter
import app.polarbear.setup.resolvePalette

object RecoveryScreen {
    private const val TAG = "PortalRecovery"

    /** Plasma failed after installation: Retry Plasma and Export diagnostics. */
    const val KIND_RUNTIME = 0
    /** The device or profile cannot run the guest: Export diagnostics only. */
    const val KIND_UNSUPPORTED = 1

    init {
        try {
            System.loadLibrary("localdesktop")
        } catch (_: UnsatisfiedLinkError) {
            // Already loaded by GameActivity; harmless.
        }
    }

    @JvmStatic external fun nativeRetryPlasma()
    @JvmStatic external fun nativeExportDiagnostics()

    internal data class UiState(
        val kind: Int,
        val reason: String,
        /** Retry Plasma was tapped; native code replaces or hides the screen next. */
        val retrying: Boolean = false,
        /** Export progress or result, shown under the actions. */
        val exportStatus: String? = null,
        val exporting: Boolean = false,
    )

    private val state = mutableStateOf(UiState(KIND_RUNTIME, ""))
    private var container: FrameLayout? = null
    private var composeView: ComposeView? = null

    /** Show the screen, or update it in place when it is already up. Any thread. */
    @JvmStatic fun show(activity: Activity, kind: Int, reason: String) {
        activity.runOnUiThread {
            // A new failure (a retry that failed again included) resets the actions.
            state.value = UiState(kind = kind, reason = reason)
            if (container != null) return@runOnUiThread
            val host = (activity as? PortalActivity)?.overlayHost()
            if (host == null) {
                Log.e(TAG, "recovery screen unavailable: host Activity is not a PortalActivity")
                return@runOnUiThread
            }
            val frame = FrameLayout(activity).apply {
                setBackgroundColor(PORTAL_CHARCOAL)
                // Owns all input while visible, so nothing reaches the desktop surface below.
                isClickable = true
                isFocusable = true
            }
            val view = ComposeView(activity).apply {
                setContent {
                    PortalRecoveryScreen(
                        state = state.value,
                        onRetry = ::retryPlasma,
                        onExport = ::exportDiagnostics,
                    )
                }
            }
            frame.addView(
                view,
                FrameLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.MATCH_PARENT,
                ),
            )
            // Added last, so it sits above the SurfaceView and any setup veil.
            host.addView(
                frame,
                FrameLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.MATCH_PARENT,
                ),
            )
            container = frame
            composeView = view
            Log.i(TAG, "recovery screen shown kind=$kind")
        }
    }

    /** Remove the screen. Safe when it is not showing. Any thread. */
    @JvmStatic fun hide(activity: Activity) {
        activity.runOnUiThread {
            val frame = container ?: return@runOnUiThread
            (frame.parent as? ViewGroup)?.removeView(frame)
            try {
                composeView?.disposeComposition()
            } catch (_: Exception) {
            }
            container = null
            composeView = null
            Log.i(TAG, "recovery screen removed")
        }
    }

    /** Native: the diagnostics export finished (share sheet opened, or failed). */
    @JvmStatic fun onExportResult(ok: Boolean, message: String) {
        val view = composeView ?: return
        view.post {
            state.value = state.value.copy(exporting = false, exportStatus = message)
            if (!ok) Log.w(TAG, "diagnostics export failed: $message")
        }
    }

    private fun retryPlasma() {
        if (state.value.retrying) return
        state.value = state.value.copy(retrying = true, exportStatus = null)
        try {
            nativeRetryPlasma()
        } catch (e: UnsatisfiedLinkError) {
            Log.e(TAG, "nativeRetryPlasma unavailable", e)
            state.value = state.value.copy(retrying = false)
        }
    }

    private fun exportDiagnostics() {
        if (state.value.exporting) return
        state.value = state.value.copy(exporting = true, exportStatus = "Preparing diagnostics…")
        try {
            nativeExportDiagnostics()
        } catch (e: UnsatisfiedLinkError) {
            Log.e(TAG, "nativeExportDiagnostics unavailable", e)
            state.value = state.value.copy(exporting = false, exportStatus = "Export is unavailable")
        }
    }

    private val PORTAL_CHARCOAL = 0xFF191B1C.toInt()
}

@Composable
private fun PortalRecoveryScreen(
    state: RecoveryScreen.UiState,
    onRetry: () -> Unit,
    onExport: () -> Unit,
) {
    val palette = resolvePalette(AppearanceMode.System)
    val runtime = state.kind == RecoveryScreen.KIND_RUNTIME
    Box(modifier = Modifier.fillMaxSize()) {
        PortalAmbientBackground(
            palette = palette,
            cardBounds = { Rect.Zero },
            readyPrelude = false,
        )
        Column(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 32.dp, vertical = 48.dp),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Image(
                painter = portalMarkPainter(main = palette.logoMain, threshold = palette.logoThreshold),
                contentDescription = "Portal logo",
                modifier = Modifier.size(PortalDimens.LogoSize),
            )
            Spacer(modifier = Modifier.height(20.dp))
            Text(
                text = if (runtime) "Plasma stopped" else "Portal can't run here",
                fontSize = PortalDimens.TitleSize,
                fontWeight = FontWeight.SemiBold,
                color = palette.textPrimary,
                textAlign = TextAlign.Center,
            )
            Spacer(modifier = Modifier.height(8.dp))
            Text(
                text = if (runtime) {
                    "Your Linux files are safe. Retry to start the desktop again."
                } else {
                    state.reason
                },
                modifier = Modifier.widthIn(max = 460.dp),
                fontSize = 14.sp,
                lineHeight = 20.sp,
                fontWeight = FontWeight.Medium,
                color = palette.textMuted,
                textAlign = TextAlign.Center,
            )
            if (runtime && state.reason.isNotBlank()) {
                RecoveryDetails(reason = state.reason, palette = palette)
            }
            Spacer(modifier = Modifier.height(32.dp))
            if (runtime) {
                BeginInstallButton(
                    palette = palette,
                    centered = false,
                    onBeginInstall = onRetry,
                    enabled = !state.retrying,
                    label = if (state.retrying) "Restarting Plasma…" else "Retry Plasma",
                    modifier = Modifier.width(PortalDimens.BeginMaxWidth),
                )
                Spacer(modifier = Modifier.height(18.dp))
                QuietAction(
                    text = "Export diagnostics →",
                    enabled = !state.exporting && !state.retrying,
                    palette = palette,
                    onClick = onExport,
                )
            } else {
                BeginInstallButton(
                    palette = palette,
                    centered = false,
                    onBeginInstall = onExport,
                    enabled = !state.exporting,
                    label = "Export diagnostics",
                    modifier = Modifier.width(PortalDimens.BeginMaxWidth),
                )
            }
            AnimatedContent(
                targetState = state.exportStatus,
                transitionSpec = {
                    (fadeIn(tween(320, delayMillis = 60, easing = PortalEmphasizedDecelerate)) +
                        slideInVertically(tween(420, easing = PortalEmphasized)) { it / 2 })
                        .togetherWith(
                            fadeOut(tween(160, easing = PortalEmphasizedAccelerate)) +
                                slideOutVertically(tween(200, easing = PortalEmphasizedAccelerate)) { -it / 3 },
                        )
                        .using(SizeTransform(clip = false))
                },
                contentAlignment = Alignment.Center,
                label = "export status",
            ) { status ->
                if (status == null) {
                    Spacer(modifier = Modifier.height(0.dp))
                } else {
                    Text(
                        text = status,
                        modifier = Modifier
                            .dissolveBlur(this, radius = 6.dp)
                            .padding(top = 12.dp)
                            .widthIn(max = 460.dp),
                        fontSize = 12.sp,
                        lineHeight = 16.sp,
                        color = palette.textMuted,
                        textAlign = TextAlign.Center,
                    )
                }
            }
        }
    }
}

/** The raw failure, folded away by default: useful for support, noise for everyone else. */
@Composable
private fun RecoveryDetails(reason: String, palette: PortalPalette) {
    var expanded by remember { mutableStateOf(false) }
    Spacer(modifier = Modifier.height(14.dp))
    QuietAction(
        text = if (expanded) "Hide details" else "Show details",
        enabled = true,
        palette = palette,
        onClick = { expanded = !expanded },
    )
    AnimatedVisibility(
        visible = expanded,
        enter = fadeIn(tween(260, easing = PortalEmphasizedDecelerate)) +
            expandVertically(tween(360, easing = PortalEmphasized)),
        exit = fadeOut(tween(160, easing = PortalEmphasizedAccelerate)) +
            shrinkVertically(tween(260, easing = PortalEmphasizedAccelerate)),
    ) {
        val shape = RoundedCornerShape(18.dp)
        SelectionContainer {
            Text(
                text = reason,
                modifier = Modifier
                    .padding(top = 12.dp)
                    .widthIn(max = 560.dp)
                    .fillMaxWidth()
                    .heightIn(max = 180.dp)
                    .clip(shape)
                    .background(palette.trackFill)
                    .border(1.dp, palette.surfaceBorder, shape)
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 16.dp, vertical = 12.dp),
                fontFamily = FontFamily.Monospace,
                fontSize = 11.sp,
                lineHeight = 16.sp,
                color = palette.textSecondary,
            )
        }
    }
}

@Composable
private fun QuietAction(
    text: String,
    enabled: Boolean,
    palette: PortalPalette,
    onClick: () -> Unit,
) {
    Text(
        text = text,
        modifier = Modifier
            .clip(RoundedCornerShape(12.dp))
            .clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
                enabled = enabled,
                role = Role.Button,
                onClick = onClick,
            )
            .padding(horizontal = 10.dp, vertical = 6.dp),
        fontSize = 13.sp,
        fontWeight = FontWeight.Medium,
        color = palette.textMuted.copy(alpha = if (enabled) 0.9f else 0.45f),
        textAlign = TextAlign.Center,
    )
}
