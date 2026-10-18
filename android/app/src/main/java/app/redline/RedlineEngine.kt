package app.redline

import android.content.Context
import android.os.Environment
import app.redline.core.AndroidPathsRecord
import app.redline.core.Engine
import app.redline.core.EngineListener
import java.io.File

/**
 * Starting the Rust core. youtubedl-android unpacks Python, ffmpeg's libraries and the yt-dlp
 * script into the app's storage and ships the programs as native libraries; this hands their
 * locations to redline-core, which runs the tools itself from then on.
 */
object RedlineEngine {
    /**
     * Where youtubedl-android keeps everything, laid out as its own `YoutubeDL.init` and
     * `FFmpeg.init` do. Call those first (off the main thread: the first run unpacks for seconds).
     */
    fun paths(context: Context, downloadDir: File = defaultDownloadDir(context)): AndroidPathsRecord {
        val nativeLibs = File(context.applicationInfo.nativeLibraryDir)
        val base = File(context.noBackupFilesDir, "youtubedl-android")
        val packages = File(base, "packages")
        val pythonHome = File(packages, "python/usr").absolutePath
        val cache = context.cacheDir.absolutePath
        // The same variables youtubedl-android runs yt-dlp with, plus a cache folder for yt-dlp.
        val env = mapOf(
            "LD_LIBRARY_PATH" to listOf("python", "ffmpeg", "aria2c")
                .joinToString(":") { File(packages, "$it/usr/lib").absolutePath },
            "SSL_CERT_FILE" to File(packages, "python/usr/etc/tls/cert.pem").absolutePath,
            "PATH" to "${System.getenv("PATH").orEmpty()}:${nativeLibs.absolutePath}",
            "PYTHONHOME" to pythonHome,
            "HOME" to pythonHome,
            "TMPDIR" to cache,
            "XDG_CACHE_HOME" to cache,
        )
        return AndroidPathsRecord(
            dataDir = context.filesDir.absolutePath,
            downloadDir = downloadDir.absolutePath,
            python = File(nativeLibs, "libpython.so").absolutePath,
            ytdlpScript = File(base, "yt-dlp/yt-dlp").absolutePath,
            jsRuntimeName = "quickjs",
            jsRuntime = File(nativeLibs, "libqjs.so").absolutePath,
            ffmpeg = File(nativeLibs, "libffmpeg.so").absolutePath,
            ffprobe = File(nativeLibs, "libffprobe.so").absolutePath,
            env = env,
        )
    }

    /**
     * App-specific external storage needs no permission. Moving downloads to the shared
     * Download folder (MediaStore or a picked folder) comes with the settings screen.
     */
    fun defaultDownloadDir(context: Context): File =
        File(context.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS) ?: context.filesDir, "Redline")

    /**
     * Starts the engine. Keep one per process (e.g. in the Application): a second would run the
     * same saved queue again. [changed] runs on Redline's own thread, at most ten times a second.
     */
    fun start(context: Context, changed: () -> Unit): Engine =
        Engine(paths(context), object : EngineListener {
            override fun onChange() = changed()
        })
}
