// androidx.media 1.8 deprecates the compat session in favour of Media3; the brief asks for MediaSessionCompat.
@file:Suppress("DEPRECATION")

package app.redline.work

import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.graphics.Bitmap
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.os.SystemClock
import android.support.v4.media.MediaMetadataCompat
import android.support.v4.media.session.MediaSessionCompat
import android.support.v4.media.session.PlaybackStateCompat
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import app.redline.RedlineApp
import app.redline.core.PlayerSnapshotRecord
import app.redline.core.PlayerStatus
import app.redline.state.MusicState
import app.redline.util.artModel
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import coil3.request.SuccessResult
import coil3.request.allowHardware
import coil3.toBitmap
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch
import kotlin.math.abs

/**
 * Keeps music playing with the screen off or Redline in the background: a foreground service
 * of type mediaPlayback while a track plays, or is paused with a queue behind it. It owns the
 * media session (lock screen, headset buttons, Bluetooth, the system media controls), the
 * MediaStyle notification, audio focus, the becoming-noisy receiver and a partial wake lock
 * (decoding runs in an ffmpeg child process and Rust threads). The core does the playing; this
 * mirrors [MusicState]'s snapshots out to Android and maps Android's requests back to it.
 * Separate from [DownloadService]; stops itself once nothing is loaded.
 */
class PlaybackService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private lateinit var session: MediaSessionCompat
    private lateinit var audio: AudioManager
    private var wakeLock: PowerManager.WakeLock? = null
    private var watching = false

    private var last: PlayerSnapshotRecord? = null
    private var speed = 1f

    // Audio focus
    private var focusRequest: AudioFocusRequest? = null
    private var hasFocus = false
    /** Paused for a call or another app's short sound; play again when focus comes back. */
    private var resumeOnGain = false

    // What the session and notification last showed, so they're only rebuilt on a change.
    private var shownState: Triple<Int, Long, Float>? = null
    private var shownPosition = 0.0
    private var shownAt = 0L
    private var shownMeta: List<Any?>? = null
    private var shownNotification: List<Any?>? = null
    private var artFor: String? = null
    private var art: Bitmap? = null
    private var artJob: Job? = null

    private val music: MusicState?
        get() = (application as RedlineApp).state.music

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        running = true
        audio = getSystemService(Context.AUDIO_SERVICE) as AudioManager
        session = MediaSessionCompat(this, "Redline").apply {
            setCallback(SessionCallback())
            setSessionActivity(Notifications.openPlayer(this@PlaybackService))
            isActive = true
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val music = music
        // startForegroundService needs startForeground within seconds, on every start.
        try {
            ServiceCompat.startForeground(
                this,
                Notifications.ID_PLAYBACK,
                Notifications.playback(this, session.sessionToken, music?.snapshot?.value, art),
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK else 0,
            )
        } catch (e: RuntimeException) {
            Log.w(TAG, "Couldn't go to the foreground", e)
            stopSelf()
            return START_NOT_STICKY
        }
        if (music == null) {
            finish()
            return START_NOT_STICKY
        }
        when (intent?.action) {
            ACTION_PREV -> music.prev()
            ACTION_TOGGLE -> music.toggle()
            ACTION_NEXT -> music.next()
        }
        if (!watching) {
            watching = true
            scope.launch {
                music.snapshot.combine(music.settings) { s, p -> s to (p?.speed ?: 1f) }.collect { (s, sp) ->
                    speed = sp
                    update(s)
                }
            }
        }
        // Nothing to come back to if Android kills the process: the queue lives in the core.
        return START_NOT_STICKY
    }

    private fun update(s: PlayerSnapshotRecord?) {
        if (s == null || !wanted(s)) {
            finish()
            return
        }
        val wasPlaying = last?.isPlaying == true
        last = s
        // Ask for focus when playback starts, however it was started (screen, session, queue).
        // Only on the change: a late "playing" snapshot after a focus loss mustn't take it back.
        if (s.isPlaying && !wasPlaying && !hasFocus) requestFocus()
        if (s.isPlaying) {
            holdWakeLock()
            registerNoisy()
        } else {
            releaseWakeLock()
            unregisterNoisy()
        }
        updateSession(s)
        updateNotification(s)
    }

    /** Playing, or paused (ended, failed) with a queue to come back to. */
    private fun wanted(s: PlayerSnapshotRecord) =
        s.isPlaying || (MusicState.loaded(s) && s.status != PlayerStatus.IDLE)

    // ---- Media session ------------------------------------------------------------------

    private fun updateSession(s: PlayerSnapshotRecord) {
        val state = when {
            s.status == PlayerStatus.LOADING || (s.buffering && s.isPlaying) -> PlaybackStateCompat.STATE_BUFFERING
            s.isPlaying -> PlaybackStateCompat.STATE_PLAYING
            s.status == PlayerStatus.ERROR -> PlaybackStateCompat.STATE_ERROR
            s.status == PlayerStatus.ENDED -> PlaybackStateCompat.STATE_STOPPED
            else -> PlaybackStateCompat.STATE_PAUSED
        }
        var actions = PlaybackStateCompat.ACTION_PLAY or PlaybackStateCompat.ACTION_PAUSE or
            PlaybackStateCompat.ACTION_PLAY_PAUSE or PlaybackStateCompat.ACTION_STOP or
            PlaybackStateCompat.ACTION_SKIP_TO_PREVIOUS
        if (s.hasNext) actions = actions or PlaybackStateCompat.ACTION_SKIP_TO_NEXT
        if (s.durationSecs != null) actions = actions or PlaybackStateCompat.ACTION_SEEK_TO
        val rate = if (state == PlaybackStateCompat.STATE_PLAYING) speed else 0f
        val now = SystemClock.elapsedRealtime()
        // The system moves the seek bar on by itself from position, time and rate; only tell
        // it again when that guess is off (a seek, a stall) or the state changed.
        val expected = shownPosition + (now - shownAt) / 1000.0 * (shownState?.third ?: 0f)
        val key = Triple(state, actions, rate)
        if (key != shownState || abs(expected - s.positionSecs) > 1.0) {
            shownState = key
            shownPosition = s.positionSecs
            shownAt = now
            val builder = PlaybackStateCompat.Builder()
                .setActions(actions)
                .setState(state, (s.positionSecs * 1000).toLong(), rate, now)
            if (state == PlaybackStateCompat.STATE_ERROR) {
                builder.setErrorMessage(PlaybackStateCompat.ERROR_CODE_APP_ERROR, s.error ?: "Couldn't play this")
            }
            session.setPlaybackState(builder.build())
        }
        loadArt(s.thumbnail)
        val meta = listOf(s.title, s.artist, s.album, s.durationSecs, art)
        if (meta != shownMeta) {
            shownMeta = meta
            val m = MediaMetadataCompat.Builder()
                .putString(MediaMetadataCompat.METADATA_KEY_TITLE, s.title)
                .putString(MediaMetadataCompat.METADATA_KEY_ARTIST, s.artist)
                .putString(MediaMetadataCompat.METADATA_KEY_ALBUM, s.album)
                .putLong(MediaMetadataCompat.METADATA_KEY_DURATION, s.durationSecs?.let { (it * 1000).toLong() } ?: -1L)
            art?.let { m.putBitmap(MediaMetadataCompat.METADATA_KEY_ALBUM_ART, it) }
            session.setMetadata(m.build())
        }
    }

    /** Loads the track's picture for the session and notification, once per picture. */
    private fun loadArt(thumbnail: String?) {
        if (thumbnail == artFor) return
        artFor = thumbnail
        art = null
        artJob?.cancel()
        val model = artModel(thumbnail) ?: return
        artJob = scope.launch {
            val request = ImageRequest.Builder(this@PlaybackService)
                .data(model)
                .size(512)
                .allowHardware(false)
                .build()
            val result = SingletonImageLoader.get(this@PlaybackService).execute(request)
            if (result is SuccessResult && artFor == thumbnail) {
                art = result.image.toBitmap()
                last?.let { updateSession(it); updateNotification(it) }
            }
        }
    }

    private fun updateNotification(s: PlayerSnapshotRecord) {
        val key = listOf(s.title, s.artist, s.isPlaying, s.status, s.buffering, s.error, art)
        if (key == shownNotification) return
        shownNotification = key
        // No canPost check: a media session's notification is exempt from the notification permission.
        try {
            NotificationManagerCompat.from(this)
                .notify(Notifications.ID_PLAYBACK, Notifications.playback(this, session.sessionToken, s, art))
        } catch (_: SecurityException) {
        }
    }

    private inner class SessionCallback : MediaSessionCompat.Callback() {
        override fun onPlay() {
            music?.play()
        }

        override fun onPause() {
            resumeOnGain = false
            music?.pause()
        }

        override fun onSkipToNext() {
            music?.next()
        }

        override fun onSkipToPrevious() {
            music?.prev()
        }

        override fun onSeekTo(pos: Long) {
            music?.seek(pos / 1000.0)
        }

        override fun onStop() {
            music?.stop()
        }
    }

    // ---- Audio focus, headphones, wake lock ---------------------------------------------

    private val focusListener = AudioManager.OnAudioFocusChangeListener { change ->
        when (change) {
            AudioManager.AUDIOFOCUS_LOSS -> {
                // Another app is playing now; stay paused until asked again.
                resumeOnGain = false
                abandonFocus()
                music?.pause()
            }
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> {
                resumeOnGain = last?.isPlaying == true
                music?.pause()
            }
            // Ducking (a navigation prompt) is left to the system, which lowers us itself.
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK -> Unit
            AudioManager.AUDIOFOCUS_GAIN -> {
                hasFocus = true
                if (resumeOnGain) {
                    resumeOnGain = false
                    music?.play()
                }
            }
        }
    }

    private fun requestFocus() {
        val request = focusRequest ?: AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                    .build(),
            )
            .setWillPauseWhenDucked(false)
            .setOnAudioFocusChangeListener(focusListener)
            .build()
            .also { focusRequest = it }
        hasFocus = audio.requestAudioFocus(request) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        // E.g. during a phone call.
        if (!hasFocus) music?.pause()
    }

    private fun abandonFocus() {
        focusRequest?.let { audio.abandonAudioFocusRequest(it) }
        hasFocus = false
    }

    /** Headphones unplugged or Bluetooth gone: pause rather than play out of the speaker. */
    private val noisy = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY) {
                resumeOnGain = false
                music?.pause()
            }
        }
    }
    private var noisyRegistered = false

    private fun registerNoisy() {
        if (noisyRegistered) return
        ContextCompat.registerReceiver(
            this,
            noisy,
            IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        noisyRegistered = true
    }

    private fun unregisterNoisy() {
        if (!noisyRegistered) return
        unregisterReceiver(noisy)
        noisyRegistered = false
    }

    /**
     * Held while playing only: the foreground service keeps the process alive, but the CPU
     * would still sleep under ffmpeg and the decoder threads. The timeout is a backstop
     * against a leak; it's taken again on the next snapshot after it runs out.
     */
    private fun holdWakeLock() {
        if (wakeLock?.isHeld == true) return
        val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
        wakeLock = (wakeLock ?: pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "Redline:playback").apply {
            setReferenceCounted(false)
        }).apply { acquire(WAKE_LOCK_TIMEOUT_MS) }
    }

    private fun releaseWakeLock() {
        wakeLock?.takeIf { it.isHeld }?.release()
    }

    private fun finish() {
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        running = false
        scope.cancel()
        abandonFocus()
        unregisterNoisy()
        releaseWakeLock()
        wakeLock = null
        session.isActive = false
        session.release()
        NotificationManagerCompat.from(this).cancel(Notifications.ID_PLAYBACK)
        super.onDestroy()
    }

    companion object {
        private const val TAG = "RedlinePlayback"
        private const val WAKE_LOCK_TIMEOUT_MS = 60 * 60 * 1000L

        const val ACTION_PREV = "app.redline.playback.PREV"
        const val ACTION_TOGGLE = "app.redline.playback.TOGGLE"
        const val ACTION_NEXT = "app.redline.playback.NEXT"

        @Volatile private var running = false

        /** Starts the service when playback starts, if it isn't running yet. */
        fun ensureRunning(context: Context) {
            if (running) return
            try {
                ContextCompat.startForegroundService(context, Intent(context, PlaybackService::class.java))
                running = true
            } catch (e: RuntimeException) {
                // Not allowed from the background on Android 12+; the next play in front retries.
                Log.w(TAG, "Couldn't start the playback service", e)
            }
        }
    }
}
