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
import android.os.IBinder

/** Holds user-started transfers alive when Android's picker takes focus. */
class TransferService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val notification = notification(this)
        try {
            if (Build.VERSION.SDK_INT >= 29) startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
            else startForeground(ID, notification)
            instance = this
            if (!wanted) finish()
        } catch (_: Exception) { stopSelf() }
        return START_NOT_STICKY
    }
    override fun onDestroy() { if (instance === this) instance = null; super.onDestroy() }
    override fun onTimeout(startId: Int, fgsType: Int) {
        FileTransfers.items.filter { it.phase in setOf(TransferPhase.Queued, TransferPhase.Running, TransferPhase.Canceling) }.forEach(FileTransfers::cancel)
        finish()
    }
    private fun finish() {
        if (Build.VERSION.SDK_INT >= 24) stopForeground(STOP_FOREGROUND_REMOVE)
        else { @Suppress("DEPRECATION") stopForeground(true) }
        stopSelf()
    }
    companion object {
        private const val ID = 1002
        private const val CHANNEL = "portal-files"
        private var wanted = false
        private var instance: TransferService? = null
        fun sync(context: Context, running: Boolean) {
            val wasWanted = wanted
            wanted = running
            if (!running) { instance?.finish(); return }
            if (wasWanted) return
            try {
                val intent = Intent(context, TransferService::class.java)
                if (Build.VERSION.SDK_INT >= 26) context.startForegroundService(intent) else context.startService(intent)
            } catch (_: Exception) { wanted = false }
        }
        private fun notification(context: Context): Notification {
            if (Build.VERSION.SDK_INT >= 26) context.getSystemService(NotificationManager::class.java)
                ?.createNotificationChannel(NotificationChannel(CHANNEL, "File transfers", NotificationManager.IMPORTANCE_LOW))
            val open = Intent(context, PortalActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
                .putExtra("portal_open_files", true)
            val pending = PendingIntent.getActivity(context, ID, open, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
            @Suppress("DEPRECATION")
            val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(context, CHANNEL) else Notification.Builder(context)
            return builder.setSmallIcon(android.R.drawable.stat_sys_upload)
                .setContentTitle("Transferring files").setContentText("Tap to open Portal Files")
                .setContentIntent(pending).setOngoing(true).setProgress(0, 0, true).build()
        }
    }
}
