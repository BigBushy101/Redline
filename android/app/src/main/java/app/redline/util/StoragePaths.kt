package app.redline.util

import android.content.ContentUris
import android.content.Context
import android.net.Uri
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.MediaStore
import java.io.File

/** The path of a folder from the system picker, on internal storage or an SD card. */
fun treePath(tree: Uri): String? {
    if (tree.authority != EXTERNAL_STORAGE) return null
    val id = runCatching { DocumentsContract.getTreeDocumentId(tree) }.getOrNull() ?: return null
    return storagePath(id)
}

/**
 * Where a picked file really is, when it's a readable file on the phone's storage: a document
 * from the storage, media or downloads providers, or a media link. `null` for files only their
 * app can hand over (Drive, a cloud folder), which have to be copied.
 */
fun documentPath(context: Context, uri: Uri): String? {
    val path = runCatching { lookUpPath(context, uri) }.getOrNull() ?: return null
    return path.takeIf { File(it).let { f -> f.isFile && f.canRead() } }
}

private fun lookUpPath(context: Context, uri: Uri): String? {
    if (uri.scheme == "file") return uri.path
    if (uri.scheme != "content") return null
    if (uri.authority == MediaStore.AUTHORITY) return dataColumn(context, uri)
    if (!DocumentsContract.isDocumentUri(context, uri)) return null
    val id = DocumentsContract.getDocumentId(uri)
    return when (uri.authority) {
        EXTERNAL_STORAGE -> storagePath(id)
        "com.android.providers.media.documents" -> {
            val (type, n) = id.split(':', limit = 2).takeIf { it.size == 2 } ?: return null
            val base = when (type) {
                "audio" -> MediaStore.Audio.Media.EXTERNAL_CONTENT_URI
                "video" -> MediaStore.Video.Media.EXTERNAL_CONTENT_URI
                else -> MediaStore.Files.getContentUri("external")
            }
            n.toLongOrNull()?.let { dataColumn(context, ContentUris.withAppendedId(base, it)) }
        }
        "com.android.providers.downloads.documents" -> when {
            id.startsWith("raw:") -> id.removePrefix("raw:")
            id.startsWith("msf:") -> id.removePrefix("msf:").toLongOrNull()?.let {
                dataColumn(context, ContentUris.withAppendedId(MediaStore.Files.getContentUri("external"), it))
            }
            else -> null
        }
        else -> null
    }
}

@Suppress("DEPRECATION") // DATA is still the file's path, and still readable.
private fun dataColumn(context: Context, uri: Uri): String? =
    context.contentResolver.query(uri, arrayOf(MediaStore.MediaColumns.DATA), null, null, null)?.use { c ->
        if (c.moveToFirst() && !c.isNull(0)) c.getString(0) else null
    }

/** "primary:Music/x" or "1234-5678:Music" as a path. */
private fun storagePath(id: String): String {
    val volume = id.substringBefore(':')
    val rel = id.substringAfter(':', "").trim('/')
    val root = if (volume.equals("primary", ignoreCase = true)) {
        Environment.getExternalStorageDirectory().absolutePath
    } else {
        "/storage/$volume"
    }
    return if (rel.isEmpty()) root else "$root/$rel"
}

private const val EXTERNAL_STORAGE = "com.android.externalstorage.documents"
