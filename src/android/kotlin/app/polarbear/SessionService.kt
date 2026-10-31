package app.polarbear

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log

/**
 * The native desktop is ongoing user work even when its SurfaceView is suspended. Without this
 * service Android classifies the entire guest as cached and may freeze it or tear down its TCP
 * connections. This service owns no guest work, wake lock, or automatic session restart.
 */
class SessionService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        try {
            val notification = notification(this)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
            } else {
                startForeground(NOTIFICATION_ID, notification)
            }
        } catch (error: Exception) {
            Log.w(TAG, "desktop foreground service refused", error)
            startPending = false
            stopSelf()
            return START_NOT_STICKY
        }
        instance = this
        startPending = false
        Log.i(TAG, "desktop foreground protection active (owner=${synchronized(ownership) { ownership.owner }})")
        // A launch can fail or end while Android is still creating its service. It must first
        // acknowledge startForegroundService(), then stop, rather than cancel its pending start.
        finishIfInactive()
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        if (instance === this) instance = null
        startPending = false
        super.onDestroy()
    }

    private fun finish() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            stopForeground(STOP_FOREGROUND_REMOVE)
        } else {
            @Suppress("DEPRECATION")
            stopForeground(true)
        }
        if (instance === this) instance = null
        Log.i(TAG, "desktop foreground protection released")
        stopSelf()
    }

    private fun finishIfInactive() {
        // Native ownership changes can arrive off the main thread. Do not let an old reconcile
        // stop the foreground service after a replacement desktop has already claimed it.
        synchronized(ownership) {
            if (!ownership.active) finish()
        }
    }

    companion object {
        private const val TAG = "PortalSessionService"
        private const val CHANNEL_ID = "portal-desktop"
        private const val NOTIFICATION_ID = 1003
        private val ownership = SessionServiceOwnership()
        private val main = Handler(Looper.getMainLooper())
        private var instance: SessionService? = null
        private var startPending = false

        private fun isActive(): Boolean = synchronized(ownership) { ownership.active }

        /** Called at the native launch boundary, before guest setup/launch work can background. */
        fun setActive(context: Context, owner: Long, active: Boolean) {
            val changed = synchronized(ownership) { ownership.update(owner, active) }
            if (changed) sync(context)
        }

        /** Retry a refused background start as soon as the Activity becomes eligible again. */
        fun sync(context: Context) {
            val app = context.applicationContext
            val reconcile = Runnable {
                if (!isActive()) {
                    instance?.finishIfInactive()
                } else if (instance == null && !startPending) {
                    try {
                        ensureChannel(app)
                        startPending = true
                        val intent = Intent(app, SessionService::class.java)
                        val started = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                            app.startForegroundService(intent)
                        } else {
                            app.startService(intent)
                        }
                        checkNotNull(started) { "Android could not resolve Portal's desktop session service" }
                    } catch (error: Exception) {
                        startPending = false
                        Log.w(TAG, "could not protect the running desktop", error)
                    }
                }
            }
            if (Looper.myLooper() == Looper.getMainLooper()) reconcile.run() else main.post(reconcile)
        }

        /** Activity destruction ends Portal's process, so a fresh desktop must not be restarted. */
        fun endProcess(context: Context) {
            synchronized(ownership) { ownership.update(ownership.owner, false) }
            sync(context)
        }

        private fun ensureChannel(context: Context) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
            val manager = context.getSystemService(NotificationManager::class.java) ?: return
            if (manager.getNotificationChannel(CHANNEL_ID) != null) return
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL_ID, "Desktop", NotificationManager.IMPORTANCE_LOW).apply {
                    description = "Keeps your running Linux desktop available in the background"
                    setShowBadge(false)
                },
            )
        }

        private fun notification(context: Context): Notification {
            val open = PendingIntent.getActivity(
                context,
                0,
                Intent(context, PortalActivity::class.java)
                    .setFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
            val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                Notification.Builder(context, CHANNEL_ID)
            } else {
                @Suppress("DEPRECATION")
                Notification.Builder(context).setPriority(Notification.PRIORITY_LOW)
            }
            return builder
                .setSmallIcon(context.applicationInfo.icon)
                .setContentTitle("Your desktop is running")
                .setContentText("Tap to return to Portal")
                .setContentIntent(open)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setShowWhen(false)
                .setCategory(Notification.CATEGORY_SERVICE)
                .build()
        }
    }
}
