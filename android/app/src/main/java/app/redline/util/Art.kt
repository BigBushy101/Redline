package app.redline.util

import java.io.File

/**
 * What Coil loads for a track's picture: the core gives an image URL or, for files with
 * embedded or side-by-side art, a path on the device.
 */
fun artModel(thumbnail: String?): Any? = when {
    thumbnail.isNullOrBlank() -> null
    thumbnail.startsWith("/") -> File(thumbnail)
    else -> thumbnail
}
