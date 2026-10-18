package app.redline.work

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import android.support.v4.media.session.MediaSessionCompat
import app.redline.MainActivity
import app.redline.R
import app.redline.core.AnnouncementRecord
import app.redline.core.JobRecord
import app.redline.core.JobState
import app.redline.core.PlayerSnapshotRecord
import app.redline.core.PlayerStatus

/** The notification channels and what goes in them. */
object Notifications {
    /** The ongoing "downloading..." notification of [DownloadService]. Quiet. */
    const val CHANNEL_PROGRESS = "progress"
    /** "Download finished" and the like, as the desktop announces them. */
    const val CHANNEL_FINISHED = "finished"

    /** The player's MediaStyle notification ([PlaybackService]). Quiet. */
    const val CHANNEL_PLAYBACK = "playback"

    const val ID_PROGRESS = 1
    private const val ID_FINISHED = 2
    const val ID_PLAYBACK = 3

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
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_PLAYBACK, "Music playback", NotificationManager.IMPORTANCE_LOW)
                .apply { setShowBadge(false) },
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

    /** Opens the app with the full player showing. */
    fun openPlayer(context: Context): PendingIntent =
        PendingIntent.getActivity(
            context,
            1,
            Intent(context, MainActivity::class.java)
                .putExtra(MainActivity.EXTRA_OPEN_PLAYER, true)
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

    /** A transport button: sends [action] to [PlaybackService]. */
    private fun playbackAction(context: Context, action: String, code: Int): PendingIntent =
        PendingIntent.getService(
            context,
            code,
            Intent(context, PlaybackService::class.java).setAction(action),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

    /**
     * The player's notification: art, title and artist, previous / play-pause / next. On
     * Android 13+ the system draws its media controls from the session instead.
     */
    @Suppress("DEPRECATION") // MediaSessionCompat and MediaStyle: see PlaybackService.
    fun playback(
        context: Context,
        session: MediaSessionCompat.Token,
        snapshot: PlayerSnapshotRecord?,
        art: Bitmap?,
    ): android.app.Notification {
        val playing = snapshot?.isPlaying == true
        val line = when {
            snapshot == null -> null
            snapshot.status == PlayerStatus.ERROR -> snapshot.error?.lineSequence()?.firstOrNull()
            snapshot.status == PlayerStatus.LOADING -> "Loading…"
            snapshot.buffering && playing -> "Buffering…"
            else -> snapshot.artist
        }
        return NotificationCompat.Builder(context, CHANNEL_PLAYBACK)
            .setSmallIcon(R.drawable.ic_stat_redline)
            .setColor(ACCENT)
            .setContentTitle(snapshot?.title ?: "Redline")
            .setContentText(line)
            .setLargeIcon(art)
            .setContentIntent(openPlayer(context))
            .setOngoing(playing)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setShowWhen(false)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setCategory(NotificationCompat.CATEGORY_TRANSPORT)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .addAction(android.R.drawable.ic_media_previous, "Previous", playbackAction(context, PlaybackService.ACTION_PREV, 10))
            .addAction(
                if (playing) android.R.drawable.ic_media_pause else android.R.drawable.ic_media_play,
                if (playing) "Pause" else "Play",
                playbackAction(context, PlaybackService.ACTION_TOGGLE, 11),
            )
            .addAction(android.R.drawable.ic_media_next, "Next", playbackAction(context, PlaybackService.ACTION_NEXT, 12))
            .setStyle(
                androidx.media.app.NotificationCompat.MediaStyle()
                    .setMediaSession(session)
                    .setShowActionsInCompactView(0, 1, 2),
            )
            .build()
    }

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
