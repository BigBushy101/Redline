package app.redline.state

import android.content.Context
import app.redline.util.isAudioPath

/**
 * The songs kept in the Library's Local section on their own because they were added on the
 * Convert page (the desktop's `settings.local_files`): their paths, in the order added. Some
 * are copies in [ImportedMusic]'s folder, which its sweep keeps while they're listed here.
 */
object LocalFiles {
    private const val PREFS = "local"
    private const val KEY = "files"

    fun load(context: Context): List<String> =
        prefs(context).getString(KEY, null)?.split('\n')?.filter { it.isNotEmpty() } ?: emptyList()

    /** Adds the songs among [paths] that aren't listed yet. */
    @Synchronized
    fun add(context: Context, paths: List<String>) {
        val now = load(context)
        val fresh = paths.filter { isAudioPath(it) && it !in now }.distinct()
        if (fresh.isNotEmpty()) save(context, now + fresh)
    }

    @Synchronized
    fun remove(context: Context, path: String) = save(context, load(context) - path)

    @Synchronized
    fun clear(context: Context) = save(context, emptyList())

    /** Watching for changes uses the same preferences, under this key. */
    fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    const val KEY_FILES = KEY

    private fun save(context: Context, files: List<String>) {
        prefs(context).edit().putString(KEY, files.joinToString("\n")).apply()
    }
}
