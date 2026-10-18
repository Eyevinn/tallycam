package se.eyevinn.tallycam.control

import android.content.Context
import kotlin.math.roundToLong

/** What the phone keeps across launches, in SharedPreferences. */
class Settings(context: Context) {
    private val prefs = context.getSharedPreferences("tallycam", Context.MODE_PRIVATE)

    var controlUrl: String
        get() = prefs.getString("controlUrl", "") ?: ""
        set(value) = prefs.edit().putString("controlUrl", value).apply()

    /** As provisioned: may hold `{device}`. */
    var srtUrl: String
        get() = prefs.getString("srtUrl", "") ?: ""
        set(value) = prefs.edit().putString("srtUrl", value).apply()

    /** The operator's name for this camera; null for the default. */
    var cameraName: String?
        get() = CameraName.normalized(prefs.getString("cameraName", null))
        set(value) {
            val name = CameraName.normalized(value)
            prefs.edit().apply { if (name == null) remove("cameraName") else putString("cameraName", name) }.apply()
        }

    /** "h264" or "hevc"; applies on the next Go Live. */
    var videoCodec: String
        get() = prefs.getString("videoCodec", "h264") ?: "h264"
        set(value) = prefs.edit().putString("videoCodec", value).apply()

    /** "opus" or "aac"; applies on the next Go Live. */
    var audioCodec: String
        get() = prefs.getString("audioCodec", "opus") ?: "opus"
        set(value) = prefs.edit().putString("audioCodec", value).apply()

    /** 50, 60 or 0 (off); -1 until set (then the region's default). */
    var mainsHz: Int
        get() = prefs.getInt("mainsHz", -1)
        set(value) = prefs.edit().putInt("mainsHz", value).apply()

    /** Adaptive bitrate (docs/PROTOCOL.md, `adaptive_bitrate`): on unless turned off. */
    var adaptiveBitrate: Boolean
        get() = prefs.getBoolean("adaptiveBitrate", true)
        set(value) = prefs.edit().putBoolean("adaptiveBitrate", value).apply()

    /** 0: the default for the format. */
    var bitrateKbps: Int
        get() = prefs.getInt("bitrateKbps", 0)
        set(value) = prefs.edit().putInt("bitrateKbps", value).apply()

    /** Soft genlock (docs/GENLOCK.md): off until turned on, then kept. */
    var genlock: Boolean
        get() = prefs.getBoolean("genlockEnabled", false)
        set(value) = prefs.edit().putBoolean("genlockEnabled", value).apply()

    /** Where frames land after each grid point, µs: a per-model calibration, so kept. */
    var genlockOffsetUs: Double
        get() = clampedGenlockOffsetUs(prefs.getLong("genlockOffsetNs", 0) / 1000.0)
        set(value) = prefs.edit().putLong("genlockOffsetNs", (clampedGenlockOffsetUs(value) * 1000).roundToLong()).apply()

    companion object {
        /** The advertised range of `genlock_offset_us`, ±µs. */
        const val GENLOCK_OFFSET_MAX_US = 50000.0

        /** Within ±[GENLOCK_OFFSET_MAX_US]; not finite is 0. */
        fun clampedGenlockOffsetUs(us: Double): Double =
            if (us.isFinite()) us.coerceIn(-GENLOCK_OFFSET_MAX_US, GENLOCK_OFFSET_MAX_US) else 0.0
    }
}
