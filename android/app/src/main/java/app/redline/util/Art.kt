package app.redline.util

import app.redline.core.TrackKind
import app.redline.core.TrackRecord
import java.io.File

/**
 * What Coil loads for a track's picture: the core gives an image URL or, for files with
 * embedded or side-by-side art, a path on the device. A song file's own path loads the art
 * inside it ([AudioCover]).
 */
fun artModel(thumbnail: String?): Any? = when {
    thumbnail.isNullOrBlank() -> null
    thumbnail.startsWith("/") && isAudioPath(thumbnail) -> AudioCover(thumbnail)
    thumbnail.startsWith("/") -> File(thumbnail)
    else -> thumbnail
}

/** The picture to show for a track: its own, else (for a song file) the art in the file. */
val TrackRecord.art: String?
    get() = thumbnail ?: location.takeIf { kind == TrackKind.FILE }

/** What the core's player takes (redline-core's `classify` Audio). */
val AUDIO_EXTENSIONS = setOf("mp3", "m4a", "aac", "flac", "wav", "ogg", "opus", "wma", "aiff", "aif", "alac")

fun isAudioPath(path: String): Boolean = path.substringAfterLast('.', "").lowercase() in AUDIO_EXTENSIONS
