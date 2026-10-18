package se.eyevinn.tallycam.ui

import android.os.SystemClock
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject
import se.eyevinn.tallycam.AppModel
import se.eyevinn.tallycam.control.Shading
import kotlinx.coroutines.delay
import kotlin.math.abs

/** A change the hub just made (docs/UI_DESIGN.md). */
private val HUB_CYAN = Color(0xFF64D2FF)

/**
 * The camera's settings in one line under the status line, like a cinema
 * camera's info line, as on iOS (ios/TallyCam/Views/CameraValues.swift):
 * format, ISO, shutter, compensation (auto, when not 0), white balance and
 * focus. It is also where the operator goes to change them: ISO, shutter
 * and EV open exposure, the white balance colour, AF or F focus, the format
 * Settings (docs/UI_DESIGN.md, "Values"). A value whose group is locked or
 * manual is yellow, auto is white; the group the adjust control has is
 * underlined. An exposure warning colours ISO and follows it ("ISO 100
 * DARK", "CLIP"). Follows
 * the values the hub sets too, and a value the hub just changed flashes
 * cyan for a second. On a narrow screen focus goes first, then
 * white balance; the line never wraps.
 *
 * Android's manual white balance is an AWB preset and its locked one keeps
 * what auto found: "AWB" unless a temperature is set.
 */
@Composable
fun CameraValuesRow(model: AppModel, selection: String?, onSelect: (String) -> Unit, onFormat: () -> Unit,
                    modifier: Modifier = Modifier) {
    val items = items(model)
    val descriptors = model.shading.descriptors
    // What the hub changed in the last second, until the flash is over.
    val hubChanged = model.shading.hubChanged
    var flashOver by remember { mutableStateOf(false) }
    LaunchedEffect(hubChanged) {
        flashOver = false
        delay(Shading.HUB_FLASH_MS)
        flashOver = true
    }
    val now = SystemClock.uptimeMillis()
    val flashing = if (flashOver) emptySet() else hubChanged.filterValues { now - it < Shading.HUB_FLASH_MS }.keys
    Box(modifier.height(Chrome.rowHeight).background(Color.Black.copy(alpha = 0.45f), CircleShape)
            .padding(horizontal = 4.dp), contentAlignment = Alignment.Center) {
        // Focus is last; colour is dropped after it.
        val drop = listOfNotNull(items.indexOfFirst { it.group == "focus" }.takeIf { it >= 0 },
                                 items.indexOfFirst { it.group == "white_balance" }.takeIf { it >= 0 })
        DroppingRow(drop) {
            items.forEachIndexed { index, item ->
                val enabled = item.group == null || modeKey(item.group) in descriptors
                ValueButton(item, separator = index > 0, selected = item.group != null && item.group == selection,
                            enabled = enabled, flash = item.ids.any { it in flashing }) {
                    if (item.group == null) onFormat() else onSelect(item.group)
                }
            }
        }
    }
}

private class Item(val text: String, val flag: String? = null, val tone: Color = Color.White,
                   /** The group it opens; null opens Settings. */
                   val group: String? = null,
                   /** The controls it shows: it flashes when the hub changes one. */
                   val ids: List<String> = emptyList())

@Composable
private fun ValueButton(item: Item, separator: Boolean, selected: Boolean, enabled: Boolean, flash: Boolean,
                        onClick: () -> Unit) {
    val tone by animateColorAsState(if (flash) HUB_CYAN else item.tone, tween(if (flash) 80 else 400), label = "flash")
    Row(Modifier.height(Chrome.rowHeight), verticalAlignment = Alignment.CenterVertically) {
        if (separator) Box(Modifier.width(1.dp).height(14.dp).background(Color.White.copy(alpha = 0.25f)))
        Box(
            Modifier.fillMaxHeight().clickable(enabled = enabled, onClick = onClick)
                .semantics { contentDescription = listOfNotNull(item.text, item.flag).joinToString(" ") }
                .padding(horizontal = 9.dp),
            contentAlignment = Alignment.Center,
        ) {
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(item.text, color = tone, fontSize = 14.sp, fontFamily = FontFamily.Monospace,
                     fontWeight = FontWeight.SemiBold, maxLines = 1, softWrap = false)
                item.flag?.let {
                    Text(it, color = tone, fontSize = 14.sp, fontWeight = FontWeight.Black, maxLines = 1, softWrap = false)
                }
            }
            if (selected) {
                Box(Modifier.align(Alignment.BottomCenter).padding(bottom = 4.dp).width(24.dp).height(2.dp)
                        .background(Color.White, CircleShape))
            }
        }
    }
}

private fun items(model: AppModel): List<Item> {
    val v = model.shading.values
    val exposureHeld = v.has("exposure_mode") && v.optString("exposure_mode") != "auto"
    val held = if (exposureHeld) Chrome.yellow else Color.White
    val list = mutableListOf<Item>()
    if (model.shading.format.isNotEmpty()) list += Item(model.shading.format, ids = listOf("resolution", "frame_rate"))
    if (v.has("iso")) {
        val iso = "ISO ${v.optInt("iso")}"
        val ids = listOf("iso", "exposure_mode")
        list += when (model.shading.exposureWarning) {
            "too_dark" -> Item(iso, "DARK", Chrome.orange, "exposure", ids)
            "clipping" -> Item(iso, "CLIP", Color(0xFFFF453A), "exposure", ids)
            else -> Item(iso, tone = held, group = "exposure", ids = ids)
        }
    }
    v.optDouble("shutter", 0.0).takeIf { it > 0 }?.let { list += Item("1/${kotlin.math.round(it).toLong()}", tone = held, group = "exposure", ids = listOf("shutter")) }
    val bias = v.optDouble("ev_bias", 0.0)
    if (!exposureHeld && abs(bias) >= 0.05) list += Item("%+.1f EV".format(bias), group = "exposure", ids = listOf("ev_bias"))
    if (v.has("wb_mode")) list += colour(v)
    if (v.has("focus_mode")) {
        val ids = listOf("focus_mode", "focus_position")
        list += when (v.optString("focus_mode")) {
            "auto" -> Item("AF", group = "focus", ids = ids)
            "locked" -> Item("AF-L", tone = Chrome.yellow, group = "focus", ids = ids)
            else -> Item(if (v.has("focus_position")) "F %.2f".format(v.optDouble("focus_position")) else "MF",
                         tone = Chrome.yellow, group = "focus", ids = ids)
        }
    }
    return list
}

/** "5200 K" (with the tint when not 0) while manual, else AWB. */
private fun colour(v: JSONObject): Item {
    val mode = v.optString("wb_mode")
    val tone = if (mode == "auto") Color.White else Chrome.yellow
    val ids = listOf("wb_mode", "wb_temperature", "wb_tint")
    if (mode != "manual" || !v.has("wb_temperature")) return Item("AWB", tone = tone, group = "white_balance", ids = ids)
    val tint = v.optDouble("wb_tint", 0.0).takeIf { abs(it) >= 0.5 }?.let { " %+.0f".format(it) } ?: ""
    return Item("${v.optInt("wb_temperature")} K$tint", tone = tone, group = "white_balance", ids = ids)
}

/** The mode control of a shading group. */
internal fun modeKey(group: String): String = Shading.MODE_CONTROLS.first { it.first == group }.second

/** A row on one line: when it does not fit, the children at [drop] (in that order) are left out. */
@Composable
private fun DroppingRow(drop: List<Int>, content: @Composable () -> Unit) {
    Layout(content) { measurables, constraints ->
        val placeables = measurables.map { it.measure(Constraints()) }
        val shown = placeables.indices.toMutableSet()
        for (i in drop) {
            if (shown.sumOf { placeables[it].width } <= constraints.maxWidth) break
            shown -= i
        }
        val width = shown.sumOf { placeables[it].width }.coerceAtMost(constraints.maxWidth)
        layout(width, placeables.maxOfOrNull { it.height } ?: 0) {
            var x = 0
            for (i in placeables.indices) if (i in shown) {
                placeables[i].placeRelative(x, 0)
                x += placeables[i].width
            }
        }
    }
}
