package app.polarbear

import android.content.ClipboardManager
import android.content.Context
import android.util.Log

/**
 * Tells native code when Android's primary clip changes, so a copy made
 * while Portal stays focused (a heads-up notification's Copy button, a
 * cross-device clipboard, a floating window) reaches the guest without
 * polling. Android only delivers this to the focused app; resume and focus
 * regain are covered by the native resync (src/android/clipboard.rs).
 */
object ClipboardWatch {
    private const val TAG = "PortalClipboard"

    @JvmStatic private external fun nativeClipChanged()

    private var attached = false

    fun attach(context: Context) {
        if (attached) return
        val manager = context.applicationContext.getSystemService(ClipboardManager::class.java) ?: return
        manager.addPrimaryClipChangedListener {
            try {
                nativeClipChanged()
            } catch (e: UnsatisfiedLinkError) {
                Log.w(TAG, "native clipboard bridge unavailable", e)
            }
        }
        attached = true
    }
}
