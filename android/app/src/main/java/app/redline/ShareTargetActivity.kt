package app.redline

import android.app.Activity
import android.content.Intent
import android.os.Bundle

/**
 * Forwards a link shared from another app to [MainActivity], tagged with what the share target
 * asks for. The share sheet shows the two subclasses as separate choices.
 */
abstract class ShareTargetActivity(private val mode: String) : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)
        startActivity(
            Intent(this, MainActivity::class.java).apply {
                action = Intent.ACTION_SEND
                type = "text/plain"
                putExtra(Intent.EXTRA_TEXT, text)
                putExtra(MainActivity.EXTRA_SHARE_MODE, mode)
                addFlags(
                    Intent.FLAG_ACTIVITY_NEW_TASK or
                        Intent.FLAG_ACTIVITY_CLEAR_TOP or
                        Intent.FLAG_ACTIVITY_SINGLE_TOP,
                )
            },
        )
        finish()
    }
}

/** The share sheet's "Save to Redline" target. */
class ShareSaveActivity : ShareTargetActivity(MainActivity.SHARE_SAVE)

/** The share sheet's "Download with Redline" target. */
class ShareDownloadActivity : ShareTargetActivity(MainActivity.SHARE_DOWNLOAD)
