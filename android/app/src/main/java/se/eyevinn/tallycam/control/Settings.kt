package se.eyevinn.tallycam.control

import android.content.Context

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

    /** 0: the default for the format. */
    var bitrateKbps: Int
        get() = prefs.getInt("bitrateKbps", 0)
        set(value) = prefs.edit().putInt("bitrateKbps", value).apply()
}
