package app.redline.util

import android.media.MediaMetadataRetriever
import coil3.ImageLoader
import coil3.decode.DataSource
import coil3.decode.ImageSource
import coil3.fetch.FetchResult
import coil3.fetch.Fetcher
import coil3.fetch.SourceFetchResult
import coil3.key.Keyer
import coil3.request.Options
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import okio.Buffer
import okio.Path.Companion.toOkioPath
import java.io.File
import java.io.IOException

/** A song file whose cover art Coil should show (see [artModel]). */
data class AudioCover(val path: String)

/**
 * Loads an [AudioCover]: the picture embedded in the file's tags, else a cover.jpg or
 * folder.jpg beside it. Fails when there's neither, and [app.redline.ui.components.Cover]
 * shows its icon.
 */
class AudioCoverFetcher(private val data: AudioCover, private val options: Options) : Fetcher {
    override suspend fun fetch(): FetchResult = withContext(Dispatchers.IO) {
        val embedded = runCatching {
            val r = MediaMetadataRetriever()
            try {
                r.setDataSource(data.path)
                r.embeddedPicture
            } finally {
                r.release()
            }
        }.getOrNull()
        if (embedded != null) {
            return@withContext SourceFetchResult(
                source = ImageSource(Buffer().write(embedded), options.fileSystem),
                mimeType = null,
                dataSource = DataSource.DISK,
            )
        }
        val dir = File(data.path).parentFile
        val beside = SIDE_COVERS.firstNotNullOfOrNull { name -> File(dir, name).takeIf { it.isFile } }
            ?: throw IOException("No cover art for ${data.path}")
        SourceFetchResult(
            source = ImageSource(beside.toOkioPath(), options.fileSystem),
            mimeType = null,
            dataSource = DataSource.DISK,
        )
    }

    class Factory : Fetcher.Factory<AudioCover> {
        override fun create(data: AudioCover, options: Options, imageLoader: ImageLoader): Fetcher =
            AudioCoverFetcher(data, options)
    }

    /** Lets the memory cache keep covers; a changed file gets a new key. */
    class Key : Keyer<AudioCover> {
        override fun key(data: AudioCover, options: Options): String =
            "audio-cover:${data.path}:${File(data.path).lastModified()}"
    }

    private companion object {
        val SIDE_COVERS = listOf("cover.jpg", "folder.jpg", "front.jpg", "cover.png", "folder.png", "Cover.jpg", "Folder.jpg")
    }
}
