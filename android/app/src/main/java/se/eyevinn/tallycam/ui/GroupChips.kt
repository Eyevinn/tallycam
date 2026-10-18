package se.eyevinn.tallycam.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import org.json.JSONObject
import se.eyevinn.tallycam.AppModel
import se.eyevinn.tallycam.control.Shading
import kotlin.math.pow
import kotlin.math.roundToInt

private val Yellow = Color(0xFFFFD60A)

/**
 * Exposure, colour and focus: one chip per group (`EXP AUTO`, `WB LOCKED`,
 * `FOC MANUAL`), as on iOS. Tapping a chip selects the group, whose control
 * then shows at the right edge ([GroupControl]); tapping it again hides it.
 * The camera operator and the hub both change these at any time; the chips
 * show what the camera does now.
 */
@Composable
fun GroupChips(model: AppModel, selected: String?, onSelect: (String?) -> Unit, modifier: Modifier = Modifier,
               interactive: Boolean = true) {
    val values = model.shading.values
    val descriptors = model.shading.descriptors
    Row(modifier, horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
        for ((group, modeKey) in Shading.MODE_CONTROLS) {
            if (modeKey !in descriptors) continue
            val mode = values.optString(modeKey, "auto")
            GroupChip(Shading.shortName(group), mode, group == selected,
                      if (!interactive) Modifier else Modifier.clickable { onSelect(if (group == selected) null else group) })
        }
        ExposureWarning(model.shading.exposureWarning)
    }
}

@Composable
private fun GroupChip(name: String, mode: String, selected: Boolean, modifier: Modifier) {
    val color = if (mode == "auto") Color.White else Yellow
    Row(
        modifier.background(Color.Black.copy(alpha = if (selected) 0.75f else 0.55f), CircleShape)
            .border(1.dp, if (selected) Color.White else if (mode == "auto") Color.Transparent else color.copy(alpha = 0.8f),
                    CircleShape)
            .padding(horizontal = 8.dp, vertical = 4.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Text(name, color = Color.White.copy(alpha = 0.7f), fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
        Text(mode.uppercase(), color = color, fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
    }
}

/** "TOO DARK" or "CLIPPING" from the image statistics, in amber. */
@Composable
private fun ExposureWarning(warning: String?) {
    val text = when (warning) {
        "too_dark" -> "TOO DARK"
        "clipping" -> "CLIPPING"
        else -> return
    }
    Text(text, color = Color.Black, fontSize = 11.sp, fontWeight = FontWeight.Bold,
         modifier = Modifier.background(Color(0xFFFF9F0A), CircleShape).padding(horizontal = 8.dp, vertical = 4.dp))
}

/**
 * The selected group's control, at the right edge: a round mode button (A:
 * auto, tap to go manual or back to auto with the values kept; for colour a
 * long press is white balance once) above a rocker and the value. The
 * rocker ramps the value, faster the further it is pushed (the square of
 * the deflection), about ten changes a second, clamped to the descriptor's
 * range and snapped to its step:
 * - exposure: `ev_bias` in auto, ISO (log scale) otherwise;
 * - colour: `wb_temperature` (manual white balance);
 * - focus: `focus_position`, up is far (manual focus).
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun GroupControl(model: AppModel, group: String, modifier: Modifier = Modifier) {
    val values = model.shading.values
    val modeKey = Shading.MODE_CONTROLS.firstOrNull { it.first == group }?.second ?: return
    val mode = values.optString(modeKey, "auto")
    var deflection by remember(group) { mutableFloatStateOf(0f) }
    LaunchedEffect(group) {
        // The value the ramp is at (between steps and presets), and the one last sent.
        var target: Double? = null
        var sent: Double? = null
        while (true) {
            delay(RAMP_MS)
            val d = deflection
            if (d == 0f) {
                target = null
                sent = null
                continue
            }
            val ramp = ramp(model, group) ?: continue
            val descriptor = model.shading.descriptors[ramp.id] ?: continue
            val min = descriptor.optDouble("min", Double.NaN)
            val max = descriptor.optDouble("max", Double.NaN)
            if (min.isNaN() || max.isNaN() || max <= min) continue
            val start = target ?: model.shading.values.optDouble(ramp.id, Double.NaN).takeIf { !it.isNaN() } ?: continue
            val rate = (if (d > 0) 1 else -1) * d * d * ramp.perSecond * RAMP_MS / 1000.0
            val next = (if (ramp.log) start * 2.0.pow(rate) else start + rate).coerceIn(min, max)
            target = next
            val step = descriptor.optDouble("step", 0.0)
            val snapped = (if (step > 0) (next / step).roundToInt() * step else next).coerceIn(min, max)
            if (snapped != sent) {
                sent = snapped
                model.localSet(JSONObject().put(ramp.id, snapped), quiet = true)
            }
        }
    }
    Column(modifier, horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        val auto = mode == "auto"
        Box(
            Modifier.size(46.dp).background(if (auto) Color.White.copy(alpha = 0.9f) else Color.Black.copy(alpha = 0.35f), CircleShape)
                .border(1.dp, if (auto) Color.Transparent else Yellow.copy(alpha = 0.8f), CircleShape)
                .combinedClickable(
                    onClick = { toggleAuto(model, group, modeKey, mode) },
                    onLongClick = if (group == "white_balance") ({ model.localWhiteBalanceOnce() }) else null,
                ),
            contentAlignment = Alignment.Center,
        ) {
            Text("A", color = if (auto) Color.Black else Color.White.copy(alpha = 0.85f), fontSize = 18.sp,
                 fontWeight = FontWeight.Bold)
        }
        val (top, bottom) = if (group == "focus") "∞" to "N" else "+" to "−"
        Rocker(top, bottom, height = 150.dp) { deflection = it }
        Text(valueLabel(group, values), color = if (auto) Color.White else Yellow, fontSize = 12.sp,
             fontFamily = FontFamily.Monospace, fontWeight = FontWeight.SemiBold,
             modifier = Modifier.background(Color.Black.copy(alpha = 0.55f), CircleShape).padding(horizontal = 8.dp, vertical = 3.dp))
    }
}

/** Which control a group's rocker ramps, and how fast at full deflection (per second; doublings when log). */
private class Ramp(val id: String, val perSecond: Double, val log: Boolean = false)

/** The control the rocker moves now, switching the group to manual first where the control needs it. */
private fun ramp(model: AppModel, group: String): Ramp? {
    val values = model.shading.values
    val descriptors = model.shading.descriptors
    fun manual(modeKey: String) {
        if (values.optString(modeKey) != "manual") model.localSet(JSONObject().put(modeKey, "manual"), quiet = true)
    }
    return when (group) {
        "exposure" -> when {
            values.optString("exposure_mode") == "auto" -> if ("ev_bias" in descriptors) Ramp("ev_bias", 2.0) else null
            "iso" in descriptors -> { manual("exposure_mode"); Ramp("iso", 2.0, log = true) }
            else -> null
        }
        "white_balance" -> if ("wb_temperature" in descriptors) { manual("wb_mode"); Ramp("wb_temperature", 2000.0) } else null
        "focus" -> if ("focus_position" in descriptors) { manual("focus_mode"); Ramp("focus_position", 0.5) } else null
        else -> null
    }
}

/**
 * The A button: auto to manual with the values kept, anything else back to
 * auto. Colour goes to locked, which keeps what auto white balance found
 * (Camera2's manual white balance is a preset); exposure to locked where
 * the camera has no manual sensor.
 */
private fun toggleAuto(model: AppModel, group: String, modeKey: String, mode: String) {
    val options = model.shading.descriptors[modeKey]?.optJSONArray("options")
    val offered = (0 until (options?.length() ?: 0)).mapNotNull { options?.optJSONObject(it)?.optString("value") }
    val next = when {
        mode != "auto" -> "auto"
        group == "white_balance" -> "locked"
        "manual" in offered -> "manual"
        else -> "locked"
    }
    model.localSet(JSONObject().put(modeKey, next))
}

private fun valueLabel(group: String, v: JSONObject): String = when (group) {
    "exposure" -> if (v.optString("exposure_mode") == "auto" && v.has("ev_bias")) "%+.1f EV".format(v.optDouble("ev_bias"))
                  else if (v.has("iso")) "ISO ${v.optInt("iso")}" else v.optString("exposure_mode").uppercase()
    "white_balance" -> when (v.optString("wb_mode")) {
        "manual" -> if (v.has("wb_temperature")) "${v.optInt("wb_temperature")} K" else "MANUAL"
        "locked" -> "LOCKED"
        else -> "AWB"
    }
    else -> if (v.optString("focus_mode") == "auto" || !v.has("focus_position")) "AF" else "%.2f".format(v.optDouble("focus_position"))
}

/**
 * The camera's values in one line at the bottom, as a cinema camera shows
 * them: format, ISO, shutter, exposure bias (when not 0), white balance and
 * focus. A value whose group is locked or manual is yellow. On a narrow
 * screen focus goes first, then white balance, instead of wrapping.
 */
@Composable
fun CameraReadout(model: AppModel, modifier: Modifier = Modifier) {
    val v = model.shading.values
    val exposure = if (v.optString("exposure_mode", "auto") == "auto") Color.White else Yellow
    val items = mutableListOf<Pair<String, Color>>()
    // Index in `items` of white balance and focus, the ones dropped first.
    var wb = -1
    var focus = -1
    items += model.shading.format to Color.White
    if (v.has("iso")) items += "ISO ${v.optInt("iso")}" to exposure
    if (v.has("shutter")) items += "1/${v.optInt("shutter")}" to exposure
    val bias = v.optDouble("ev_bias", 0.0)
    if (v.optString("exposure_mode") == "auto" && kotlin.math.abs(bias) >= 0.05) items += "%+.1fEV".format(bias) to exposure
    if (v.has("wb_mode")) {
        wb = items.size
        items += when (v.optString("wb_mode")) {
            "manual" -> (if (v.has("wb_temperature")) "${v.optInt("wb_temperature")}K" else "WB") to Yellow
            "locked" -> "AWB" to Yellow
            else -> "AWB" to Color.White
        }
    }
    if (v.has("focus_mode")) {
        focus = items.size
        items += (if (v.optString("focus_mode") == "auto" || !v.has("focus_position")) "AF" to Color.White
                  else "F %.2f".format(v.optDouble("focus_position")) to Yellow)
    }
    Box(modifier.background(Color.Black.copy(alpha = 0.5f), CircleShape).padding(horizontal = 12.dp, vertical = 4.dp)) {
        DroppingRow(listOf(focus, wb).filter { it >= 0 }) {
            items.forEachIndexed { i, (text, color) ->
                Text((if (i == 0) "" else "  ") + text, color = color, fontSize = 12.sp, fontFamily = FontFamily.Monospace,
                     fontWeight = FontWeight.SemiBold, maxLines = 1, softWrap = false)
            }
        }
    }
}

/** A row on one line: when it does not fit, the children at `drop` (in that order) are left out. */
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

/** How often a held rocker changes its value (about 10 Hz). */
private const val RAMP_MS = 100L
