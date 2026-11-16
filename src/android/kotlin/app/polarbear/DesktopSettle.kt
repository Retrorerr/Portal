package app.polarbear

// Plasma presents its black startup splash first, so the native "desktop
// ready" signal arrives seconds before there is a desktop worth showing.
// This waits, with a tiny copy of the desktop surface every poll, until the
// picture is no longer the splash and has stopped fading in.

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

    private val main = Handler(Looper.getMainLooper())
    private var generation = 0
    private var watching = false

    /**
     * Run [settled] once the desktop under [root] has settled. A watch already
     * running is kept, and [cancel] drops it. Main thread.
     */
    fun watch(root: View?, settled: () -> Unit) {
        if (watching) return
        watching = true
        val mine = ++generation
        val surface = root?.let(::findDesktopSurface)
        val started = SystemClock.uptimeMillis()
        val sample = Bitmap.createBitmap(SAMPLE_W, SAMPLE_H, Bitmap.Config.ARGB_8888)
        val pixels = IntArray(SAMPLE_W * SAMPLE_H)
        var previous = -1f
        fun finish(reason: String) {
            if (mine != generation) return
            watching = false
            Log.i(TAG, "desktop settled after ${SystemClock.uptimeMillis() - started}ms ($reason)")
            settled()
        }
        fun poll() {
            if (mine != generation) return
            val elapsed = SystemClock.uptimeMillis() - started
            when {
                surface == null || Build.VERSION.SDK_INT < 24 -> return finish("no surface to sample")
                elapsed >= MAX_MS -> return finish("time limit, luma=$previous")
                surface.holder.surface?.isValid != true -> return finish("surface gone")
            }
            try {
                PixelCopy.request(surface, sample, { code ->
                    if (mine != generation) return@request
                    if (code == PixelCopy.SUCCESS) {
                        sample.getPixels(pixels, 0, SAMPLE_W, 0, 0, SAMPLE_W, SAMPLE_H)
                        val luma = meanLuma(pixels)
                        val steady = previous >= 0f && kotlin.math.abs(luma - previous) < MAX_CHANGE
                        previous = luma
                        if (luma > MIN_LUMA && steady) {
                            finish("luma=$luma")
                            return@request
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

    private fun meanLuma(pixels: IntArray): Float {
        var sum = 0f
        for (p in pixels) {
            sum += 0.2126f * ((p shr 16) and 0xff) + 0.7152f * ((p shr 8) and 0xff) + 0.0722f * (p and 0xff)
        }
        return sum / (pixels.size * 255f)
    }
}
