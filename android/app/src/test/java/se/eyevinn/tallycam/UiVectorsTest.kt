package se.eyevinn.tallycam

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import se.eyevinn.tallycam.control.Shading
import se.eyevinn.tallycam.ui.Neutral
import se.eyevinn.tallycam.ui.SIDE_GROUPS
import se.eyevinn.tallycam.ui.Sides
import se.eyevinn.tallycam.ui.Tone
import se.eyevinn.tallycam.ui.VALUE_DROP_ORDER
import se.eyevinn.tallycam.ui.adjustValueText
import se.eyevinn.tallycam.ui.bitrate
import se.eyevinn.tallycam.ui.neutral
import se.eyevinn.tallycam.ui.tapStep
import se.eyevinn.tallycam.ui.timecode
import se.eyevinn.tallycam.ui.uptime
import se.eyevinn.tallycam.ui.valueItems
import se.eyevinn.tallycam.ui.zoomFactorText
import java.io.File
import java.util.Locale

/**
 * The shared UI vectors (core/tests/vectors/ui_values.json), against the
 * functions the shooting screen draws with; the iOS tests read the same
 * file (docs/UI_DESIGN.md).
 */
class UiVectorsTest {
    private val vectors: JSONObject = JSONObject(
        File(System.getProperty("tallycam.vectors") ?: "../../core/tests/vectors", "ui_values.json").readText())

    private val descriptors: Map<String, JSONObject> =
        vectors.getJSONObject("descriptors").let { d -> d.keys().asSequence().associateWith { d.getJSONObject(it) } }

    private fun JSONArray.objects() = (0 until length()).map { getJSONObject(it) }
    private fun JSONObject.stringOrNull(key: String) = if (isNull(key)) null else optString(key)

    /** A set as plain values, for comparing 0.3 with 0.30000000000000004 and 2 with 2.0. */
    private fun plain(o: JSONObject): Map<String, Any> = o.keys().asSequence().associateWith {
        when (val v = o.get(it)) {
            is Number -> Math.round(v.toDouble() * 1e6) / 1e6
            else -> v
        }
    }

    @Test
    fun format() {
        for (c in vectors.getJSONArray("format").objects()) {
            val shading = Shading()
            shading.updateFormat(minOf(c.getInt("width"), c.getInt("height")), c.getInt("fps"))
            assertEquals(c.toString(), c.getString("expect"), shading.format)
        }
    }

    @Test
    fun valuesRow() {
        // In any language: a Swedish phone prints 0.42, not 0,42.
        for (locale in listOf(Locale.ROOT, Locale.forLanguageTag("sv-SE"))) {
            Locale.setDefault(locale)
            for (c in vectors.getJSONArray("values_row").objects()) {
                val items = valueItems(c.getJSONObject("values"), c.getString("format"), c.optString("warning").ifEmpty { null })
                val expect = c.getJSONArray("expect").objects()
                val name = c.getString("name")
                assertEquals(name, expect.map { it.getString("text") }, items.map { it.text })
                expect.zip(items).forEach { (e, item) ->
                    assertEquals(name, e.stringOrNull("flag"), item.flag)
                    assertEquals(name, Tone.valueOf(e.getString("tone").uppercase()), item.tone)
                    assertEquals(name, e.stringOrNull("group"), item.group)
                }
            }
        }
        Locale.setDefault(Locale.ROOT)
    }

    @Test
    fun dropOrder() {
        val order = vectors.getJSONArray("drop_order")
        // The top bar's order; the values row's is the same without the chip's label.
        val list = (0 until order.length()).map { order.getString(it) }
        assertEquals(list, se.eyevinn.tallycam.ui.TOP_BAR_DROP_ORDER)
        assertEquals(list - "tally_label", VALUE_DROP_ORDER)
    }

    @Test
    fun adjustValue() {
        for (c in vectors.getJSONArray("adjust_value").objects()) {
            assertEquals(c.toString(), c.getString("expect"), adjustValueText(c.getString("group"), c.getJSONObject("values"), c.getBoolean("tint")))
        }
    }

    @Test
    fun labels() {
        for (c in vectors.getJSONArray("zoom_factor").objects()) assertEquals(c.getString("expect"), zoomFactorText(c.getDouble("factor")))
        for (c in vectors.getJSONArray("bitrate").objects()) assertEquals(c.getString("expect"), bitrate(c.getInt("kbps")))
        for (c in vectors.getJSONArray("uptime").objects()) assertEquals(c.getString("expect"), uptime(c.getLong("seconds")))
        for (c in vectors.getJSONArray("timecode").objects()) {
            assertEquals(c.getString("name"), c.getString("expect"),
                         timecode(c.getLong("utc_ns"), c.getLong("utc_offset_s"), c.getInt("fps"), c.getBoolean("synced")))
        }
    }

    @Test
    fun steps() {
        for (c in vectors.getJSONArray("steps").objects()) {
            val set = tapStep(c.getString("group"), c.getJSONObject("values"), descriptors, c.getBoolean("tint"), c.getInt("direction"),
                              if (c.has("base")) c.getDouble("base") else null)
            assertEquals(c.getString("name"), plain(c.getJSONObject("expect")), set?.let(::plain))
        }
    }

    @Test
    fun neutrals() {
        for (c in vectors.getJSONArray("neutral").objects()) {
            val action = neutral(c.getString("group"), c.getJSONObject("values"), c.getBoolean("tint"))
            val name = c.toString()
            if (c.isNull("expect")) {
                assertNull(name, action)
                continue
            }
            val e = c.getJSONObject("expect")
            when {
                e.has("command") -> assertEquals(name, Neutral.Command(e.getString("command")), action)
                else -> assertEquals(name, plain(e.getJSONObject("set")), (action as Neutral.Set).values.let(::plain))
            }
        }
    }

    @Test
    fun sides() {
        val s = vectors.getJSONObject("sides")
        val default = s.getJSONObject("default")
        assertEquals(Sides(default.getString("left"), default.getString("right")), Sides.DEFAULT)
        for (c in s.getJSONArray("cases").objects()) {
            fun sides(o: JSONObject) = Sides(o.getString("left"), o.getString("right"), o.optBoolean("left_hidden"), o.optBoolean("right_hidden"))
            val start = sides(c.getJSONObject("start"))
            val available = c.optJSONArray("available")?.let { a -> (0 until a.length()).map { a.getString(it) } } ?: SIDE_GROUPS
            val shown = start.resolved(available)
            val result = when {
                c.has("choose") -> c.getJSONObject("choose").let { shown.choose(it.getString("side") == "left", it.getString("group")) }
                c.has("values_tap") -> shown.fromValues(c.getString("values_tap"))
                c.has("show") -> shown.show(c.getString("show") == "left")
                c.has("hide") -> shown.hide(c.getString("hide") == "left")
                else -> shown
            }
            assertEquals(c.getString("name"), sides(c.getJSONObject("expect")), result)
        }
    }
}
