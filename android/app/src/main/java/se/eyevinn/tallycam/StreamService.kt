package se.eyevinn.tallycam

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

/**
 * Keeps the app in the foreground while it streams, so camera, microphone
 * and network keep running with the screen off or another app in front. The
 * stream itself lives in [AppModel]; this only holds the notification.
 */
class StreamService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Streaming", NotificationManager.IMPORTANCE_LOW)
        )
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java),
                                             PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val text = intent?.getStringExtra(EXTRA_TEXT) ?: "Live"
        val notification = Notification.Builder(this, CHANNEL)
            .setContentTitle("TallyCam")
            .setContentText(text)
            .setSmallIcon(android.R.drawable.presence_video_online)
            .setContentIntent(open)
            .setOngoing(true)
            .build()
        val types = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA or ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
        } else 0
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) startForeground(ID, notification, types)
        else startForeground(ID, notification)
        return START_NOT_STICKY
    }

    companion object {
        private const val CHANNEL = "stream"
        private const val ID = 1
        private const val EXTRA_TEXT = "text"

        fun start(context: Context, text: String) {
            context.startForegroundService(Intent(context, StreamService::class.java).putExtra(EXTRA_TEXT, text))
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, StreamService::class.java))
        }
    }
}

/** The one AppModel of the process: the activity comes and goes, the stream stays. */
object TallyCam {
    @Volatile private var instance: AppModel? = null

    fun model(context: Context): AppModel =
        instance ?: synchronized(this) { instance ?: AppModel(context.applicationContext).also { instance = it } }
}
