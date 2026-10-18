package se.eyevinn.tallycam

/**
 * The SRT passphrase across the hub (security review): `state` sends the
 * stream URL with every passphrase redacted ("***", SrtUrl.redacted), and a
 * `set` may send it back that way. The phone fills in its own passphrase
 * only for the same target: the same scheme, host and port and the same
 * streamid. Anything else with "***" is refused, so a hub (or a ws://
 * man in the middle) cannot send `srt://attacker:9000?passphrase=***` and
 * have the phone dial out with the real passphrase.
 */
object SrtPassphrase {
    private const val REDACTED = "***"

    /**
     * [url] with each redacted passphrase replaced by the one in [current]
     * (the last, which is the one SRT uses); [url] unchanged when it has no
     * redacted passphrase; null when it must be refused.
     */
    fun restore(url: String, current: String): String? {
        val params = query(url)
        if (params.none { isPassphrase(it.first) && it.second == REDACTED }) return url
        val target = target(url) ?: return null
        if (target != target(current)) return null
        if (value(params, "streamid") != value(query(current), "streamid")) return null
        val kept = query(current).lastOrNull { isPassphrase(it.first) }?.second?.takeIf { it != REDACTED } ?: return null
        val rebuilt = params.joinToString("&") { (key, v) ->
            if (isPassphrase(key) && v == REDACTED) "$key=$kept" else if (v == null) key else "$key=$v"
        }
        return url.substringBefore('?') + "?" + rebuilt
    }

    private fun isPassphrase(key: String) = key.equals("passphrase", ignoreCase = true)

    /** "srt://host:port", lower case; null without a host. */
    private fun target(url: String): String? {
        val base = url.substringBefore('?').trimEnd('/')
        val scheme = base.substringBefore("://", "").lowercase().takeIf { it.isNotEmpty() } ?: return null
        val hostPort = base.substringAfter("://").substringBefore('/').lowercase().takeIf { it.isNotEmpty() } ?: return null
        return "$scheme://$hostPort"
    }

    private fun query(url: String): List<Pair<String, String?>> {
        val q = url.substringAfter('?', "").takeIf { it.isNotEmpty() } ?: return emptyList()
        return q.split('&').filter { it.isNotEmpty() }.map { part ->
            val key = part.substringBefore('=')
            key to (if ('=' in part) part.substringAfter('=') else null)
        }
    }

    private fun value(params: List<Pair<String, String?>>, key: String) =
        params.lastOrNull { it.first.equals(key, ignoreCase = true) }?.second
}
