package app.polarbear

// The workspace switcher's state and actions. Native code owns the registry
// and the switch itself (src/android/workspace_switch.rs); this object only
// mirrors its JSON for Compose and keeps each workspace's last frame, which
// the Workspaces card shows so every workspace looks like itself before it
// is opened.

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.PixelCopy
import android.view.SurfaceView
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import org.json.JSONObject
import java.io.File
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

object Workspaces {
    private const val TAG = "PortalWorkspaces"
    /** Previews are a third of the desktop's size: sharp on a card, small on disk. */
    private const val PREVIEW_DIVISOR = 3
    private const val PREVIEW_QUALITY = 85
    // Waiting for the new desktop to be worth showing: a tiny copy every
    // poll, until it is no longer the black splash and has stopped fading in.
    private const val SETTLE_SAMPLE_W = 48
    private const val SETTLE_SAMPLE_H = 30
    private const val SETTLE_POLL_MS = 120L
    private const val SETTLE_MIN_LUMA = 0.06f
    private const val SETTLE_MAX_CHANGE = 0.012f
    /** A wallpaper darker than the splash never "arrives"; lift anyway. */
    private const val SETTLE_MAX_MS = 6_000L
    private const val SETTLE_CLEAR_DELAY_MS = 700L

    data class Workspace(val id: String, val name: String, val lastUsedUnix: Long)

    data class State(
        val active: String = "",
        val openAtLaunch: String? = null,
        val workspaces: List<Workspace> = emptyList(),
        /** The workspace being opened, until its desktop starts. */
        val switching: String? = null,
        val error: String? = null,
    ) {
        fun name(id: String?) = workspaces.firstOrNull { it.id == id }?.name
    }

    val state = mutableStateOf(State())
    /** Each workspace's last frame, keyed by id. */
    val previews = mutableStateMapOf<String, ImageBitmap>()

    /**
     * The workspace being opened, from the tap until the veil lifts onto its
     * desktop. Native state clears `switching` as soon as the new session
     * starts; the veil keeps saying "Opening …" until there is a desktop to
     * show.
     */
    val opening = mutableStateOf<String?>(null)
    private val returnAfterSwitch = AtomicBoolean(false)
    private val io = Executors.newSingleThreadExecutor { Thread(it, "portal-workspace-previews") }
    private val main = Handler(Looper.getMainLooper())
    private var previewDir: File? = null

    fun attach(context: Context) {
        if (previewDir == null) previewDir = File(context.filesDir, "workspaces/previews")
    }

    private fun parse(json: String): State? = try {
        val root = JSONObject(json)
        val list = root.optJSONArray("workspaces")
        State(
            active = root.optString("active"),
            openAtLaunch = root.optString("openAtLaunch").takeIf { !root.isNull("openAtLaunch") && it.isNotEmpty() },
            workspaces = (0 until (list?.length() ?: 0)).map { index ->
                val item = list!!.getJSONObject(index)
                Workspace(item.getString("id"), item.getString("name"), item.optLong("lastUsed"))
            },
            switching = root.optString("switching").takeIf { !root.isNull("switching") && it.isNotEmpty() },
            error = root.optString("error").takeIf { !root.isNull("error") && it.isNotEmpty() },
        )
    } catch (e: Exception) {
        Log.e(TAG, "unreadable workspace state", e)
        null
    }

    /** Native push: any thread. */
    fun update(json: String) {
        val next = parse(json) ?: return
        main.post {
            // A failed switch reopens the old workspace; the veil stays so
            // the card can say why.
            if (next.switching == null && next.error != null) {
                returnAfterSwitch.set(false)
                opening.value = null
            }
            state.value = next
        }
    }

    /** Read the state now, e.g. when the card opens. Main thread. */
    fun refresh() {
        val json = try {
            ComposeOverlay.nativeWorkspaces()
        } catch (_: UnsatisfiedLinkError) {
            Log.e(TAG, "nativeWorkspaces unavailable")
            null
        }
        json?.let(::parse)?.let { state.value = it }
        loadPreviews(state.value.workspaces.map { it.id })
    }

    /**
     * Open another workspace. The running one's frame is saved first, so its
     * card shows it as it was left. Main thread.
     */
    fun switchTo(id: String, surface: SurfaceView?) {
        val current = state.value.active
        capture(current, surface) {
            val started = try {
                ComposeOverlay.nativeSwitchWorkspace(id)
            } catch (_: UnsatisfiedLinkError) {
                Log.e(TAG, "nativeSwitchWorkspace unavailable")
                false
            }
            if (started) {
                returnAfterSwitch.set(true)
                opening.value = id
                state.value = state.value.copy(switching = id, error = null)
            }
        }
    }

    /**
     * The new desktop presented its first frame. That frame is Plasma's
     * black startup splash; lift the veil once the wallpaper has come in and
     * settled, so the first thing seen is the workspace itself. Main thread.
     */
    fun onDesktopReady(root: android.view.View?, lift: () -> Unit) {
        if (!returnAfterSwitch.getAndSet(false)) return
        val surface = root?.let { app.polarbear.setup.components.findDesktopSurface(it) }
        val started = android.os.SystemClock.uptimeMillis()
        val sample = Bitmap.createBitmap(SETTLE_SAMPLE_W, SETTLE_SAMPLE_H, Bitmap.Config.ARGB_8888)
        val pixels = IntArray(SETTLE_SAMPLE_W * SETTLE_SAMPLE_H)
        var previous = -1f
        fun finish() {
            lift()
            // The reveal animates over the new desktop; the "Opening" line
            // goes with it rather than changing under the user's eyes.
            main.postDelayed({ opening.value = null }, SETTLE_CLEAR_DELAY_MS)
        }
        fun poll() {
            val elapsed = android.os.SystemClock.uptimeMillis() - started
            if (surface == null || Build.VERSION.SDK_INT < 24 || elapsed >= SETTLE_MAX_MS ||
                surface.holder.surface?.isValid != true
            ) {
                finish()
                return
            }
            try {
                PixelCopy.request(surface, sample, { code ->
                    if (code == PixelCopy.SUCCESS) {
                        sample.getPixels(pixels, 0, SETTLE_SAMPLE_W, 0, 0, SETTLE_SAMPLE_W, SETTLE_SAMPLE_H)
                        val brightness = meanLuma(pixels)
                        val settled = brightness > SETTLE_MIN_LUMA && previous >= 0f &&
                            kotlin.math.abs(brightness - previous) < SETTLE_MAX_CHANGE
                        previous = brightness
                        if (settled) {
                            Log.i(TAG, "new desktop settled after ${elapsed}ms (luma=$brightness)")
                            finish()
                            return@request
                        }
                    }
                    main.postDelayed(::poll, SETTLE_POLL_MS)
                }, main)
            } catch (_: IllegalArgumentException) {
                main.postDelayed(::poll, SETTLE_POLL_MS)
            }
        }
        poll()
    }

    private fun meanLuma(pixels: IntArray): Float {
        var sum = 0f
        for (p in pixels) {
            sum += 0.2126f * ((p shr 16) and 0xff) + 0.7152f * ((p shr 8) and 0xff) + 0.0722f * (p and 0xff)
        }
        return sum / (pixels.size * 255f)
    }

    /** Null when renamed, else the reason shown under the name. Main thread. */
    fun rename(id: String, name: String): String? = try {
        ComposeOverlay.nativeRenameWorkspace(id, name)
    } catch (_: UnsatisfiedLinkError) {
        "Portal couldn't save the name."
    }

    fun setOpenAtLaunch(id: String?) {
        try {
            ComposeOverlay.nativeSetOpenAtLaunch(id ?: "")
        } catch (_: UnsatisfiedLinkError) {
            Log.e(TAG, "nativeSetOpenAtLaunch unavailable")
        }
    }

    /**
     * Copy the desktop into the running workspace's preview, then run [then]
     * on the main thread whether or not the copy worked.
     */
    fun capture(id: String, surface: SurfaceView?, then: () -> Unit = {}) {
        val dir = previewDir
        val width = (surface?.width ?: 0) / PREVIEW_DIVISOR
        val height = (surface?.height ?: 0) / PREVIEW_DIVISOR
        if (id.isEmpty() || dir == null || surface == null || Build.VERSION.SDK_INT < 24 ||
            width < 8 || height < 8 || surface.holder.surface?.isValid != true
        ) {
            then()
            return
        }
        val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
        try {
            PixelCopy.request(surface, bitmap, { code ->
                if (code == PixelCopy.SUCCESS) {
                    bitmap.prepareToDraw()
                    previews[id] = bitmap.asImageBitmap()
                    io.execute { save(dir, id, bitmap) }
                } else {
                    Log.i(TAG, "desktop copy for $id unavailable (code=$code)")
                }
                then()
            }, main)
        } catch (e: IllegalArgumentException) {
            then()
        }
    }

    private fun save(dir: File, id: String, bitmap: Bitmap) {
        try {
            dir.mkdirs()
            val part = File(dir, "$id.jpg.part")
            part.outputStream().use { bitmap.compress(Bitmap.CompressFormat.JPEG, PREVIEW_QUALITY, it) }
            if (!part.renameTo(File(dir, "$id.jpg"))) part.delete()
        } catch (e: Exception) {
            Log.w(TAG, "could not save the preview for $id", e)
        }
    }

    private fun loadPreviews(ids: List<String>) {
        val dir = previewDir ?: return
        val missing = ids.filter { it !in previews }
        if (missing.isEmpty()) return
        io.execute {
            for (id in missing) {
                val file = File(dir, "$id.jpg")
                if (!file.isFile) continue
                val bitmap = BitmapFactory.decodeFile(file.path) ?: continue
                bitmap.prepareToDraw()
                main.post { if (id !in previews) previews[id] = bitmap.asImageBitmap() }
            }
        }
    }
}
