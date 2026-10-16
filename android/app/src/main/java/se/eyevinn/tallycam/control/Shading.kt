package se.eyevinn.tallycam.control

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import org.json.JSONObject
import se.eyevinn.tallycam.core.GroupView
import se.eyevinn.tallycam.core.ImageStatsResult
import se.eyevinn.tallycam.core.OwnershipMachine
import se.eyevinn.tallycam.core.shadingControlGroup

/**
 * Who owns the camera's exposure, colour and focus (docs/SHADING.md): the
 * core's ownership state machine, persisted across restarts (D3), plus what
 * the operator's screen shows about it and the latest image statistics, as
 * on iOS (ios/TallyCam/Control/Shading.swift). AppModel drives it on the
 * main thread; the UI reads its Compose state.
 */
class Shading(context: Context) {
    private val prefs = context.getSharedPreferences("shading", Context.MODE_PRIVATE)

    val machine = OwnershipMachine(GROUPS, prefs.getString(STATE_KEY, null))

    /** One entry per group the camera has, in a fixed order. */
    var groups by mutableStateOf(machine.view())
        private set

    /** `too_dark` or `clipping`, from the image statistics. */
    var exposureWarning by mutableStateOf<String?>(null)
        private set

    /** Called after an owner changes, with the `ownership` message. */
    var onChange: ((String) -> Unit)? = null

    /** The group controls' values while a group is not auto, saved so a relaunch comes back with them (D3). */
    private var savedValues: String = prefs.getString(VALUES_KEY, null) ?: "{}"

    fun view(group: String): GroupView? = groups.firstOrNull { it.group == group }

    /** The groups this camera has (focus only with a focus control). */
    fun useGroups(names: List<String>) {
        machine.setGroups(names)
        changed()
    }

    /** An owner changed (or the link did): persist, refresh the view, tell the server. */
    fun changed(notify: Boolean = true) {
        prefs.edit().putString(STATE_KEY, machine.saveJson()).apply()
        val view = machine.view()
        if (view != groups) groups = view
        if (notify) onChange?.invoke(machine.messageJson())
    }

    /** Keeps the values of the groups that are not auto. */
    fun note(values: JSONObject, shutterFree: Boolean) {
        val owned = groups.filter { it.indicator != "auto" }.map { it.group }.toSet()
        val kept = JSONObject()
        for (key in values.keys()) {
            if (shadingControlGroup(key, shutterFree)?.let { it in owned } == true) kept.put(key, values.get(key))
        }
        val text = kept.toString()
        if (text == savedValues) return
        savedValues = text
        prefs.edit().putString(VALUES_KEY, text).apply()
    }

    /** The saved values of the groups that are not auto. */
    fun restoredValues(): JSONObject {
        val owned = groups.filter { it.indicator != "auto" }.map { it.group }.toSet()
        val saved = runCatching { JSONObject(savedValues) }.getOrDefault(JSONObject())
        val out = JSONObject()
        for (key in saved.keys()) {
            if (shadingControlGroup(key, true)?.let { it in owned } == true) out.put(key, saved.get(key))
        }
        return out
    }

    fun update(stats: ImageStatsResult) {
        if (stats.warning != exposureWarning) exposureWarning = stats.warning
    }

    companion object {
        val GROUPS = listOf("exposure", "white_balance", "focus")
        private const val STATE_KEY = "ownership"
        private const val VALUES_KEY = "values"

        /** The values that put groups into auto (rule 4: never the last values). */
        fun autoValues(groups: List<String>): JSONObject {
            val v = JSONObject()
            for (group in groups) when (group) {
                "exposure" -> v.put("exposure_mode", "auto").put("ev_bias", 0)
                "white_balance" -> v.put("wb_mode", "auto")
                "focus" -> v.put("focus_mode", "auto")
            }
            return v
        }
    }
}

/** Short name on the phone's chips. */
val GroupView.shortName: String
    get() = when (group) {
        "exposure" -> "EXP"
        "white_balance" -> "WB"
        "focus" -> "FOC"
        else -> group.uppercase()
    }

val GroupView.title: String
    get() = when (group) {
        "exposure" -> "Exposure"
        "white_balance" -> "Colour"
        "focus" -> "Focus"
        else -> group
    }

val GroupView.isRemote: Boolean get() = indicator == "remote" || indicator == "remote_disconnected"

/** "exposure: under remote control" -> "Exposure: under remote control". */
fun String.capitalizedFirst(): String = replaceFirstChar { it.uppercase() }
