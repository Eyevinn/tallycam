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
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.runtime.mutableDoubleStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import org.json.JSONObject
import se.eyevinn.tallycam.AppModel
import kotlin.math.log2
import kotlin.math.pow
import kotlin.math.roundToLong

/**
 * The adjust control at the right edge, always there, mirroring zoom on
 * the left (docs/UI_DESIGN.md, "Adjust control"; iOS
 * ios/TallyCam/Views/AdjustControl.swift). It adjusts the group picked in
 * the camera values row: the mode as an A / M switch (M is any held mode,
 * manual or locked; both always shown, a tap on the other switches,
 * keeping the values) under a caption (EXP, WB, FOCUS), a rocker as long as
 * the zoom rocker (up is more; hidden while the screen is [locked]), and the
 * value, which grows while adjusting. The rocker ramps the value while
 * dragged, steps it on a tap on an end and goes to the neutral value on a
 * double tap of the knob (the table in docs/UI_DESIGN.md), with a tick per
 * step and a firmer one at the end of the range. The caption, the mode
 * switch and the value sit above and below the rocker where [maxHeight]
 * allows, else beside it on the inner side. Exposure ramps the bias in auto
 * and ISO otherwise; colour the temperature (or the tint, with the K / Tint
 * switch, where the camera has one); focus the lens position (up is far).
 * Moving colour or focus while auto switches them to manual first. A long
 * press on colour's mode switch is "white balance once". The operator and
 * the hub may both change the values; the last change wins
 * (docs/SHADING.md).
 *
 * Android's colour goes to locked from A (what auto white balance found;
 * Camera2's manual white balance is a preset); exposure to locked where the
 * camera has no manual sensor.
 */
@Composable
fun AdjustControl(model: AppModel, group: String, rockerLength: Dp, maxHeight: Dp, locked: Boolean,
                  modifier: Modifier = Modifier) {
    val descriptors = model.shading.descriptors
    if (modeKey(group) !in descriptors) return
    val hasTint = group == "white_balance" && "wb_tint" in descriptors
    // K each time colour is opened.
    var tint by remember(group) { mutableStateOf(false) }
    var rate by remember(group) { mutableDoubleStateOf(0.0) }
    val ramping by rememberUpdatedState(ramped(model, group, tint))
    val haptics = rememberHaptics()
    // When a value last changed from here: the value label stands out a moment longer.
    var touched by remember(group) { mutableLongStateOf(0L) }
    var recent by remember(group) { mutableStateOf(false) }
    LaunchedEffect(touched) {
        if (touched == 0L) return@LaunchedEffect
        recent = true
        delay(1000)
        recent = false
    }

    /** Clamps [raw] to the control's range, snaps it to its step and sends it (with the mode first where needed); null when out of reach. */
    fun send(r: Ramped, raw: Double, last: Double? = null): Double? {
        val d = model.shading.descriptors[r.id] ?: return null
        val low = d.optDouble("min", Double.NaN)
        val high = d.optDouble("max", Double.NaN)
        if (low.isNaN() || high.isNaN() || high <= low) return null
        var value = raw
        val step = d.optDouble("step", 0.0)
        if (step > 0) value = (value / step).roundToLong() * step
        value = value.coerceIn(low, high)
        if (value == last) return value
        val set = JSONObject().put(r.id, value)
        r.mode?.let { set.put(modeKey(group), it) }
        model.localSet(set, quiet = true)
        haptics.step(atEnd = value <= low || value >= high)
        touched = android.os.SystemClock.uptimeMillis()
        return value
    }

    LaunchedEffect(group) {
        // The ramp's own position in the control's scale (log2 for ISO),
        // unsnapped, and the value last sent; reset when the rocker is let
        // go or what it moves changes.
        var position: Double? = null
        var sent: Double? = null
        var positionId: String? = null
        while (true) {
            delay(RAMP_MS)
            val r = ramping
            if (rate == 0.0 || r == null) {
                position = null
                sent = null
                continue
            }
            val d = model.shading.descriptors[r.id] ?: continue
            val low = d.optDouble("min", Double.NaN)
            val high = d.optDouble("max", Double.NaN)
            if (low.isNaN() || high.isNaN() || high <= low || (r.log && low <= 0)) continue
            if (positionId != r.id) {
                positionId = r.id
                position = null
                sent = null
            }
            val current = position ?: r.scale(model.shading.values.optDouble(r.id, (low + high) / 2))
            val next = (current + rate * r.speed * RAMP_MS / 1000.0).coerceIn(r.scale(low), r.scale(high))
            position = next
            sent = send(r, r.unscale(next), sent) ?: sent
        }
    }
    /** A tap on an end: one step up (+1) or down (-1). */
    fun step(direction: Int) {
        val r = ramping ?: return
        val current = model.shading.values.optDouble(r.id, Double.NaN).takeIf { !it.isNaN() } ?: return
        if (r.log && current <= 0) return
        send(r, r.unscale(r.scale(current) + direction * r.tap))
    }
    /** A double tap on the knob: the neutral value (docs/UI_DESIGN.md). */
    fun reset() {
        val mode = model.shading.values.optString(modeKey(group), "auto")
        when {
            group == "white_balance" && tint -> ramping?.let { send(it, 0.0) }
            group == "white_balance" -> model.localWhiteBalanceOnce()
            group == "exposure" && mode == "auto" -> ramping?.let { send(it, 0.0) }
            mode != "auto" -> model.localSet(JSONObject().put(modeKey(group), "auto"))
        }
        haptics.step()
        touched = android.os.SystemClock.uptimeMillis()
    }

    val mode = model.shading.values.optString(modeKey(group), "auto")
    val auto = mode == "auto"
    val text = valueText(group, model.shading.values, tint)
    val adjusting = rate != 0.0 || recent

    val caption = @Composable {
        Text(caption(group), color = Color.White.copy(alpha = 0.85f), fontSize = 11.sp, fontWeight = FontWeight.Bold,
             letterSpacing = 0.5.sp, maxLines = 1,
             modifier = Modifier.background(Color.Black.copy(alpha = 0.55f), CircleShape).padding(horizontal = 7.dp, vertical = 2.dp))
    }
    val modeButton = @Composable { ModeSwitch(model, group, mode) }
    val tintSwitch = @Composable { if (hasTint) TintSwitch(tint) { tint = it; rate = 0.0 } }
    val rocker = @Composable {
        // Hidden while the screen is locked, like zoom.
        if (!locked) {
            Rocker(highLabel(group, tint), lowLabel(group, tint), "${title(group)} ${ramping?.label ?: ""}", rockerLength,
                   onStep = ::step, onReset = ::reset) { rate = it }
        }
    }
    val value = @Composable {
        // Grows and turns bright while adjusting.
        val grow by animateFloatAsState(if (adjusting) 1.3f else 1f, label = "value")
        val tone = if (auto) Color.White else Chrome.yellow
        Text(text, color = if (adjusting) tone else tone.copy(alpha = 0.85f), fontSize = 13.sp, fontFamily = FontFamily.Monospace,
             fontWeight = FontWeight.SemiBold, maxLines = 1, softWrap = false,
             modifier = Modifier.graphicsLayer { scaleX = grow; scaleY = grow }
                 .background(Color.Black.copy(alpha = if (adjusting) 0.75f else 0.55f), CircleShape)
                 .padding(horizontal = 8.dp, vertical = 5.dp))
    }
    val stacked = 18.dp + Chrome.gap + Chrome.rowHeight * 2 + 8.dp + (if (hasTint) Chrome.rowHeight - 6.dp + Chrome.gap else 0.dp) + Chrome.gap +
        rockerLength + Chrome.gap + Chrome.rowHeight - 6.dp
    if (stacked <= maxHeight) {
        Column(modifier, horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(Chrome.gap)) {
            caption(); modeButton(); tintSwitch(); rocker(); value()
        }
    } else {
        Row(modifier, horizontalArrangement = Arrangement.spacedBy(Chrome.gap), verticalAlignment = Alignment.CenterVertically) {
            Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(Chrome.gap)) {
                caption(); modeButton(); tintSwitch(); value()
            }
            rocker()
        }
    }
}

/**
 * The group's mode as a vertical two-way switch, [A] over [M]: both always
 * shown, the one in use lit (white for auto, yellow for held), a tap on the
 * other switches to it, keeping the values. M is any held mode: manual,
 * locked (by the hub, or focus locked on the picture), or the lock a camera
 * uses where it has no manual (Android's white balance). A long press on
 * colour's is "white balance once".
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ModeSwitch(model: AppModel, group: String, mode: String) {
    val auto = mode == "auto"
    val held = if (mode == "locked") "locked" else heldMode(model, group)
    val wbOnce: (() -> Unit)? = if (group == "white_balance") ({ model.localWhiteBalanceOnce() }) else null
    Column(
        Modifier.width(Chrome.control).background(Color.Black.copy(alpha = 0.35f), RoundedCornerShape(Chrome.control / 2))
            .border(1.dp, Color.White.copy(alpha = 0.25f), RoundedCornerShape(Chrome.control / 2)).padding(3.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        for ((value, label) in listOf("auto" to "A", held to "M")) {
            val on = (value == "auto") == auto
            Box(
                Modifier.fillMaxWidth().height(Chrome.rowHeight)
                    .background(if (!on) Color.Transparent else if (auto) Color.White else Chrome.yellow, CircleShape)
                    .combinedClickable(
                        onClick = { if (!on) model.localSet(JSONObject().put(modeKey(group), value)) },
                        onLongClick = wbOnce,
                        onLongClickLabel = wbOnce?.let { "White balance once" },
                    )
                    .semantics {
                        contentDescription = "${title(group)} ${if (value == "auto") "auto" else value}"
                        selected = on
                    },
                contentAlignment = Alignment.Center,
            ) {
                Text(label, color = if (on) Color.Black else Color.White.copy(alpha = 0.7f), fontSize = 17.sp,
                     fontWeight = FontWeight.Bold)
            }
        }
    }
}

/** K or Tint for colour's rocker. */
@Composable
private fun TintSwitch(tint: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.background(Color.Black.copy(alpha = 0.45f), CircleShape).padding(2.dp)) {
        for ((title, on) in listOf("K" to !tint, "Tint" to tint)) {
            Text(title, color = if (on) Color.Black else Color.White.copy(alpha = 0.8f), fontSize = 12.sp, fontWeight = FontWeight.Bold,
                 modifier = Modifier.height(Chrome.rowHeight - 10.dp).background(if (on) Color.White else Color.Transparent, CircleShape)
                     .clickable { if (!on) onChange(title == "Tint") }
                     .semantics { contentDescription = if (title == "K") "Rocker moves the temperature" else "Rocker moves the tint" }
                     .padding(horizontal = 8.dp, vertical = 5.dp))
        }
    }
}

/**
 * The held mode the A button goes to: colour locked (what auto found),
 * otherwise manual where the camera has it, else locked.
 */
private fun heldMode(model: AppModel, group: String): String {
    if (group == "white_balance") return "locked"
    val options = model.shading.descriptors[modeKey(group)]?.optJSONArray("options")
    val offered = (0 until (options?.length() ?: 0)).map { options?.optJSONObject(it)?.optString("value") }
    return if ("manual" in offered) "manual" else "locked"
}

/**
 * What the rocker moves: full speed in the control's scale per second, a
 * tap's step in the same scale (log2 for ISO), and the mode to switch to
 * first.
 */
private class Ramped(val id: String, val label: String, val speed: Double, val tap: Double, val log: Boolean = false,
                     val mode: String? = null) {
    fun scale(x: Double) = if (log) log2(x) else x
    fun unscale(x: Double) = if (log) 2.0.pow(x) else x
}

private fun ramped(model: AppModel, group: String, tint: Boolean): Ramped? {
    val descriptors = model.shading.descriptors
    val mode = model.shading.values.optString(modeKey(group), "auto")
    val manual = if (mode == "manual") null else "manual"
    val r = when (group) {
        "exposure" -> if (mode == "auto") Ramped("ev_bias", "compensation", 2.0, tap = 0.3)
                      else Ramped("iso", "ISO", 2.0, tap = 1.0 / 3, log = true, mode = manual)
        "white_balance" -> if (tint) Ramped("wb_tint", "tint", 60.0, tap = 1.0, mode = manual)
                           else Ramped("wb_temperature", "temperature", 1500.0, tap = 100.0, mode = manual)
        else -> Ramped("focus_position", "position", 0.4, tap = 0.02, mode = manual)
    }
    return r.takeIf { it.id in descriptors }
}

/** Above the mode switch, so the rocker's job is never a guess. */
private fun caption(group: String) = when (group) {
    "exposure" -> "EXP"
    "white_balance" -> "WB"
    else -> "FOCUS"
}

private fun title(group: String) = when (group) {
    "exposure" -> "Exposure"
    "white_balance" -> "White balance"
    else -> "Focus"
}

private fun highLabel(group: String, tint: Boolean) = when (group) {
    "exposure" -> "+"
    "white_balance" -> if (tint) "M" else "K+"
    else -> "Far"
}

private fun lowLabel(group: String, tint: Boolean) = when (group) {
    "exposure" -> "−"
    "white_balance" -> if (tint) "G" else "K−"
    else -> "Near"
}

/** What the rocker moves, as it stands. */
private fun valueText(group: String, v: JSONObject, tint: Boolean): String {
    val auto = v.optString(modeKey(group), "auto") == "auto"
    return when (group) {
        "exposure" -> if (auto) "%+.1f EV".format(v.optDouble("ev_bias", 0.0))
                      else if (v.has("iso")) "ISO ${v.optInt("iso")}" else "ISO"
        "white_balance" -> when {
            tint -> if (v.has("wb_tint")) "Tint %+.0f".format(v.optDouble("wb_tint")) else "Tint"
            v.optString("wb_mode") == "manual" && v.has("wb_temperature") -> "${v.optInt("wb_temperature")} K"
            else -> "AWB"
        }
        else -> if (auto) "AF" else if (v.has("focus_position")) "F %.2f".format(v.optDouble("focus_position")) else "F"
    }
}

/** How often a held rocker changes its value (about 10 Hz). */
private const val RAMP_MS = 100L
