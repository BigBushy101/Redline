package app.redline.work

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import app.redline.MainActivity
import app.redline.R
import app.redline.core.AnnouncementRecord
import app.redline.core.JobRecord
import app.redline.core.JobState

/** The two notification channels and what goes in them. */
object Notifications {
    /** The ongoing "downloading..." notification of [DownloadService]. Quiet. */
    const val CHANNEL_PROGRESS = "progress"
    /** "Download finished" and the like, as the desktop announces them. */
    const val CHANNEL_FINISHED = "finished"

    const val ID_PROGRESS = 1
    private const val ID_FINISHED = 2

    private val ACCENT = 0xFFE8263A.toInt()

    fun createChannels(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val nm = context.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_PROGRESS, "Downloads in progress", NotificationManager.IMPORTANCE_LOW)
                .apply { setShowBadge(false) },
        )
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_FINISHED, "Finished downloads", NotificationManager.IMPORTANCE_DEFAULT),
        )
    }

    fun canPost(context: Context): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED

    /** Opens the app on the Queue page. */
    private fun openQueue(context: Context): PendingIntent =
        PendingIntent.getActivity(
            context,
            0,
            Intent(context, MainActivity::class.java)
                .putExtra(MainActivity.EXTRA_OPEN_QUEUE, true)
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

    /** The service's notification: the first running job with its progress, and how many wait. */
    fun progress(context: Context, queue: List<JobRecord>, active: Int): android.app.Notification {
        val job = queue.lastOrNull { it.state == JobState.RUNNING || it.state == JobState.PROCESSING }
        val builder = NotificationCompat.Builder(context, CHANNEL_PROGRESS)
            .setSmallIcon(R.drawable.ic_stat_redline)
            .setColor(ACCENT)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setCategory(NotificationCompat.CATEGORY_PROGRESS)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .setContentIntent(openQueue(context))
        if (job == null) {
            // Only queued (or scheduled) jobs left.
            builder.setContentTitle("Waiting to start")
                .setContentText("$active in the queue")
        } else {
            val others = active - 1
            builder.setContentTitle(job.title)
                .setContentText(job.detail.ifBlank { job.subtitle })
                .setSubText(if (others > 0) "+$others more" else null)
            val p = job.progress
            if (job.state == JobState.RUNNING && p != null) {
                builder.setProgress(1000, (p * 1000).toInt().coerceIn(0, 1000), false)
            } else {
                builder.setProgress(0, 0, true)
            }
        }
        return builder.build()
    }

    /** The desktop's "jobs finished" announcement. */
    fun finished(context: Context, announcement: AnnouncementRecord) {
        if (!canPost(context)) return
        val n = NotificationCompat.Builder(context, CHANNEL_FINISHED)
            .setSmallIcon(R.drawable.ic_stat_redline)
            .setColor(ACCENT)
            .setContentTitle(announcement.title)
            .setContentText(announcement.body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(announcement.body))
            .setAutoCancel(true)
            .setContentIntent(openQueue(context))
            .build()
        try {
            NotificationManagerCompat.from(context).notify(ID_FINISHED, n)
        } catch (_: SecurityException) {
            // Permission revoked between the check and the post.
        }
    }
}
