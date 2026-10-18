package se.eyevinn.tallycam.control

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import org.json.JSONObject
import se.eyevinn.tallycam.core.ImageStatsResult

/**
 * What the operator's screen shows about exposure, colour and focus
 * (docs/SHADING.md): the camera's control values and descriptors, and the
 * latest image statistics' warning, as on iOS
 * (ios/TallyCam/Control/Shading.swift). The camera operator and the hub
 * both change these at any time; the last change wins. AppModel updates it
 * on the main thread; the UI reads its Compose state.
 */
class Shading {
    /** `too_dark` or `clipping`, from the image statistics. */
    var exposureWarning by mutableStateOf<String?>(null)
        private set

    /** The camera's control values, refreshed with every `state` (also from the hub's changes). */
    var values by mutableStateOf(JSONObject())
        private set

    /** The camera's control descriptors, by id. */
    var descriptors by mutableStateOf<Map<String, JSONObject>>(emptyMap())
        private set

    /** Resolution and frame rate for the operator's readout: "1080p30", "4K30". */
    var format by mutableStateOf("")
        private set

    private var valuesText = ""

    fun updateFormat(height: Int, frameRate: Int) {
        val text = (if (height >= 2160) "4K" else "${height}p") + frameRate
        if (text != format) format = text
    }

    fun update(values: JSONObject, text: String = values.toString()) {
        if (text == valuesText) return
        valuesText = text
        this.values = values
    }

    fun update(descriptors: List<JSONObject>) {
        val map = descriptors.associateBy { it.optString("id") }
        if (map.mapValues { it.value.toString() } != this.descriptors.mapValues { it.value.toString() }) this.descriptors = map
    }

    fun update(stats: ImageStatsResult) {
        if (stats.warning != exposureWarning) exposureWarning = stats.warning
    }

    companion object {
        /** The mode control of each group the operator's chips show. */
        val MODE_CONTROLS = listOf("exposure" to "exposure_mode", "white_balance" to "wb_mode", "focus" to "focus_mode")

        /** Short name on the phone's chips. */
        fun shortName(group: String): String = when (group) {
            "exposure" -> "EXP"
            "white_balance" -> "WB"
            "focus" -> "FOC"
            else -> group.uppercase()
        }
    }
}

/** "exposure_mode: requires manual" -> "Exposure_mode: requires manual". */
fun String.capitalizedFirst(): String = replaceFirstChar { it.uppercase() }
