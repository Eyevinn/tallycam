package se.eyevinn.tallycam.control

import org.json.JSONObject
import se.eyevinn.tallycam.media.CameraCapture.Shutter

/**
 * The server's `production` message (docs/SHADING.md, "Shutter"), read
 * leniently as the core reads it: unknown fields ignored, an unknown
 * shutter is `mains`.
 */
object ShadingMessages {
    data class Production(val mainsHz: Int?, val shutter: Shutter)

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
}
