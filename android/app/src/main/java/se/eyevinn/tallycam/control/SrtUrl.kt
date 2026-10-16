package se.eyevinn.tallycam.control

/**
 * The `latency` parameter of an SRT URL (docs/PROTOCOL.md, "Stream URL"),
 * edited as text so a `{device}` placeholder survives (java.net.URI rejects
 * the braces).
 */
object SrtUrl {
    const val DEFAULT_LATENCY_MS = 120
    val LATENCY_RANGE = 0..8000

    fun latency(url: String): Int? = params(url)?.firstOrNull { it.first.equals("latency", true) }?.second?.toIntOrNull()

    /** The URL with `latency=ms`, replacing an existing one; null for a non-SRT URL. */
    fun settingLatency(ms: Int, url: String): String? {
        val trimmed = url.trim()
        if (!trimmed.startsWith("srt://", ignoreCase = true)) return null
        val q = trimmed.indexOf('?')
        val base = if (q < 0) trimmed else trimmed.substring(0, q)
        val items = (params(trimmed) ?: emptyList()).toMutableList()
        val i = items.indexOfFirst { it.first.equals("latency", true) }
        if (i >= 0) items[i] = items[i].first to ms.toString() else items += "latency" to ms.toString()
        return base + "?" + items.joinToString("&") { (k, v) -> if (v == null) k else "$k=$v" }
    }

    /** Redacts the passphrase for status and logs. */
    fun redacted(url: String): String = url.replace(Regex("(?i)passphrase=[^&]*"), "passphrase=***")

    private fun params(url: String): List<Pair<String, String?>>? {
        val q = url.indexOf('?')
        if (q < 0) return emptyList()
        return url.substring(q + 1).split('&').filter { it.isNotEmpty() }.map {
            val eq = it.indexOf('=')
            if (eq < 0) it to null else it.substring(0, eq) to it.substring(eq + 1)
        }
    }
}
