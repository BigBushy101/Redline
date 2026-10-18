package app.redline

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.core.content.IntentCompat
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import app.redline.ui.RedlineRoot
import app.redline.ui.Tab
import app.redline.ui.convert.ConvertModel
import app.redline.ui.download.DownloadModel
import app.redline.ui.playlists.PlaylistsModel
import app.redline.ui.saved.SavedModel
import app.redline.ui.theme.RedlineTheme

/** The one activity: the bottom-bar pages, and where shared links and files come in. */
class MainActivity : ComponentActivity() {
    private val nav: NavModel by viewModels()
    private val download: DownloadModel by viewModels()
    private val convert: ConvertModel by viewModels()
    private val playlists: PlaylistsModel by viewModels()
    private val saved: SavedModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        // A recreated activity (rotation) already handled its intent.
        if (savedInstanceState == null) handle(intent)
        val state = (application as RedlineApp).state
        setContent {
            RedlineTheme {
                RedlineRoot(state, download, convert, playlists, saved, nav)
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handle(intent)
    }

    private fun handle(intent: Intent?) {
        when {
            intent == null -> Unit
            // Videos, songs and pictures shared from the gallery, a file manager...: Convert them.
            isMedia(intent.type) && (intent.action == Intent.ACTION_SEND || intent.action == Intent.ACTION_SEND_MULTIPLE) -> {
                val uris = if (intent.action == Intent.ACTION_SEND) {
                    listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
                } else {
                    IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
                }
                if (uris.isEmpty()) return
                nav.tab = Tab.Convert
                convert.addUris(uris)
            }
            intent.action == Intent.ACTION_SEND && intent.type?.startsWith("text/") == true -> {
                val text = intent.getStringExtra(Intent.EXTRA_TEXT) ?: return
                nav.tab = Tab.Download
                download.shared(text)
            }
            intent.getBooleanExtra(EXTRA_OPEN_QUEUE, false) -> nav.tab = Tab.Queue
            intent.getBooleanExtra(EXTRA_OPEN_PLAYER, false) -> nav.playerOpen = true
        }
    }

    /** Whether a shared item's MIME type (e.g. video/mp4) is one Convert takes. */
    private fun isMedia(type: String?): Boolean {
        val top = type?.substringBefore('/') ?: return false
        return top == "audio" || top == "video" || top == "image"
    }

    companion object {
        /** Set by the notifications: open on the Queue page. */
        const val EXTRA_OPEN_QUEUE = "app.redline.OPEN_QUEUE"
        /** Set by the player's notification: show the full player. */
        const val EXTRA_OPEN_PLAYER = "app.redline.OPEN_PLAYER"
    }
}

/**
 * The selected page, the Library's section and whether the full player is up, surviving
 * rotation and process death.
 */
class NavModel(private val saved: SavedStateHandle) : ViewModel() {
    private var current by mutableStateOf(saved.get<String>(KEY)?.let(::tabNamed) ?: Tab.Download)
    private var before by mutableStateOf(saved.get<String>(KEY_BACK)?.let(::tabNamed) ?: Tab.Download)
    private var player by mutableStateOf(saved.get<Boolean>(KEY_PLAYER) ?: false)
    private var showSaved by mutableStateOf(saved.get<Boolean>(KEY_LIBRARY_SAVED) ?: true)

    var tab: Tab
        get() = current
        set(value) {
            // Settings isn't in the bottom bar; Back from it returns to the page it was opened on.
            if (value == Tab.Settings && current != Tab.Settings) {
                before = current
                saved[KEY_BACK] = current.name
            }
            current = value
            saved[KEY] = value.name
        }

    /** The page Settings was opened from. */
    val backFromSettings: Tab get() = before

    var playerOpen: Boolean
        get() = player
        set(value) {
            player = value
            saved[KEY_PLAYER] = value
        }

    /** The Library shows Saved (else Playlists). */
    var librarySaved: Boolean
        get() = showSaved
        set(value) {
            showSaved = value
            saved[KEY_LIBRARY_SAVED] = value
        }

    // Library was Playlists before Saved came; a page saved then still opens it.
    private fun tabNamed(name: String) = if (name == "Playlists") Tab.Library else Tab.entries.firstOrNull { it.name == name }

    private companion object {
        const val KEY = "tab"
        const val KEY_BACK = "tab-before-settings"
        const val KEY_PLAYER = "player-open"
        const val KEY_LIBRARY_SAVED = "library-saved"
    }
}
