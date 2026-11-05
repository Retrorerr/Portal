package app.polarbear

// Bug and compatibility reports. Portal uploads nothing itself: it fills a
// GitHub issue form with the device summary the person has just seen, and they
// decide whether to post it. The field ids match .github/ISSUE_TEMPLATE, and
// tests/report_privacy.rs holds the two together.

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.media.MediaCodecList
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.widget.Toast
import kotlin.concurrent.thread

object PortalReport {
    private const val TAG = "PortalReport"
    private const val NEW_ISSUE = "https://github.com/Retrorerr/Portal/issues/new"
    const val BUG_FORM = "bug-report.yml"
    const val COMPATIBILITY_FORM = "compatibility-report.yml"
    // GitHub answers 414 from about 8 KB; encoding roughly doubles this text.
    private const val MAX_PREFILLED_CHARS = 2500

    @JvmStatic private external fun nativeSummary(): String?

    /** Everything a report carries about the device. Blocking: off the main thread. */
    fun summary(context: Context): String {
        val native = try {
            nativeSummary().orEmpty()
        } catch (error: UnsatisfiedLinkError) {
            Log.w(TAG, "native summary unavailable", error)
            ""
        }
        return "package=${context.packageName}\n$native${hardwareCodecs()}"
    }

    /** Which video codecs this chip has in hardware. Also called from native for the archive. */
    @JvmStatic fun hardwareCodecs(): String {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return "hw_codecs=unknown (before Android 10)"
        return try {
            val decoders = sortedMapOf<String, String>()
            val encoders = sortedMapOf<String, String>()
            for (codec in MediaCodecList(MediaCodecList.ALL_CODECS).codecInfos) {
                if (!codec.isHardwareAccelerated || codec.name.endsWith(".secure")) continue
                for (type in codec.supportedTypes) {
                    if (!type.startsWith("video/")) continue
                    val video = codec.getCapabilitiesForType(type).videoCapabilities ?: continue
                    val size = "${video.supportedWidths.upper}x${video.supportedHeights.upper}"
                    (if (codec.isEncoder) encoders else decoders)[type.substringAfterLast('/').substringAfterLast('.')] = size
                }
            }
            fun line(codecs: Map<String, String>) =
                codecs.entries.joinToString { "${it.key} ${it.value}" }.ifEmpty { "none" }
            "hw_decoders=${line(decoders)}\nhw_encoders=${line(encoders)}"
        } catch (error: RuntimeException) {
            "hw_codecs=unavailable (${error.javaClass.simpleName})"
        }
    }

    /** Open the form in a browser, filled in. False if nothing can open it. */
    fun openForm(context: Context, form: String, summary: String, evidence: String? = null): Boolean {
        val uri = Uri.parse(NEW_ISSUE).buildUpon()
            .appendQueryParameter("template", form)
            .appendQueryParameter("device-info", summary.take(MAX_PREFILLED_CHARS))
        if (form == COMPATIBILITY_FORM) {
            uri.appendQueryParameter("title", "[Compatibility]: ${Build.MANUFACTURER} ${Build.MODEL}")
        }
        if (!evidence.isNullOrBlank()) {
            uri.appendQueryParameter("evidence", "Portal showed:\n${evidence.take(MAX_PREFILLED_CHARS / 2)}")
        }
        val view = Intent(Intent.ACTION_VIEW, uri.build()).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        // The GitHub app claims these links and drops the filled-in fields, so
        // ask for whatever handles any web page: the person's browser.
        val browser = Intent(view).apply {
            selector = Intent(Intent.ACTION_VIEW, Uri.parse("https://example.com")).addCategory(Intent.CATEGORY_BROWSABLE)
        }
        for (intent in listOf(browser, view)) {
            try {
                context.startActivity(intent)
                return true
            } catch (error: RuntimeException) {
                Log.w(TAG, "could not open the report form", error)
            }
        }
        return false
    }

    /** For the screens shown when things went wrong: straight to a filled bug form. Main thread. */
    fun reportFailure(context: Context, reason: String?) {
        val app = context.applicationContext
        thread(name = "portal-report") {
            val summary = summary(app)
            Handler(Looper.getMainLooper()).post {
                if (!openForm(context, BUG_FORM, summary, reason)) {
                    copy(app, summary)
                    Toast.makeText(app, "No browser found. Device details copied.", Toast.LENGTH_LONG).show()
                }
            }
        }
    }

    fun copy(context: Context, text: String) {
        val clipboard = context.getSystemService(ClipboardManager::class.java) ?: return
        clipboard.setPrimaryClip(ClipData.newPlainText("Portal device details", text))
    }
}
