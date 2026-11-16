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
            if (next.switching == null && next.error != null) returnAfterSwitch.set(false)
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
                state.value = state.value.copy(switching = id, error = null)
            }
        }
    }

    /** True once, when a switch's desktop is ready: the veil then lifts by itself. */
    fun takeReturnAfterSwitch(): Boolean = returnAfterSwitch.getAndSet(false)

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
