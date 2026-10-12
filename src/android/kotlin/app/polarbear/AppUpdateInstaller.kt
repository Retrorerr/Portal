package app.polarbear

// Portal's self-update through Android's PackageInstaller. Native code
// downloads the release APK and checks its SHA-256 against GitHub's record;
// this confirms it is really a newer Portal signed with the same key before
// Android's own checks, then shows Android's confirmation. When Android
// replaces the app, Portal's process ends; the result only comes back when
// the user cancels or the install fails.

import android.app.Activity
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageInfo
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.content.pm.Signature
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.util.Log
import androidx.core.content.ContextCompat
import androidx.core.content.IntentCompat
import java.io.File
import java.lang.ref.WeakReference
import java.util.concurrent.Executors

object AppUpdateInstaller {
    private const val TAG = "PortalAppUpdate"

    const val RESULT_INSTALLED = 0
    const val RESULT_CANCELLED = 1
    const val RESULT_NEEDS_PERMISSION = 2
    const val RESULT_FAILED = 3

    /**
     * Some Android versions send nothing when the confirmation is dismissed.
     * Once Portal is back in front, a session still unanswered after this is
     * treated as cancelled; a confirmed install ends the process well before.
     */
    private const val CONFIRMATION_GRACE_MS = 10_000L

    private val worker = Executors.newSingleThreadExecutor()
    private val main = Handler(Looper.getMainLooper())
    private val lock = Any()
    private var receiverRegistered = false
    private var report: ((Int, String) -> Unit)? = null
    /** The session whose result native code is waiting for. */
    private var activeSession = -1
    /** Android's confirmation, held until Portal is in front to show it. */
    private var pendingConfirmation: Intent? = null
    private var confirmationShown = false
    @Volatile private var resumed: WeakReference<Activity>? = null

    /** Whether Android lets Portal install apps ("Install unknown apps"). */
    @JvmStatic fun canInstall(context: Context): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.O ||
            context.packageManager.canRequestPackageInstalls()

    /** Android's "Install unknown apps" page for Portal. */
    @JvmStatic fun openInstallPermissionSettings(context: Context): Boolean = try {
        val intent = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${context.packageName}"))
        } else {
            Intent(Settings.ACTION_SECURITY_SETTINGS)
        }
        if (context !is Activity) intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        context.startActivity(intent)
        true
    } catch (e: Exception) {
        Log.e(TAG, "could not open the install permission page", e)
        false
    }

    @JvmStatic fun onActivityResumed(activity: Activity) {
        resumed = WeakReference(activity)
        val waiting = synchronized(lock) {
            if (pendingConfirmation != null) {
                showConfirmation(activity)
                null
            } else {
                activeSession.takeIf { confirmationShown && it >= 0 }
            }
        }
        if (waiting != null) {
            main.postDelayed({ expireUnanswered(activity, waiting) }, CONFIRMATION_GRACE_MS)
        }
    }

    @JvmStatic fun onActivityPaused(activity: Activity) {
        if (resumed?.get() === activity) resumed = null
    }

    /**
     * Start installing [path]. Returns false only when nothing was started;
     * otherwise [onResult] is called once (unless Android ends Portal first).
     */
    @JvmStatic fun install(context: Context, path: String, onResult: (Int, String) -> Unit): Boolean {
        val app = context.applicationContext
        synchronized(lock) {
            if (activeSession >= 0) {
                Log.w(TAG, "an install session is already open")
                return false
            }
            report = onResult
            activeSession = Int.MAX_VALUE // reserved until the session exists
            pendingConfirmation = null
            confirmationShown = false
        }
        worker.execute {
            try {
                start(app, File(path))
            } catch (e: Exception) {
                Log.e(TAG, "app update failed", e)
                finish(RESULT_FAILED, e.message ?: e.javaClass.simpleName)
            }
        }
        return true
    }

    private fun start(context: Context, apk: File) {
        if (!apk.isFile) return finish(RESULT_FAILED, "The downloaded update is missing")
        verifyArchive(context, apk)?.let { return finish(RESULT_FAILED, it) }
        if (!canInstall(context)) {
            return finish(RESULT_NEEDS_PERMISSION, "Allow Portal to install apps in Android settings")
        }
        registerReceiver(context)

        val installer = context.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(context.packageName)
            setSize(apk.length())
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                setInstallReason(PackageManager.INSTALL_REASON_USER)
            }
            // No package source: Android 13 restricts sensitive settings
            // (Portal's accessibility service) for apps marked as downloaded.
        }
        val sessionId = installer.createSession(params)
        synchronized(lock) { activeSession = sessionId }
        try {
            installer.openSession(sessionId).use { session ->
                apk.inputStream().use { input ->
                    session.openWrite("Portal.apk", 0, apk.length()).use { output ->
                        input.copyTo(output, 1 shl 16)
                        session.fsync(output)
                    }
                }
                val intent = Intent(statusAction(context)).setPackage(context.packageName)
                val flags = android.app.PendingIntent.FLAG_UPDATE_CURRENT or
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                        android.app.PendingIntent.FLAG_MUTABLE
                    } else {
                        0
                    }
                val pending = android.app.PendingIntent.getBroadcast(context, sessionId, intent, flags)
                session.commit(pending.intentSender)
            }
            Log.i(TAG, "install session $sessionId committed")
        } catch (e: Exception) {
            runCatching { installer.abandonSession(sessionId) }
            throw e
        }
    }

    /** Null when [apk] is a newer Portal signed like this one; else why not. */
    private fun verifyArchive(context: Context, apk: File): String? {
        val pm = context.packageManager
        val signingFlag = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            PackageManager.GET_SIGNING_CERTIFICATES
        } else {
            @Suppress("DEPRECATION")
            PackageManager.GET_SIGNATURES
        }
        val archive = pm.getPackageArchiveInfo(apk.path, signingFlag)
            ?: return "Android could not read the downloaded update"
        if (archive.packageName != context.packageName) {
            return "The download is ${archive.packageName}, not this Portal"
        }
        val installed = pm.getPackageInfo(context.packageName, signingFlag)
        if (versionCode(archive) <= versionCode(installed)) {
            return "The download is not newer than this Portal"
        }
        val ours = signers(installed)
        val theirs = signers(archive)
        if (theirs.isEmpty()) {
            // Android refuses a differently signed update anyway.
            Log.w(TAG, "could not read the update's signers; leaving the check to Android")
        } else if (ours.isNotEmpty() && ours.intersect(theirs).isEmpty()) {
            return "The download is not signed like this Portal"
        }
        return null
    }

    private fun versionCode(info: PackageInfo): Long =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            info.longVersionCode
        } else {
            @Suppress("DEPRECATION")
            info.versionCode.toLong()
        }

    /** Every certificate in the signing lineage, so a rotated key still matches. */
    private fun signers(info: PackageInfo): Set<String> {
        val signatures: Array<Signature>? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            val signing = info.signingInfo ?: return emptySet()
            if (signing.hasMultipleSigners()) {
                signing.apkContentsSigners
            } else {
                signing.signingCertificateHistory
            }
        } else {
            @Suppress("DEPRECATION")
            info.signatures
        }
        return signatures.orEmpty().map { it.toCharsString() }.toSet()
    }

    private fun statusAction(context: Context) = "${context.packageName}.APP_UPDATE_STATUS"

    private fun registerReceiver(context: Context) {
        synchronized(lock) {
            if (receiverRegistered) return
            receiverRegistered = true
        }
        ContextCompat.registerReceiver(
            context,
            object : BroadcastReceiver() {
                override fun onReceive(context: Context, intent: Intent) = onStatus(intent)
            },
            IntentFilter(statusAction(context)),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
    }

    private fun onStatus(intent: Intent) {
        val session = intent.getIntExtra(PackageInstaller.EXTRA_SESSION_ID, -1)
        val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)
        val message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE).orEmpty()
        Log.i(TAG, "session $session status $status $message")
        synchronized(lock) {
            if (session != activeSession) return
        }
        when (status) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                val confirm = IntentCompat.getParcelableExtra(intent, Intent.EXTRA_INTENT, Intent::class.java)
                    ?: return finish(RESULT_FAILED, "Android's installer did not ask for confirmation")
                synchronized(lock) {
                    pendingConfirmation = confirm
                    resumed?.get()?.let { showConfirmation(it) }
                }
            }
            PackageInstaller.STATUS_SUCCESS -> finish(RESULT_INSTALLED, "")
            PackageInstaller.STATUS_FAILURE_ABORTED -> finish(RESULT_CANCELLED, "")
            else -> finish(RESULT_FAILED, describe(status, message))
        }
    }

    /** Caller holds [lock]. */
    private fun showConfirmation(activity: Activity) {
        val confirm = pendingConfirmation ?: return
        pendingConfirmation = null
        confirmationShown = true
        activity.runOnUiThread {
            try {
                activity.startActivity(confirm)
            } catch (e: Exception) {
                Log.e(TAG, "could not show Android's install confirmation", e)
                finish(RESULT_FAILED, "Android's installer could not open")
            }
        }
    }

    private fun expireUnanswered(activity: Activity, session: Int) {
        val expired = synchronized(lock) {
            activeSession == session && confirmationShown && pendingConfirmation == null &&
                resumed?.get() === activity
        }
        if (!expired) return
        Log.i(TAG, "session $session left unanswered; treating it as cancelled")
        runCatching { activity.packageManager.packageInstaller.abandonSession(session) }
        finish(RESULT_CANCELLED, "")
    }

    private fun describe(status: Int, message: String): String {
        val reason = when (status) {
            PackageInstaller.STATUS_FAILURE_BLOCKED -> "Android blocked the update"
            PackageInstaller.STATUS_FAILURE_CONFLICT -> "The update conflicts with the installed Portal"
            PackageInstaller.STATUS_FAILURE_INCOMPATIBLE -> "The update is not compatible with this device"
            PackageInstaller.STATUS_FAILURE_INVALID -> "The update file is invalid"
            PackageInstaller.STATUS_FAILURE_STORAGE -> "Not enough storage to install the update"
            else -> "Android could not install the update"
        }
        return if (message.isBlank()) reason else "$reason ($message)"
    }

    private fun finish(code: Int, message: String) {
        val callback = synchronized(lock) {
            if (activeSession < 0) return
            activeSession = -1
            pendingConfirmation = null
            confirmationShown = false
            report.also { report = null }
        }
        Log.i(TAG, "install finished: $code $message")
        callback?.invoke(code, message)
    }
}
