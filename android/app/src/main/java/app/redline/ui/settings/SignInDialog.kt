package app.redline.ui.settings

import android.annotation.SuppressLint
import android.content.Context
import android.webkit.CookieManager
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors
import java.io.File

// Google's sign-in, landing on mobile YouTube once it's done.
private const val SIGN_IN_URL = "https://accounts.google.com/ServiceLogin?service=youtube&passive=true" +
    "&continue=https%3A%2F%2Fm.youtube.com%2F"

// The sites whose cookies go into cookies.txt, written as domain-wide cookies.
private val COOKIE_SITES = listOf(
    "https://www.youtube.com" to ".youtube.com",
    "https://music.youtube.com" to ".youtube.com",
    "https://accounts.google.com" to ".google.com",
    "https://www.google.com" to ".google.com",
)

/**
 * A full-screen web page for signing in to YouTube. "Done" writes the WebView's cookies as a
 * Netscape cookies.txt, which yt-dlp reads like one exported from a desktop browser.
 */
@SuppressLint("SetJavaScriptEnabled")
@Composable
fun SignInDialog(onSignedIn: (String) -> Unit, onDismiss: () -> Unit) {
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        var web by remember { mutableStateOf<WebView?>(null) }
        var notYet by remember { mutableStateOf(false) }
        BackHandler { web?.takeIf { it.canGoBack() }?.goBack() ?: onDismiss() }
        Column(Modifier.fillMaxSize().background(RedlineColors.Elevated).statusBarsPadding()) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                TextButton(onClick = onDismiss) { Text("Cancel", color = RedlineColors.Muted) }
                Text(
                    if (notYet) "Not signed in yet" else "Sign in to YouTube",
                    Modifier.weight(1f),
                    style = MaterialTheme.typography.titleSmall,
                    color = RedlineColors.Text,
                )
                TextButton(onClick = {
                    val path = web?.context?.let(::saveCookies)
                    if (path != null) onSignedIn(path) else notYet = true
                }) {
                    Text("Done", color = LocalAccent.current.hover)
                }
            }
            AndroidView(
                factory = { ctx ->
                    WebView(ctx).apply {
                        settings.javaScriptEnabled = true
                        settings.domStorageEnabled = true
                        // Google refuses sign-in from pages that say they're an embedded WebView.
                        settings.userAgentString = settings.userAgentString.replace("; wv", "")
                        CookieManager.getInstance().setAcceptThirdPartyCookies(this, true)
                        webViewClient = WebViewClient()
                        loadUrl(SIGN_IN_URL)
                        web = this
                    }
                },
                modifier = Modifier.weight(1f).fillMaxWidth(),
                onRelease = { it.destroy() },
            )
        }
    }
}

/** Writes the signed-in cookies to the app's cookies.txt and returns its path, or null when not signed in. */
private fun saveCookies(context: Context): String? {
    val manager = CookieManager.getInstance()
    manager.flush()
    // YouTube sets these once an account is signed in.
    val youtube = manager.getCookie("https://www.youtube.com").orEmpty()
    if ("LOGIN_INFO=" !in youtube && "SAPISID=" !in youtube) return null
    // A year out: CookieManager doesn't give expiry dates, and yt-dlp drops expired cookies.
    val expiry = System.currentTimeMillis() / 1000 + 365L * 24 * 3600
    val seen = mutableSetOf<String>()
    val lines = buildList {
        add("# Netscape HTTP Cookie File")
        for ((url, domain) in COOKIE_SITES) {
            val header = manager.getCookie(url) ?: continue
            for (pair in header.split(";")) {
                val name = pair.substringBefore("=").trim()
                val value = pair.substringAfter("=", "").trim()
                if (name.isEmpty() || !seen.add("$domain\t$name")) continue
                add(listOf(domain, "TRUE", "/", "TRUE", expiry.toString(), name, value).joinToString("\t"))
            }
        }
    }
    val out = File(context.filesDir, "cookies.txt")
    out.writeText(lines.joinToString("\n", postfix = "\n"))
    return out.absolutePath
}

/** Forgets the in-app sign-in. */
fun clearSignIn() {
    CookieManager.getInstance().removeAllCookies(null)
}
