package app.polarbear

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.util.AtomicFile
import android.view.DragEvent
import android.webkit.MimeTypeMap
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.lang.ref.WeakReference
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicInteger

enum class TransferKind { Import, Save }
enum class TransferPhase { Queued, Running, Canceling, Done, Failed, Canceled, Interrupted }
data class TransferItem(
    val id: String = UUID.randomUUID().toString(), val name: String,
    val kind: TransferKind, val phase: TransferPhase = TransferPhase.Queued,
    val bytes: Long = 0, val total: Long = -1, val error: String? = null,
    val source: String, val destination: String,
    val output: String? = null, val permission: String? = null,
)
data class GuestFile(val id: String, val name: String, val directory: Boolean, val size: Long, val modified: Long = 0)
data class GuestFolder(
    val id: String = "", val parent: String = "", val name: String = "Downloads",
    val entries: List<GuestFile> = emptyList(), val loading: Boolean = false,
    val more: Boolean = false, val error: String? = null,
)

/** Process-owned transfers. No worker or URI permission is owned by a composition. */
object FileTransfers {
    @JvmStatic private external fun nativeRun(request: String, input: Int, output: Int, job: String): String
    @JvmStatic private external fun nativeCancel(job: String)
    @JvmStatic private external fun nativeForget(job: String)

    var items by mutableStateOf<List<TransferItem>>(emptyList()); private set
    var folder by mutableStateOf(GuestFolder()); private set
    var importDirectory by mutableStateOf(""); private set
    var importLabel by mutableStateOf("Downloads"); private set
    var savingFiles by mutableStateOf(false); private set
    var selected by mutableStateOf<Set<String>>(emptySet()); private set
    var showHidden by mutableStateOf(false); private set
    var message by mutableStateOf<String?>(null); private set
    var dragInside by mutableStateOf(false); private set
    var filesRequested by mutableStateOf(false)
    // Android View coordinates, in the stable root's coordinate space.
    var dropBounds = android.graphics.RectF()
        set(value) { field = value; if (incomingDrag) dragInside = value.contains(dragX, dragY) }
    private var incomingDrag = false
    private var dragX = Float.NaN
    private var dragY = Float.NaN

    private lateinit var app: Context
    private var activity = WeakReference<PortalActivity>(null)
    private val main = Handler(Looper.getMainLooper())
    private val work = Executors.newSingleThreadExecutor()
    private val listings = Executors.newSingleThreadExecutor()
    private val journal = Executors.newSingleThreadExecutor()
    private val canceled = ConcurrentHashMap.newKeySet<String>()
    private val running = ConcurrentHashMap.newKeySet<String>()
    private val signals = ConcurrentHashMap<String, android.os.CancellationSignal>()
    private var initialized = false
    private var initializing = false
    private var listingGeneration = 0
    private var home = ""
    private var pendingSave = emptyList<GuestFile>()
    private var pick: (() -> Unit)? = null
    private var save: ((Intent) -> Unit)? = null
    private var tree: ((Intent) -> Unit)? = null
    val busy: Boolean get() = items.any { it.phase == TransferPhase.Queued || it.phase == TransferPhase.Running || it.phase == TransferPhase.Canceling }

    fun attach(host: PortalActivity) {
        app = host.applicationContext
        activity = WeakReference(host)
        val picker = host.registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            if (result.resultCode == Activity.RESULT_OK) {
                val data = result.data
                val uris = data?.clipData?.let { clip -> (0 until clip.itemCount).mapNotNull { clip.getItemAt(it).uri } }
                    ?: listOfNotNull(data?.data)
                importUris(uris)
            }
        }
        val saver = host.registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            val selection = pendingSave.also { pendingSave = emptyList() }
            if (result.resultCode == Activity.RESULT_OK) {
                result.data?.data?.let { uri -> selection.singleOrNull()?.let {
                    enqueueSave(it, uri, false, retainSavePermission(result.data)); closeSave()
                } }
            }
        }
        val directory = host.registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            val selection = pendingSave.also { pendingSave = emptyList() }
            if (result.resultCode == Activity.RESULT_OK) {
                result.data?.data?.let { uri ->
                    val permission = retainSavePermission(result.data)
                    selection.forEach { enqueueSave(it, uri, true, permission) }; closeSave()
                }
            }
        }
        pick = { picker.launch(Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE); type = "*/*"
            putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }) }
        save = saver::launch
        tree = directory::launch
        if (!initialized) {
            initialized = true
            restore()
            // Remove temporary drag snapshots left by earlier preview builds.
            work.execute { File(app.cacheDir, "file-exports").deleteRecursively() }
        }
        host.overlayHost().setOnDragListener { _, event -> receiveDrag(event) }
    }

    fun open() {
        if (home.isNotEmpty()) {
            if (!folder.loading && folder.id.isNotEmpty()) browse(folder.id, keepSelection = true)
            return
        }
        if (initializing) return
        message = null
        initializing = true
        listings.execute {
            val defaults = runCatching { run(JSONObject().put("action", "defaults")) }
            main.post {
                initializing = false
                defaults.onSuccess {
                    home = it.getString("home")
                    importDirectory = it.getString("downloads")
                    importLabel = it.optString("label", "Downloads")
                    browse(importDirectory)
                }.onFailure { message = it.message ?: "Could not open Linux files" }
            }
        }
    }

    fun browse(id: String, append: Boolean = false, keepSelection: Boolean = false) {
        if (id.isEmpty()) return
        val generation = ++listingGeneration
        val previous = if (append) folder.entries else emptyList()
        folder = if (id == folder.id) folder.copy(loading = true, error = null)
            else GuestFolder(id = id, name = "Opening…", loading = true)
        if (!append && !keepSelection) selected = emptySet()
        val hidden = showHidden
        listings.execute {
            val result = runCatching { run(JSONObject().put("action", "list").put("directory", id)
                .put("hidden", hidden).put("offset", previous.size)) }
            main.post {
                if (generation != listingGeneration) return@post
                result.onSuccess { value ->
                    val entries = value.getJSONArray("entries")
                    folder = GuestFolder(value.getString("directory"), value.getString("parent"),
                        value.getString("name"), previous + (0 until entries.length()).map {
                            entries.getJSONObject(it).let { entry -> GuestFile(entry.getString("id"),
                                entry.getString("name"), entry.getBoolean("directory"), entry.getLong("size"), entry.getLong("modified")) }
                        }.distinctBy { it.id }, more = value.getBoolean("more"))
                    if (keepSelection) selected = selected.intersect(folder.entries.map { it.id }.toSet())
                }.onFailure { folder = folder.copy(loading = false, error = it.message) }
            }
        }
    }

    fun home() = browse(home)
    fun hidden() { showHidden = !showHidden; browse(folder.id) }
    fun toggle(entry: GuestFile) { selected = if (entry.id in selected) selected - entry.id else selected + entry.id }
    fun beginSave() {
        savingFiles = true
        message = null
        selected = emptySet()
        browse(importDirectory)
    }
    fun closeSave() { savingFiles = false; selected = emptySet() }
    fun back(): Boolean {
        if (savingFiles) { closeSave(); return true }
        if (selected.isNotEmpty()) { selected = emptySet(); return true }
        return false
    }
    fun chooseFiles() {
        if (importDirectory.isEmpty()) { open(); message = "Linux files are still opening"; return }
        message = null
        runCatching { pick?.invoke() }.onFailure { message = "Android's file picker is unavailable" }
    }

    fun saveSelection() {
        val selection = folder.entries.filter { it.id in selected && !it.directory }
        if (selection.isEmpty()) return
        pendingSave = selection
        runCatching {
            if (selection.size == 1) save?.invoke(Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE); type = mime(selection[0].name)
                putExtra(Intent.EXTRA_TITLE, selection[0].name)
            }) else tree?.invoke(Intent(Intent.ACTION_OPEN_DOCUMENT_TREE))
        }.onFailure { pendingSave = emptyList(); message = "Android's save dialog is unavailable" }
    }

    private fun receiveDrag(event: DragEvent): Boolean {
        if (Build.VERSION.SDK_INT < 24) return false
        return when (event.action) {
            DragEvent.ACTION_DRAG_STARTED -> {
                incomingDrag = false
                event.clipDescription != null
            }
            DragEvent.ACTION_DRAG_ENTERED, DragEvent.ACTION_DRAG_LOCATION -> {
                dragX = event.x; dragY = event.y
                if (!incomingDrag) {
                    incomingDrag = true
                    val host = activity.get() ?: return false
                    filesRequested = true
                    if (!ComposeOverlay.isShowing()) SessionControls.open(host, requireFocus = false)
                    open()
                }
                dragInside = dropBounds.contains(dragX, dragY)
                true
            }
            DragEvent.ACTION_DROP -> {
                dragInside = false
                if (!dropBounds.contains(event.x, event.y) || importDirectory.isEmpty()) return false
                val host = activity.get() ?: return false
                val clip = event.clipData ?: return false
                val uris = (0 until clip.itemCount).mapNotNull { clip.getItemAt(it).uri }
                if (uris.isEmpty() || uris.any { it.scheme != "content" }) {
                    message = "Drop files here. Text, links and folders cannot be transferred."; return false
                }
                val permission = host.requestDragAndDropPermissions(event)
                importUris(uris, permission?.let { { it.release() } }); true
            }
            DragEvent.ACTION_DRAG_EXITED, DragEvent.ACTION_DRAG_ENDED -> {
                incomingDrag = false; dragInside = false; dragX = Float.NaN; dragY = Float.NaN; true
            }
            else -> true
        }
    }

    fun importUris(uris: List<Uri>, releasePermission: (() -> Unit)? = null, destination: String = importDirectory) {
        if (destination.isEmpty()) { releasePermission?.invoke(); message = "Open Files before choosing a transfer"; return }
        val candidates = uris.distinct()
        if (candidates.isEmpty()) { releasePermission?.invoke(); return }
        val remaining = AtomicInteger(candidates.size)
        candidates.forEach { uri ->
            val item = TransferItem(name = uri.lastPathSegment?.substringAfterLast('/') ?: "File",
                kind = TransferKind.Import, source = uri.toString(), destination = destination)
            enqueue(item, completed = { if (remaining.decrementAndGet() == 0) releasePermission?.invoke() }) { current ->
                    require(uri.scheme == "content") { "Choose a file from Android's file picker" }
                    require(app.contentResolver.getType(uri) != DocumentsContract.Document.MIME_TYPE_DIR) {
                        "Folders cannot be transferred. Choose files or a ZIP archive."
                    }
                    var name = current.name
                    var size = -1L
                    app.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { cursor ->
                        if (cursor.moveToFirst()) {
                            val nameIndex = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                            val sizeIndex = cursor.getColumnIndex(OpenableColumns.SIZE)
                            if (nameIndex >= 0 && !cursor.isNull(nameIndex)) name = cursor.getString(nameIndex)
                            if (sizeIndex >= 0 && !cursor.isNull(sizeIndex)) size = cursor.getLong(sizeIndex)
                        }
                    }
                    update(current.id) { it.copy(name = name, total = size) }
                    val signal = android.os.CancellationSignal()
                    signals[current.id] = signal
                    if (current.id in canceled) signal.cancel()
                    val request = JSONObject().put("action", "import").put("directory", current.destination)
                        .put("name", name).put("size", size).put("job", current.id).put("defer_publish", true)
                    try {
                        app.contentResolver.openFileDescriptor(uri, "r", signal)?.use { input ->
                            check(current.id !in canceled) { "Transfer canceled" }
                            run(request, input.fd, job = current.id)
                            try { input.checkError() } catch (error: java.io.IOException) {
                                throw IllegalStateException("The source app could not finish sending this file. Choose it again.", error)
                            }
                        } ?: error("This document is unavailable. Choose it again.")
                        check(current.id !in canceled) { "Transfer canceled" }
                        run(request.put("action", "commit"), job = current.id).optString("name", name)
                    } catch (error: Exception) {
                        runCatching { run(JSONObject().put("action", "cleanup").put("directory", current.destination).put("job", current.id)) }
                        throw error
                    }
            }
        }
    }

    private fun enqueueSave(file: GuestFile, destination: Uri, directory: Boolean, permission: String? = null) {
        val transfer = TransferItem(name = file.name, kind = TransferKind.Save, total = file.size,
            source = file.id, destination = (if (directory) "tree:" else "") + destination,
            output = if (directory) null else destination.toString(), permission = permission)
        var started = false
        enqueue(transfer, completed = {
            if (!started && !directory) runCatching { DocumentsContract.deleteDocument(app.contentResolver, destination) }
        }) { item ->
            started = true
            var target: Uri? = null
            try {
                target = if (directory) {
                    val parent = DocumentsContract.buildDocumentUriUsingTree(destination, DocumentsContract.getTreeDocumentId(destination))
                    DocumentsContract.createDocument(app.contentResolver, parent, mime(file.name), file.name)
                        ?: error("This Android folder cannot create files")
                } else destination
                // Record the provider's exact new URI (including any collision
                // rename) before writing bytes. Recovery never guesses a name.
                val checkpoint = java.util.concurrent.FutureTask {
                    items = items.map { if (it.id == item.id) it.copy(output = target.toString()) else it }
                    persist()
                }
                main.post(checkpoint)
                checkpoint.get().get()
                val signal = android.os.CancellationSignal()
                signals[item.id] = signal
                if (item.id in canceled) signal.cancel()
                app.contentResolver.openFileDescriptor(target, "w", signal)?.use { output ->
                    check(item.id !in canceled) { "Transfer canceled" }
                    run(JSONObject().put("action", "export").put("entry", file.id), output = output.fd, job = item.id)
                    output.checkError()
                } ?: error("This Android destination is unavailable")
                file.name
            } catch (error: Exception) {
                target?.let { runCatching { DocumentsContract.deleteDocument(app.contentResolver, it) } }
                throw error
            }
        }
    }

    fun retry(item: TransferItem) {
        when (item.kind) {
            TransferKind.Import -> importUris(listOf(Uri.parse(item.source)), destination = item.destination)
            TransferKind.Save -> {
                pendingSave = listOf(GuestFile(item.source, item.name, false, item.total))
                runCatching { save?.invoke(Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
                    addCategory(Intent.CATEGORY_OPENABLE); type = mime(item.name); putExtra(Intent.EXTRA_TITLE, item.name)
                }) }.onFailure { message = "Android's save dialog is unavailable" }
            }

        }
    }

    fun cancel(item: TransferItem) {
        canceled.add(item.id)
        signals[item.id]?.cancel()
        if (item.id in running) nativeCancel(item.id)
        items = items.map { if (it.id == item.id) it.copy(phase = if (item.id in running) TransferPhase.Canceling else TransferPhase.Canceled, error = null) else it }
        persist()
        TransferService.sync(app, busy)
    }
    private fun enqueue(item: TransferItem, completed: () -> Unit = {}, operation: (TransferItem) -> String) {
        message = null
        items = items + item
        val durable = persist()
        TransferService.sync(app, true)
        work.execute {
            try { durable.get() } catch (error: Exception) {
                runCatching(completed); finish(item, null, IllegalStateException("Could not record this transfer. Free some storage and try again.", error)); return@execute
            }
            if (item.id in canceled) { runCatching(completed); finish(item, null, null); return@execute }
            running.add(item.id)
            update(item.id) { if (item.id in canceled) it else it.copy(phase = TransferPhase.Running) }
            var name: String? = null
            var failure: Exception? = null
            try {
                check(item.id !in canceled) { "Transfer canceled" }
                name = operation(item)
            } catch (error: Exception) { failure = error }
            finally { signals.remove(item.id); runCatching(completed) }
            // A cancel may arrive immediately after the helper atomically
            // commits. A successful operation still reports the completed file.
            finish(item, name, failure)
        }
    }

    private fun finish(item: TransferItem, name: String?, failure: Exception?) {
        running.remove(item.id)
        nativeForget(item.id)
        main.post {
            val phase = when {
                name != null -> TransferPhase.Done
                item.id in canceled -> TransferPhase.Canceled
                else -> TransferPhase.Failed
            }
            items = items.map { if (it.id == item.id) it.copy(name = name ?: it.name, phase = phase, error = when (failure) {
                is SecurityException -> "Android no longer allows access to this file. Choose it again."
                is java.io.FileNotFoundException -> "This file is unavailable. Choose it again."
                else -> failure?.message
            }) else it }
            canceled.remove(item.id)
            val durable = persist()
            item.permission?.let { permission -> work.execute {
                runCatching { durable.get() }.onSuccess { main.post { releaseSavePermission(permission) } }
            } }
            TransferService.sync(app, busy)
            if (item.kind == TransferKind.Import && phase == TransferPhase.Done && folder.id == item.destination) browse(folder.id, keepSelection = true)
        }
    }

    private fun update(id: String, transform: (TransferItem) -> TransferItem) {
        main.post { items = items.map { if (it.id == id) transform(it) else it } }
    }
    @JvmStatic fun onNativeProgress(id: String, bytes: Long, total: Long) {
        if (id in canceled) nativeCancel(id)
        update(id) { it.copy(bytes = bytes, total = total) }
    }
    private fun run(request: JSONObject, input: Int = -1, output: Int = -1, job: String = UUID.randomUUID().toString()): JSONObject {
        val result = JSONObject(nativeRun(request.toString(), input, output, job))
        check(result.optBoolean("ok")) { result.optString("error", "File transfer failed") }
        return result
    }

    private fun persist(): java.util.concurrent.Future<*> {
        val state = JSONArray()
        val retained = items.filter { it.phase in setOf(TransferPhase.Queued, TransferPhase.Running, TransferPhase.Canceling) }.map { it.id }.toSet() + items.takeLast(200).map { it.id }
        items.filter { it.id in retained }.forEach { item -> state.put(JSONObject().put("id", item.id).put("name", item.name)
            .put("kind", item.kind.name).put("phase", item.phase.name).put("source", item.source)
            .put("destination", item.destination).put("total", item.total)
            .put("output", item.output).put("permission", item.permission)) }
        val bytes = state.toString().toByteArray()
        return journal.submit {
            val atomic = AtomicFile(File(app.filesDir, "file-transfers.json"))
            var output: java.io.FileOutputStream? = null
            try { output = atomic.startWrite(); output.write(bytes); atomic.finishWrite(output) }
            catch (error: Exception) { output?.let(atomic::failWrite); throw error }
        }
    }

    private fun restore() {
        runCatching {
            val state = JSONArray(String(AtomicFile(File(app.filesDir, "file-transfers.json")).readFully()))
            items = (0 until state.length()).mapNotNull { index ->
                val value = state.getJSONObject(index)
                if (value.getString("kind") == "Prepare") return@mapNotNull null
                var phase = TransferPhase.valueOf(value.getString("phase"))
                if (phase in setOf(TransferPhase.Queued, TransferPhase.Running, TransferPhase.Canceling)) phase = TransferPhase.Interrupted
                TransferItem(value.getString("id"), value.getString("name"), TransferKind.valueOf(value.getString("kind")),
                    phase, total = value.optLong("total", -1), source = value.getString("source"),
                    destination = value.getString("destination"),
                    output = value.optString("output").takeUnless { it.isEmpty() || it == "null" },
                    permission = value.optString("permission").takeUnless { it.isEmpty() || it == "null" })
            }
            items.filter { it.phase == TransferPhase.Interrupted && it.kind == TransferKind.Import }.forEach { item ->
                work.execute { runCatching { run(JSONObject().put("action", "cleanup").put("directory", item.destination).put("job", item.id)) } }
            }
            val interruptedSaves = items.filter { it.phase == TransferPhase.Interrupted && it.kind == TransferKind.Save }
            val permissions = items.mapNotNull { it.permission }.toSet()
            work.execute {
                interruptedSaves.forEach { item ->
                    val output = item.output ?: item.destination.takeUnless { it.startsWith("tree:") }
                    output?.let { runCatching { DocumentsContract.deleteDocument(app.contentResolver, Uri.parse(it)) } }
                }
                main.post { permissions.forEach(::releaseSavePermission) }
            }

        }
    }

    private fun retainSavePermission(result: Intent?): String? {
        val uri = result?.data ?: return null
        if (result.flags and Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION == 0) return null
        val modes = result.flags and (Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
        return runCatching {
            app.contentResolver.takePersistableUriPermission(uri, modes)
            uri.toString()
        }.getOrNull()
    }

    private fun releaseSavePermission(permission: String) {
        if (items.any { it.permission == permission && it.phase in setOf(TransferPhase.Queued, TransferPhase.Running, TransferPhase.Canceling) }) return
        runCatching {
            val uri = Uri.parse(permission)
            val grant = app.contentResolver.persistedUriPermissions.firstOrNull { it.uri == uri } ?: return@runCatching
            val modes = (if (grant.isReadPermission) Intent.FLAG_GRANT_READ_URI_PERMISSION else 0) or
                (if (grant.isWritePermission) Intent.FLAG_GRANT_WRITE_URI_PERMISSION else 0)
            app.contentResolver.releasePersistableUriPermission(uri, modes)
        }
    }

    internal fun mime(name: String) = MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase()) ?: "application/octet-stream"
}
