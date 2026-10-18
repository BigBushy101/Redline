package app.redline.work

import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import app.redline.RedlineApp
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch

/**
 * Keeps the process (and so the Rust queue and the yt-dlp/ffmpeg it runs) alive while jobs
 * are queued or running, with an ongoing notification showing the current one. The engine does
 * the work; this only exists so Android doesn't kill it in the background. Stops itself when
 * the queue drains.
 */
class DownloadService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var wakeLock: PowerManager.WakeLock? = null
    private var watching = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        running = true
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val state = (application as RedlineApp).state
        // startForegroundService needs startForeground within seconds, on every start.
        val first = Notifications.progress(this, state.queue.value, state.activeCount.value)
        try {
            ServiceCompat.startForeground(
                this,
                Notifications.ID_PROGRESS,
                first,
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC else 0,
            )
        } catch (e: RuntimeException) {
            // E.g. ForegroundServiceStartNotAllowedException; the queue still runs while we're open.
            Log.w(TAG, "Couldn't go to the foreground", e)
            stopSelf()
            return START_NOT_STICKY
        }
        holdWakeLock()
        if (!watching) {
            watching = true
            scope.launch {
                state.queue.combine(state.activeCount) { q, n -> q to n }.collect { (queue, active) ->
                    if (active == 0) {
                        finish()
                    } else if (Notifications.canPost(this@DownloadService)) {
                        try {
                            NotificationManagerCompat.from(this@DownloadService)
                                .notify(Notifications.ID_PROGRESS, Notifications.progress(this@DownloadService, queue, active))
                        } catch (_: SecurityException) {
                        }
                    }
                }
            }
        }
        // If Android kills us, the engine restores the queue on the next launch anyway.
        return START_NOT_STICKY
    }

    /**
     * Downloads run as child processes that need the CPU even with the screen off; a
     * foreground service keeps the process alive but doesn't stop the CPU sleeping. So hold a
     * partial wake lock exactly as long as the service runs (it stops when the queue drains),
     * with a generous timeout as a backstop against a leak.
     */
    private fun holdWakeLock() {
        if (wakeLock?.isHeld == true) return
        val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
        wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "Redline:downloads").apply {
            setReferenceCounted(false)
            acquire(WAKE_LOCK_TIMEOUT_MS)
        }
    }

    private fun finish() {
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    /** Android 15+ caps dataSync services at 6 hours a day; give the slot back when it's up. */
    override fun onTimeout(startId: Int, fgsType: Int) {
        finish()
    }

    override fun onDestroy() {
        running = false
        scope.cancel()
        wakeLock?.takeIf { it.isHeld }?.release()
        wakeLock = null
        super.onDestroy()
    }

    companion object {
        private const val TAG = "RedlineService"
        private const val WAKE_LOCK_TIMEOUT_MS = 6 * 60 * 60 * 1000L

        @Volatile private var running = false

        /** Starts the service if jobs are active and it isn't running yet. */
        fun ensureRunning(context: Context) {
            if (running) return
            try {
                ContextCompat.startForegroundService(context, Intent(context, DownloadService::class.java))
                running = true
            } catch (e: RuntimeException) {
                // Starting from the background isn't allowed on Android 12+ (e.g. a scheduled job
                // coming due); the next refresh while the app is in front tries again.
                Log.w(TAG, "Couldn't start the download service", e)
            }
        }
    }
}
