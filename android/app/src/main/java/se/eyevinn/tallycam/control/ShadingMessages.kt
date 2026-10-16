package se.eyevinn.tallycam.control

import org.json.JSONArray
import org.json.JSONObject
import se.eyevinn.tallycam.media.CameraCapture.Shutter

/**
 * The server's shading messages (docs/SHADING.md, "LCP messages"), read
 * leniently as the core reads them: unknown fields ignored, an unknown
 * shutter is `mains`. Checked against core/tests/vectors/lcp_ownership.json.
 */
object ShadingMessages {
    data class Own(val groups: List<String>, val holderId: String, val holderLabel: String)

    data class Production(val mainsHz: Int?, val shutter: Shutter)

    fun own(message: JSONObject): Own {
        val holder = message.optJSONObject("holder")
        return Own(groups(message), holder?.optString("id").orEmpty(), holder?.optString("label").orEmpty())
    }

    fun groups(message: JSONObject): List<String> = strings(message.optJSONArray("groups"))

    fun production(message: JSONObject): Production = Production(
        message.optDouble("mains_hz", Double.NaN).takeIf { it == 50.0 || it == 60.0 || it == 0.0 }?.toInt(),
        shutter(message.opt("shutter")),
    )

    /** `"mains"`, `"free"` or a whole denominator; anything else is mains. */
    fun shutter(value: Any?): Shutter = when {
        value == "free" -> Shutter.Free
        value is Number && value.toDouble().let { it >= 1 && it <= 100_000 && it == Math.rint(it) } ->
            Shutter.Fixed(value.toInt())
        else -> Shutter.Mains
    }

    private fun strings(array: JSONArray?): List<String> =
        (0 until (array?.length() ?: 0)).mapNotNull { array?.optString(it)?.takeIf(String::isNotEmpty) }
}
