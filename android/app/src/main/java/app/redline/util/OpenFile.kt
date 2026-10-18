package app.redline.util

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import java.io.File

/** Why a file couldn't be opened, worded for a snackbar; null when it opened. */
fun openFile(context: Context, path: String): String? {
    val file = File(path)
    if (!file.exists()) return "The file was moved or deleted."
    // Chapter splits and playlists end in a folder; there's no portable "open folder" intent.
    if (file.isDirectory) return "Saved in ${file.name}/ (open it from a file manager)."
    val uri = try {
        FileProvider.getUriForFile(context, "${context.packageName}.files", file)
    } catch (_: IllegalArgumentException) {
        return "Redline can't share files from that folder."
    }
    val intent = Intent(Intent.ACTION_VIEW)
        .setDataAndType(uri, mimeType(file))
        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
    return try {
        context.startActivity(intent)
        null
    } catch (_: ActivityNotFoundException) {
        "No app here can open ${file.extension} files."
    }
}

/** The MIME type players look for; Android's table misses a few of yt-dlp's containers. */
fun mimeType(file: File): String {
    val ext = file.extension.lowercase()
    MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext)?.let { return it }
    return when (ext) {
        "mkv" -> "video/x-matroska"
        "webm" -> "video/webm"
        "opus" -> "audio/ogg"
        "m4a" -> "audio/mp4"
        "flac" -> "audio/flac"
        "srt" -> "application/x-subrip"
        else -> "*/*"
    }
}
