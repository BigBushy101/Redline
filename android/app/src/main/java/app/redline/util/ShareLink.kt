package app.redline.util

private val LINK = Regex("""https?://\S+""")

/** Most links one batch accepts, matching the desktop's cap. */
const val MAX_BATCH_LINKS = 100

/**
 * Every http(s) link in text, in order and without repeats. The punctuation a sentence puts
 * after a link is dropped; anything that isn't a link is ignored.
 */
fun linksFromText(text: CharSequence?): List<String> {
    val body = text?.toString()?.trim().orEmpty()
    if (body.isEmpty()) return emptyList()
    return LINK.findAll(body)
        .map { it.value.trimEnd('.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '"', '\'') }
        .filter { it.length > 8 }
        .distinct()
        .take(MAX_BATCH_LINKS)
        .toList()
}

/**
 * The first http(s) link in text shared from another app, without punctuation that came with the
 * sentence around it. Null when the share has no link.
 */
fun linkFromShare(text: CharSequence?): String? = linksFromText(text).firstOrNull()
