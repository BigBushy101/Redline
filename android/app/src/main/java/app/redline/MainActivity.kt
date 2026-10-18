package app.redline

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import app.redline.ui.RedlineRoot
import app.redline.ui.Tab
import app.redline.ui.download.DownloadModel
import app.redline.ui.theme.RedlineTheme

/** The one activity: the bottom-bar pages, and where shared links come in. */
class MainActivity : ComponentActivity() {
    private val nav: NavModel by viewModels()
    private val download: DownloadModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        // A recreated activity (rotation) already handled its intent.
        if (savedInstanceState == null) handle(intent)
        val state = (application as RedlineApp).state
        setContent {
            RedlineTheme {
                RedlineRoot(state, download, nav.tab) { nav.tab = it }
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
            intent.action == Intent.ACTION_SEND && intent.type?.startsWith("text/") == true -> {
                val text = intent.getStringExtra(Intent.EXTRA_TEXT) ?: return
                nav.tab = Tab.Download
                download.shared(text)
            }
            intent.getBooleanExtra(EXTRA_OPEN_QUEUE, false) -> nav.tab = Tab.Queue
        }
    }

    companion object {
        /** Set by the notifications: open on the Queue page. */
        const val EXTRA_OPEN_QUEUE = "app.redline.OPEN_QUEUE"
    }
}

/** The selected page, surviving rotation and process death. */
class NavModel(private val saved: SavedStateHandle) : ViewModel() {
    private var current by mutableStateOf(saved.get<String>(KEY)?.let { name -> Tab.entries.firstOrNull { it.name == name } } ?: Tab.Download)

    var tab: Tab
        get() = current
        set(value) {
            current = value
            saved[KEY] = value.name
        }

    private companion object {
        const val KEY = "tab"
    }
}
