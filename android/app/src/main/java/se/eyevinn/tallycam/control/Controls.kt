package se.eyevinn.tallycam.control

import org.json.JSONArray
import org.json.JSONObject

/** Control descriptors as `hello` carries them (docs/PROTOCOL.md). */
object Controls {
    fun text(id: String, label: String, group: String) =
        JSONObject().put("id", id).put("label", label).put("group", group).put("kind", "text")

    fun bool(id: String, label: String, group: String) =
        JSONObject().put("id", id).put("label", label).put("group", group).put("kind", "bool")

    fun range(id: String, label: String, group: String, min: Double, max: Double, step: Double, unit: String) =
        JSONObject().put("id", id).put("label", label).put("group", group).put("kind", "range")
            .put("min", min).put("max", max).put("step", step).put("unit", unit)

    fun command(name: String, label: String) = JSONObject().put("name", name).put("label", label)

    fun array(items: List<JSONObject>) = JSONArray().apply { items.forEach { put(it) } }
}
