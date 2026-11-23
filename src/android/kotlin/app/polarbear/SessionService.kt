package app.polarbear

import android.app.ActivityManager
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
import android.os.HandlerThread
import android.os.IBinder
import android.os.Looper
import android.os.Process
import android.util.Log
import android.view.Display
import java.io.File
import java.io.IOException

/**
 * The native desktop is ongoing user work even when its SurfaceView is suspended. Without this
 * service Android classifies the entire guest as cached and may freeze it or tear down its TCP
 * connections. This service also reports sustained device memory pressure during the session.
 */
class SessionService : Service() {
    private var memoryMonitor: HandlerThread? = null
    private var memoryWarning = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        try {
            val notification = notification(this, memoryWarning)
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
        if (instance === this) startMemoryMonitor()
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        if (instance === this) instance = null
        startPending = false
        memoryMonitor?.quitSafely()
        memoryMonitor = null
        super.onDestroy()
    }

    private fun startMemoryMonitor() {
        if (memoryMonitor != null) return
        val manager = getSystemService(Context.ACTIVITY_SERVICE) as? ActivityManager ?: return
        val worker = HandlerThread("Portal memory check", Process.THREAD_PRIORITY_BACKGROUND)
        worker.start()
        memoryMonitor = worker
        val handler = Handler(worker.looper)
        handler.post(object : Runnable {
            private val info = ActivityManager.MemoryInfo()
            private val pressure = SessionMemoryPressure()

            override fun run() {
                sampleMemory()
                handler.postDelayed(this, MEMORY_SAMPLE_MS)
            }

            private fun sampleMemory() {
                try {
                    manager.getMemoryInfo(info)
                } catch (error: RuntimeException) {
                    Log.w(TAG, "Could not read device memory info", error)
                    return
                }
                val swap = try {
                    File("/proc/meminfo").useLines { SessionMemoryPressure.readSwap(it) }
                } catch (_: IOException) {
                    null
                } catch (_: SecurityException) {
                    null
                }
                val low = SessionMemoryPressure.isLow(info.lowMemory, info.availMem, info.totalMem, swap)
                if (pressure.update(low)) {
                    val availableMiB = info.availMem / (1024 * 1024)
                    Log.i(
                        TAG,
                        "Device memory pressure changed: warning=$low systemLow=${info.lowMemory} availableMiB=$availableMiB swap=$swap",
                    )
                    main.post {
                        if (instance === this@SessionService && memoryMonitor === worker) {
                            memoryWarning = low
                            refreshNotification()
                        }
                    }
                }
            }
        })
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
        private const val MEMORY_SAMPLE_MS = 5_000L
        private val ownership = SessionServiceOwnership()
        private val main = Handler(Looper.getMainLooper())
        private var instance: SessionService? = null
        private var startPending = false

        private fun isActive(): Boolean = synchronized(ownership) { ownership.active }

        /** UI thread: expose the tablet handle whenever a running desktop moves away. */
        internal fun refreshNotification() {
            val service = instance ?: return
            val manager = service.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            manager.notify(NOTIFICATION_ID, notification(service, service.memoryWarning))
        }

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

        private fun notification(context: Context, memoryWarning: Boolean): Notification {
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
            // On another display, the notification is the tablet's way back to it.
            val projected = DesktopControlActivity.projectedDesktop()
            val display = projected?.desktopDisplay()
            var contentText = "Tap to return to Portal"
            var content = open
            if (projected != null && display != null) {
                val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
                val tablet = DesktopControlActivity.tabletOptions()
                fun control(take: Boolean) = PendingIntent.getActivity(
                    context, if (take) 1 else 2, DesktopControlActivity.controlIntent(context, take), flags, tablet,
                )
                val back = PendingIntent.getActivity(
                    context, 3, PortalDisplays.returnIntent(context), flags,
                    PortalDisplays.displayOptions(Display.DEFAULT_DISPLAY),
                )
                builder.addAction(Notification.Action.Builder(null, "Control desktop", control(true)).build())
                builder.addAction(Notification.Action.Builder(null, "Return to ${deviceNoun(context)}", back).build())
                contentText = "On ${projected.portalDisplays.displayName(display)}"
                // A phone presenting the desktop opens its touchpad page.
                content = if (projected.presentedDisplay != null) open else control(false)
            }
            var title = "Your desktop is running"
            if (memoryWarning) {
                title = "Low device memory"
                contentText = "Close unused apps to free memory."
                builder.setStyle(
                    Notification.BigTextStyle().bigText(
                        "$contentText For Windows games, try lowering Video memory in Portal’s Game settings " +
                            "and restarting the game.",
                    ),
                )
            }
            return builder
                .setSmallIcon(context.applicationInfo.icon)
                .setContentTitle(title)
                .setContentText(contentText)
                .setContentIntent(content)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setShowWhen(false)
                .setCategory(Notification.CATEGORY_SERVICE)
                .build()
        }
    }
}
