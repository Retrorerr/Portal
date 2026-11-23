package app.polarbear

// The workspace switcher's state and actions. Native code owns the registry
// and the switch itself (src/android/workspace_switch.rs); this object only
// mirrors its JSON for Compose and keeps each workspace's last frame, which
// the Workspaces card shows so every workspace looks like itself before it
// is opened.
//
// Each workspace is a separate Debian install with its own home, apps and
// settings. A new one, or one whose install never finished, opens through a
// restart of Portal into the ordinary first-run installer, which runs in
// that workspace's empty folder.

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.Process
import android.util.Log
import android.view.PixelCopy
import android.view.SurfaceView
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import kotlinx.coroutines.suspendCancellableCoroutine
import org.json.JSONObject
import java.io.File
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.coroutines.resume

object Workspaces {
    private const val TAG = "PortalWorkspaces"
    /** Previews are a third of the desktop's size: sharp on a card, small on disk. */
    private const val PREVIEW_DIVISOR = 3
    private const val PREVIEW_QUALITY = 85
    /** How long "Opening …" outlasts the lift, so it leaves with the veil. */
    private const val OPENING_CLEAR_DELAY_MS = 700L

    data class Workspace(
        val id: String,
        val name: String,
        val lastUsedUnix: Long,
        /** Its Debian install hasn't finished; opening it resumes the installer. */
        val needsSetup: Boolean = false,
        val createdUnix: Long = 0,
    )

    data class State(
        val active: String = "",
        val openAtLaunch: String? = null,
        val workspaces: List<Workspace> = emptyList(),
        /** The workspace being opened, until its desktop starts. */
        val switching: String? = null,
        val error: String? = null,
        /** While the running workspace is being set up: where giving up goes back to. */
        val setupReturn: Workspace? = null,
        /** Where Portal goes if the open one is deleted; null when it is the only one. */
        val afterDelete: Workspace? = null,
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

    /** Main thread: also reads the list, so the Portal menu knows whether to offer Workspaces. */
    fun attach(context: Context) {
        if (previewDir == null) previewDir = File(context.filesDir, "workspaces/previews")
        refresh()
    }

    private fun parse(json: String): State? = try {
        val root = JSONObject(json)
        val list = root.optJSONArray("workspaces")
        State(
            active = root.optString("active"),
            openAtLaunch = root.optString("openAtLaunch").takeIf { !root.isNull("openAtLaunch") && it.isNotEmpty() },
            workspaces = (0 until (list?.length() ?: 0)).map { index ->
                val item = list!!.getJSONObject(index)
                Workspace(
                    item.getString("id"),
                    item.getString("name"),
                    item.optLong("lastUsed"),
                    item.optBoolean("needsSetup"),
                    item.optLong("created"),
                )
            },
            switching = root.optString("switching").takeIf { !root.isNull("switching") && it.isNotEmpty() },
            error = root.optString("error").takeIf { !root.isNull("error") && it.isNotEmpty() },
            setupReturn = root.optJSONObject("setupReturn")?.let {
                Workspace(it.getString("id"), it.getString("name"), 0)
            },
            afterDelete = root.optJSONObject("afterDelete")?.let {
                Workspace(it.getString("id"), it.getString("name"), 0)
            },
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
            applyState(next)
        }
    }

    private fun applyState(next: State) {
        val changed = state.value.active != next.active
        state.value = next
        if (changed) FileTransfers.resetWorkspaceBrowser()
    }

    /** Read the state now, e.g. when the card opens. Main thread. */
    fun refresh() {
        val json = try {
            ComposeOverlay.nativeWorkspaces()
        } catch (_: UnsatisfiedLinkError) {
            Log.e(TAG, "nativeWorkspaces unavailable")
            null
        }
        json?.let(::parse)?.let(::applyState)
        loadPreviews(state.value.workspaces.map { it.id })
    }

    /**
     * Open another workspace. The running one's frame is saved first, so its
     * card shows it as it was left. Main thread.
     */
    suspend fun switchTo(id: String, surface: SurfaceView?): Boolean = suspendCancellableCoroutine { result ->
        val current = state.value.active
        capture(current, surface) {
            if (!result.isActive) return@capture
            transferRefusal()?.let { reason ->
                state.value = state.value.copy(error = reason)
                result.resume(false)
                return@capture
            }
            val started = try {
                ComposeOverlay.nativeSwitchWorkspace(id)
            } catch (_: UnsatisfiedLinkError) {
                Log.e(TAG, "nativeSwitchWorkspace unavailable")
                state.value = state.value.copy(error = "Portal couldn't open that workspace.")
                false
            }
            if (started) {
                returnAfterSwitch.set(true)
                opening.value = id
                state.value = state.value.copy(switching = id, error = null)
            }
            result.resume(started)
        }
    }

    private fun transferRefusal(): String? =
        "Finish choosing or copying files before switching workspaces.".takeIf { FileTransfers.blocksWorkspaceSwitch }

    /** A switch started here is waiting for the new desktop. */
    fun switchPending(): Boolean = returnAfterSwitch.get()

    /**
     * Make a new workspace, a fresh Debian of its own, and restart Portal
     * into its installer. Null when the restart has begun, else why it
     * can't. Main thread.
     */
    fun create(context: Context, name: String, surface: SurfaceView?): String? {
        transferRefusal()?.let { return it }
        val refusal = try {
            ComposeOverlay.nativeCreateWorkspace(name)
        } catch (_: UnsatisfiedLinkError) {
            "Portal couldn't make the workspace."
        }
        if (refusal == null) restartInto(context, surface)
        return refusal
    }

    /**
     * Open a workspace whose install hasn't finished: Portal restarts into
     * its installer, which carries on where it stopped. Null when the
     * restart has begun. Main thread.
     */
    fun setUp(context: Context, id: String, surface: SurfaceView?): String? {
        transferRefusal()?.let { reason ->
            state.value = state.value.copy(error = reason)
            return reason
        }
        val refusal = try {
            ComposeOverlay.nativeSetUpWorkspace(id)
        } catch (_: UnsatisfiedLinkError) {
            "Portal couldn't open that workspace."
        }
        if (refusal == null) {
            opening.value = id
            restartInto(context, surface)
        }
        return refusal
    }

    /**
     * From the installer: give up the running workspace's unfinished setup,
     * delete it and restart into the workspace Portal goes back to. [refused]
     * gets the reason, on the main thread, if it can't.
     */
    fun abandonSetup(context: Context, refused: (String) -> Unit) {
        val activity = context.findActivity() ?: return refused("Portal couldn't restart.")
        io.execute {
            val refusal = try {
                ComposeOverlay.nativeAbandonWorkspaceSetup()
            } catch (_: UnsatisfiedLinkError) {
                "Portal couldn't remove that workspace."
            }
            main.post { if (refusal == null) relaunch(activity) else refused(refusal) }
        }
    }

    /**
     * Delete a workspace with its files and preview, Main included. [done]
     * gets null when deleted, else why not, on the main thread. Deleting the
     * open one switches to another in place, as a switch does ([opening]
     * names it); as the last one, Portal restarts into its setup and [done]
     * hears only a refusal.
     */
    fun delete(context: Context, id: String, done: (String?) -> Unit) {
        val dir = previewDir
        val open = id == state.value.active
        val activity = context.findActivity()
        io.execute {
            val refusal = try {
                ComposeOverlay.nativeDeleteWorkspace(id)
            } catch (_: UnsatisfiedLinkError) {
                "Portal couldn't delete that workspace."
            }
            if (refusal == null) dir?.let { File(it, "$id.jpg").delete() }
            main.post {
                if (open && refusal == null) refresh()
                val next = state.value.switching
                when {
                    refusal != null -> done(refusal)
                    open && next != null -> {
                        // The veil shows the one arriving and lifts onto it.
                        returnAfterSwitch.set(true)
                        opening.value = next
                        previews.remove(id)
                        done(null)
                    }
                    open -> activity?.let(::relaunch) ?: Process.killProcess(Process.myPid())
                    else -> {
                        // The list without it lands in this same frame.
                        refresh()
                        previews.remove(id)
                        done(null)
                    }
                }
            }
        }
    }

    /** Save this desktop's frame, stop Plasma cleanly, then restart Portal. */
    private fun restartInto(context: Context, surface: SurfaceView?) {
        val activity = context.findActivity()
        capture(state.value.active, surface) {
            Thread({
                try {
                    ComposeOverlay.nativeStopForWorkspaceRestart()
                } catch (_: UnsatisfiedLinkError) {
                    Log.e(TAG, "nativeStopForWorkspaceRestart unavailable")
                }
                main.post { activity?.let(::relaunch) ?: Process.killProcess(Process.myPid()) }
            }, "portal-workspace-restart").start()
        }
    }

    /**
     * A fresh process opens the workspace the registry names next. The new
     * task is queued before this process ends, and Android starts it in a
     * new one; the guest's processes end with this one.
     */
    private fun relaunch(activity: Activity) {
        Log.i(TAG, "restarting Portal into another workspace")
        val intent = activity.packageManager.getLaunchIntentForPackage(activity.packageName)
            ?.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
        SessionService.endProcess(activity)
        if (intent != null) activity.startActivity(intent)
        Process.killProcess(Process.myPid())
    }

    internal tailrec fun Context.findActivity(): Activity? = when (this) {
        is Activity -> this
        is ContextWrapper -> baseContext.findActivity()
        else -> null
    }

    /**
     * The new desktop has settled past Plasma's splash: lift the veil onto
     * it, so the first thing seen is the workspace itself. Main thread.
     */
    fun onDesktopSettled(lift: () -> Unit) {
        if (!returnAfterSwitch.getAndSet(false)) return
        lift()
        // The reveal animates over the new desktop; the "Opening" line goes
        // with it rather than changing under the user's eyes.
        main.postDelayed({ opening.value = null }, OPENING_CLEAR_DELAY_MS)
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
