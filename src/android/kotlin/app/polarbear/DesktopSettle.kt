package app.polarbear

// Plasma presents its black startup splash first, so the native "desktop
// ready" signal arrives seconds before there is a desktop worth showing.
// This waits, with a tiny copy of the desktop surface every poll, until the
// picture is no longer the splash and has stopped fading in, and then for
// the panel, which plasmashell draws a second or so after the wallpaper.

import android.graphics.Bitmap
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import android.view.PixelCopy
import android.view.View
import app.polarbear.setup.components.findDesktopSurface

internal object DesktopSettle {
    private const val TAG = "PortalDesktopSettle"
    private const val SAMPLE_W = 48
    private const val SAMPLE_H = 30
    private const val POLL_MS = 120L
    private const val MIN_LUMA = 0.06f
    private const val MAX_CHANGE = 0.012f
    /** A wallpaper darker than the splash never "arrives"; settle anyway. */
    private const val MAX_MS = 6_000L
    /** How much a panel's edge of the picture changes when the panel arrives. */
    private const val PANEL_CHANGE = 0.035f
    /** After the wallpaper, how long a panel that never shows is waited for. */
    private const val PANEL_MAX_MS = 2_500L

    private val main = Handler(Looper.getMainLooper())
    private var generation = 0
    private var watching = false

    /**
     * Run [settled] once the desktop under [root] has settled, including a
     * panel on each of [panelEdges] ("top", "bottom", "left", "right"). A
     * watch already running is kept, and [cancel] drops it. Main thread.
     */
    fun watch(root: View?, panelEdges: Set<String>, settled: () -> Unit) {
        if (watching) return
        watching = true
        val mine = ++generation
        val surface = root?.let(::findDesktopSurface)
        val started = SystemClock.uptimeMillis()
        val sample = Bitmap.createBitmap(SAMPLE_W, SAMPLE_H, Bitmap.Config.ARGB_8888)
        val pixels = IntArray(SAMPLE_W * SAMPLE_H)
        var previous = -1f
        // Once the wallpaper has settled: when, and each panel edge as it was.
        var wallpaperAt = -1L
        var edgesThen = FloatArray(0)
        var edgesBefore = FloatArray(0)
        var panelSeen = false
        fun finish(reason: String) {
            if (mine != generation) return
            watching = false
            Log.i(TAG, "desktop settled after ${SystemClock.uptimeMillis() - started}ms ($reason)")
            settled()
        }
        fun poll() {
            if (mine != generation) return
            val now = SystemClock.uptimeMillis()
            val elapsed = now - started
            when {
                surface == null || Build.VERSION.SDK_INT < 24 -> return finish("no surface to sample")
                wallpaperAt < 0 && elapsed >= MAX_MS -> return finish("time limit, luma=$previous")
                wallpaperAt >= 0 && now - wallpaperAt >= PANEL_MAX_MS -> return finish("no panel arrived")
                surface.holder.surface?.isValid != true -> return finish("surface gone")
            }
            try {
                PixelCopy.request(surface, sample, { code ->
                    if (mine != generation) return@request
                    if (code == PixelCopy.SUCCESS) {
                        sample.getPixels(pixels, 0, SAMPLE_W, 0, 0, SAMPLE_W, SAMPLE_H)
                        if (wallpaperAt < 0) {
                            val luma = meanLuma(pixels)
                            val steady = previous >= 0f && kotlin.math.abs(luma - previous) < MAX_CHANGE
                            previous = luma
                            if (luma > MIN_LUMA && steady) {
                                if (panelEdges.isEmpty()) {
                                    finish("luma=$luma")
                                    return@request
                                }
                                wallpaperAt = SystemClock.uptimeMillis()
                                edgesThen = edgeLumas(pixels, panelEdges)
                                edgesBefore = edgesThen
                                Log.i(TAG, "wallpaper settled after ${wallpaperAt - started}ms; waiting for $panelEdges panel")
                            }
                        } else {
                            val edges = edgeLumas(pixels, panelEdges)
                            if (!panelSeen) {
                                panelSeen = edges.indices.any { kotlin.math.abs(edges[it] - edgesThen[it]) > PANEL_CHANGE }
                            } else if (edges.indices.all { kotlin.math.abs(edges[it] - edgesBefore[it]) < MAX_CHANGE }) {
                                finish("panel shown")
                                return@request
                            }
                            edgesBefore = edges
                        }
                    }
                    main.postDelayed(::poll, POLL_MS)
                }, main)
            } catch (_: IllegalArgumentException) {
                main.postDelayed(::poll, POLL_MS)
            }
        }
        poll()
    }

    /** The desktop went away; a running watch never reports. Main thread. */
    fun cancel() {
        generation++
        watching = false
    }

    /** The mean brightness of the outermost sample row or column of each edge. */
    private fun edgeLumas(pixels: IntArray, edges: Set<String>): FloatArray =
        edges.sorted().map { edge ->
            val strip = when (edge) {
                "top" -> IntArray(SAMPLE_W) { pixels[it] }
                "bottom" -> IntArray(SAMPLE_W) { pixels[(SAMPLE_H - 1) * SAMPLE_W + it] }
                "left" -> IntArray(SAMPLE_H) { pixels[it * SAMPLE_W] }
                else -> IntArray(SAMPLE_H) { pixels[it * SAMPLE_W + SAMPLE_W - 1] }
            }
            meanLuma(strip)
        }.toFloatArray()

    private fun meanLuma(pixels: IntArray): Float {
        var sum = 0f
        for (p in pixels) {
            sum += 0.2126f * ((p shr 16) and 0xff) + 0.7152f * ((p shr 8) and 0xff) + 0.0722f * (p and 0xff)
        }
        return sum / (pixels.size * 255f)
    }
}
