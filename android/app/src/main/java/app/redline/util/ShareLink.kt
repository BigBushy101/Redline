package app.redline.util

private val LINK = Regex("""https?://\S+""")

/**
 * The first http(s) link in text shared from another app, without punctuation that came with the
 * sentence around it. Null when the share has no link.
 */
fun linkFromShare(text: CharSequence?): String? {
    val body = text?.toString()?.trim().orEmpty()
    val found = LINK.find(body)?.value ?: return null
    return found.trimEnd('.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '"', '\'')
}
