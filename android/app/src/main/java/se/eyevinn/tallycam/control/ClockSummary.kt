package se.eyevinn.tallycam.control

import org.json.JSONObject

/** The clock status for Settings, from the core's status JSON (as iOS ClockSummary). */
data class ClockSummary(
    val synced: Boolean,
    val health: String,
    val estErrorUs: Double?,
    val maxErrorUs: Double?,
    val reference: String?,
    val stratum: Int?,
    val reachable: Int?,
    val total: Int?,
    val findings: List<String>,
) {
    companion object {
        fun parse(json: String): ClockSummary? = runCatching {
            val o = JSONObject(json)
            val synced = o.optBoolean("synced", false)
            val sources = o.optJSONArray("sources")
            var reference: String? = o.optString("reference_name").ifEmpty { null }
                ?.let { name -> o.optString("reference").ifEmpty { null }?.let { "$name ($it)" } ?: name }
            var reachable: Int? = null
            if (sources != null) {
                reachable = 0
                for (i in 0 until sources.length()) {
                    val s = sources.getJSONObject(i)
                    if (s.optInt("reach", 0) > 0) reachable++
                    if (reference == null && s.optString("state") == "*") {
                        val name = s.optString("name")
                        val addr = s.optString("addr")
                        reference = if (name.isNotEmpty() && addr.isNotEmpty()) "$name ($addr)" else name.ifEmpty { addr }
                    }
                }
            }
            val findings = o.optJSONArray("findings")?.let { a -> (0 until a.length()).map { a.getString(it) } }.orEmpty()
            ClockSummary(
                synced = synced,
                health = o.optString("health").ifEmpty { if (synced) "healthy" else "bad" },
                estErrorUs = o.optDouble("est_error_us").takeIf { !it.isNaN() },
                maxErrorUs = o.optDouble("max_error_us").takeIf { !it.isNaN() },
                reference = reference,
                stratum = o.optInt("stratum", 0).takeIf { it > 0 },
                reachable = o.optInt("reachable", -1).takeIf { it >= 0 } ?: reachable,
                total = o.optInt("total", -1).takeIf { it >= 0 } ?: sources?.length(),
                findings = findings,
            )
        }.getOrNull()

        /** "212 µs", "3.2 ms", "1.20 s". */
        fun formatMicroseconds(us: Double?): String = when {
            us == null || us < 0 -> "-"
            us < 1000 -> "%.0f µs".format(us)
            us < 1_000_000 -> "%.1f ms".format(us / 1000)
            else -> "%.2f s".format(us / 1_000_000)
        }
    }
}
