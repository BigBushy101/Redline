package app.redline

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
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
            convertDir = defaultConvertDir(context).absolutePath,
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
     * The phone's Download/Redline, so files show up in the Files app. Android 11+ lets an app
     * create its own files there by path; 8-10 need WRITE_EXTERNAL_STORAGE, and until it's
     * granted the app's own folder is used (see [canWritePublic]).
     */
    fun defaultDownloadDir(context: Context): File =
        if (canWritePublic(context)) {
            File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS), "Redline")
        } else {
            File(context.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS) ?: context.filesDir, "Redline")
        }

    /** Converted files: ffmpeg needs a real folder, so Converted inside the downloads folder. */
    fun defaultConvertDir(context: Context): File =
        if (canWritePublic(context)) {
            File(defaultDownloadDir(context), "Converted")
        } else {
            File(context.getExternalFilesDir(null) ?: context.filesDir, "Converted")
        }

    fun canWritePublic(context: Context): Boolean =
        Build.VERSION.SDK_INT > Build.VERSION_CODES.Q ||
            context.checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED

    /**
     * Whether a saved folder is one of the app's own (where earlier versions saved), so
     * it can be switched to the public one. [ownRoots]: getExternalFilesDir(null) and filesDir.
     */
    fun isOwnFolder(path: String?, ownRoots: List<File?>): Boolean {
        if (path == null) return false
        val dir = File(path).absoluteFile
        return ownRoots.filterNotNull().any { root -> generateSequence(dir) { it.parentFile }.any { it == root.absoluteFile } }
    }

    /** Where files picked for playlists are copied (see [app.redline.state.ImportedMusic]). */
    fun importedMusicDir(context: Context): File =
        context.getExternalFilesDir(Environment.DIRECTORY_MUSIC) ?: File(context.filesDir, "Music")

    /** Where picked and shared files are copied for ffmpeg; each copy gets a folder of its own. */
    fun convertInputsDir(context: Context): File = File(context.cacheDir, "convert-inputs")

    /**
     * Starts the engine. Keep one per process (e.g. in the Application): a second would run the
     * same saved queue again. [changed] runs on Redline's own thread, at most ten times a second.
     */
    fun start(context: Context, changed: () -> Unit): Engine =
        Engine(paths(context), object : EngineListener {
            override fun onChange() = changed()
        })
}
