package app.polarbear

// Keeps first setup alive while the user is in another app. Without a
// foreground service Android cuts a backgrounded app's network within a second
// and soon freezes it, which used to stall the download until the user came
// back and tapped Retry. The service owns no work: the native installer runs
// regardless, this only tells Android a user-started download is in progress
// and shows its progress in the notification shade.

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

class SetupService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        try {
            val notification = buildNotification(this, lastState)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
            } else {
                startForeground(NOTIFICATION_ID, notification)
            }
        } catch (e: Exception) {
            // Android refused (for example a start from the background on 12+).
            // Setup still runs while Portal is open; nothing else depends on it.
            Log.w(TAG, "setup foreground service refused", e)
            stopSelf()
            return START_NOT_STICKY
        }
        instance = this
        // Setup may have ended between startForegroundService() and now; only
        // after startForeground() may the service stop without crashing.
        if (!running) finish()
        // A killed process restarts setup itself when Portal opens again.
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        if (instance === this) instance = null
        super.onDestroy()
    }

    // Android 15 caps dataSync services at six hours a day.
    override fun onTimeout(startId: Int, fgsType: Int) {
        finish()
    }

    private fun finish() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            stopForeground(STOP_FOREGROUND_REMOVE)
        } else {
            @Suppress("DEPRECATION")
            stopForeground(true)
        }
        stopSelf()
    }

    companion object {
        private const val TAG = "PortalSetupService"
        private const val CHANNEL_ID = "portal-setup"
        private const val NOTIFICATION_ID = 1001

        @Volatile private var running = false
        @Volatile private var lastState: ComposeOverlay.InstallUiState? = null
        @Volatile private var lastShownPercent = -1
        @Volatile private var lastShownMessage = ""

        /** Follow the native install state: run while installing, stop otherwise. */
        fun sync(context: Context, state: ComposeOverlay.InstallUiState) {
            lastState = state
            val app = context.applicationContext
            if (state.running) {
                if (!running) {
                    running = true
                    lastShownPercent = state.progress
                    lastShownMessage = state.message
                    try {
                        ensureChannel(app)
                        val intent = Intent(app, SetupService::class.java)
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                            app.startForegroundService(intent)
                        } else {
                            app.startService(intent)
                        }
                    } catch (e: Exception) {
                        Log.w(TAG, "could not start the setup foreground service", e)
                        running = false
                    }
                } else if (state.progress != lastShownPercent || friendlyStatus(state) != lastShownMessage) {
                    // Only whole-percent or wording changes reach the shade.
                    lastShownPercent = state.progress
                    lastShownMessage = friendlyStatus(state)
                    notify(app, buildNotification(app, state))
                }
            } else if (running) {
                // Never stopService() here: a service that has not reached
                // startForeground() yet would crash the app. A started one is
                // stopped now; one still starting stops itself in onStartCommand.
                running = false
                mainHandler.post { if (!running) instance?.finish() }
            }
        }

        @Volatile private var instance: SetupService? = null
        private val mainHandler = Handler(Looper.getMainLooper())

        private fun ensureChannel(context: Context) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
            val manager = context.getSystemService(NotificationManager::class.java) ?: return
            if (manager.getNotificationChannel(CHANNEL_ID) != null) return
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL_ID, "Setup", NotificationManager.IMPORTANCE_LOW).apply {
                    description = "Progress while Portal installs"
                    setShowBadge(false)
                },
            )
        }

        private fun notify(context: Context, notification: Notification) {
            try {
                context.getSystemService(NotificationManager::class.java)?.notify(NOTIFICATION_ID, notification)
            } catch (e: Exception) {
                // No notification permission: the service still runs.
                Log.d(TAG, "setup notification not shown", e)
            }
        }

        private fun friendlyStatus(state: ComposeOverlay.InstallUiState?): String {
            if (state == null) return "Getting ready…"
            return app.polarbear.setup.installStep(state.progress, state.message).detail
        }

        private fun buildNotification(context: Context, state: ComposeOverlay.InstallUiState?): Notification {
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
                Notification.Builder(context)
            }
            val percent = state?.progress ?: 0
            return builder
                .setSmallIcon(context.applicationInfo.icon)
                .setContentTitle("Installing Portal · $percent%")
                .setContentText(friendlyStatus(state))
                .setProgress(100, percent, state == null)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setShowWhen(false)
                .setContentIntent(open)
                .setCategory(Notification.CATEGORY_PROGRESS)
                .build()
        }
    }
}
