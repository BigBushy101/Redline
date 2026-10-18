package app.redline

import android.app.Activity
import android.app.Application
import android.os.Bundle
import app.redline.state.RedlineState
import app.redline.util.AudioCoverFetcher
import app.redline.work.Notifications
import coil3.ImageLoader
import coil3.PlatformContext
import coil3.SingletonImageLoader

/**
 * Owns the one [RedlineState] (and through it the one Rust `Engine`) for the whole process, so
 * the queue keeps running across activity restarts and while the download service is alive.
 */
class RedlineApp : Application(), SingletonImageLoader.Factory {
    lateinit var state: RedlineState
        private set

    override fun onCreate() {
        super.onCreate()
        Notifications.createChannels(this)
        state = RedlineState(this)
        registerActivityLifecycleCallbacks(ForegroundTracker(state))
        // Unpacks Python on the first run (seconds): RedlineState does it off the main thread.
        state.start()
    }

    /** Coil's one loader, which also reads the cover art inside song files. */
    override fun newImageLoader(context: PlatformContext): ImageLoader =
        ImageLoader.Builder(context)
            .components {
                add(AudioCoverFetcher.Factory())
                add(AudioCoverFetcher.Key())
            }
            .build()
}

/**
 * Whether an activity is on screen. The desktop only notifies about finished jobs while
 * Redline isn't in front; this is the same test without pulling in lifecycle-process.
 */
private class ForegroundTracker(private val state: RedlineState) : Application.ActivityLifecycleCallbacks {
    private var started = 0

    override fun onActivityStarted(activity: Activity) {
        started++
        state.inForeground = true
    }

    override fun onActivityStopped(activity: Activity) {
        started = (started - 1).coerceAtLeast(0)
        state.inForeground = started > 0
    }

    override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit
    override fun onActivityResumed(activity: Activity) = Unit
    override fun onActivityPaused(activity: Activity) = Unit
    override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit
    override fun onActivityDestroyed(activity: Activity) = Unit
}
