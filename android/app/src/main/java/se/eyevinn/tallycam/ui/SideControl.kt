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
import androidx.compose.animation.core.tween
import androidx.compose.ui.layout.Layout
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
 * A side control (docs/UI_DESIGN.md, "Side control"; iOS
 * ios/TallyCam/Views/AdjustControl.swift): one at each edge, mirror
 * images, each running one of ZOOM, EXP, WB, FOC. The rocker sits at the
 * screen edge ([leading]: the left one), as long as the selector and what
 * is under it, the same length on both sides. On its inner side: the
 * selector beside its upper half (a tap puts that group on this side; the
 * other side's group swaps), and beside its lower half the A / M mode
 * switch (M is any held mode, manual or locked; a tap on the other segment
 * switches, keeping the values), or for zoom the lens buttons. Under the
 * rocker the value (zoom: the factor against the wide lens), which grows
 * while adjusting, and for white balance the K / Tint switch. While the
 * screen is [locked] the rocker hides, its place kept.
 *
 * The rocker ramps the value while dragged, steps it on a tap on an end and
 * goes to the neutral value on a double tap of the knob (the table in
 * docs/UI_DESIGN.md), with a tick per step and a firmer one at the end of
 * the range. Exposure ramps the bias in auto and ISO otherwise; colour the
 * temperature (or the tint, where the camera has one); focus the lens
 * position (up is far); zoom in doublings. Moving colour or focus while
 * auto switches them to manual first. A long press on colour's mode switch
 * is "white balance once". The operator and the hub may both change the
 * values; the last change wins (docs/SHADING.md).
 *
 * Android's colour goes to locked from A (what auto white balance found;
 * Camera2's manual white balance is a preset); exposure to locked where the
 * camera has no manual sensor.
 */
@Composable
fun SideControl(model: AppModel, group: String, groups: List<String>, leading: Boolean, metrics: SideMetrics,
                locked: Boolean, onSelect: (String) -> Unit, modifier: Modifier = Modifier) {
    val zoom = group == "zoom"
    val descriptors = model.shading.descriptors
    val hasTint = group == "white_balance" && "wb_tint" in descriptors
    // K each time colour is chosen.
    var tint by remember(group) { mutableStateOf(false) }
    var rate by remember(group) { mutableDoubleStateOf(0.0) }
    val ramping by rememberUpdatedState(if (zoom) null else ramped(model, group, tint))
    val haptics = rememberHaptics()
    // When a value last changed from here: the value label stands out a moment longer.
    var touched by remember(group) { mutableLongStateOf(0L) }
    var recent by remember(group) { mutableStateOf(false) }
    LaunchedEffect(touched) {
        if (touched == 0L) return@LaunchedEffect
        recent = true
        delay(800)
        recent = false
    }
    // The value a quick tap builds on: the camera reports a step back a moment later (as iOS).
    var lastTap by remember(group) { mutableStateOf<Pair<Long, Double>?>(null) }

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
        if (zoom) return@LaunchedEffect
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
    /** The value a step starts from: the last tap's within a second, else the camera's. */
    fun base(id: String, fallback: Double?): Double? {
        val now = android.os.SystemClock.uptimeMillis()
        lastTap?.let { (at, v) -> if (now - at < 1000) return v }
        return model.shading.values.optDouble(id, Double.NaN).takeIf { !it.isNaN() } ?: fallback
    }
    fun zoomTo(factor: Double) {
        val d = model.shading.descriptors["zoom"] ?: return
        val low = d.optDouble("min", 1.0)
        val high = d.optDouble("max", 1.0)
        val value = (Math.round(factor * 100) / 100.0).coerceIn(low, high)
        model.localSet(JSONObject().put("zoom", value), quiet = true)
        haptics.step(atEnd = value <= low || value >= high)
        lastTap = android.os.SystemClock.uptimeMillis() to value
        touched = android.os.SystemClock.uptimeMillis()
    }
    /** A tap on an end: one step up (+1) or down (-1). */
    fun step(direction: Int) {
        if (zoom) {
            base("zoom", 1.0)?.let { zoomTo(it * 2.0.pow(ZOOM_TAP * direction)) }
            return
        }
        val r = ramping ?: return
        // No value yet (Android's auto white balance reports no temperature):
        // from the middle of the range, as the drag does.
        val d = model.shading.descriptors[r.id] ?: return
        val current = base(r.id, ((d.optDouble("min") + d.optDouble("max")) / 2).takeIf { !it.isNaN() }) ?: return
        if (r.log && current <= 0) return
        send(r, r.unscale(r.scale(current) + direction * r.tap))?.let { lastTap = android.os.SystemClock.uptimeMillis() to it }
    }
    /** A double tap on the knob: the neutral value (docs/UI_DESIGN.md). */
    fun reset() {
        lastTap = null
        if (zoom) {
            zoomTo(1.0)
            return
        }
        val mode = model.shading.values.optString(modeKey(group), "auto")
        when {
            // Tint 0, the white balance mode as it is (docs/UI_DESIGN.md).
            group == "white_balance" && tint -> ramping?.let { send(Ramped(it.id, it.label, it.speed, it.tap), 0.0) }
            group == "white_balance" -> model.localWhiteBalanceOnce()
            group == "exposure" && mode == "auto" -> ramping?.let { send(it, 0.0) }
            mode != "auto" -> model.localSet(JSONObject().put(modeKey(group), "auto"))
        }
        haptics.step()
        touched = android.os.SystemClock.uptimeMillis()
    }

    // The zoom drag ticks only at its ends: once when it reaches one.
    val zoomValue = model.shading.values.optDouble("zoom", 1.0)
    var zoomAtEnd by remember { mutableStateOf(false) }
    LaunchedEffect(zoomValue, rate) {
        if (!zoom || rate == 0.0) { zoomAtEnd = false; return@LaunchedEffect }
        val d = model.shading.descriptors["zoom"] ?: return@LaunchedEffect
        val atEnd = zoomValue <= d.optDouble("min", 1.0) + 0.01 || zoomValue >= d.optDouble("max", 1.0) - 0.01
        if (atEnd && !zoomAtEnd) haptics.step(atEnd = true)
        zoomAtEnd = atEnd
    }

    val mode = if (zoom) "auto" else model.shading.values.optString(modeKey(group), "auto")
    val auto = mode == "auto"
    val text = if (zoom) zoomFactorText(model) else valueText(group, model.shading.values, tint)
    val adjusting = rate != 0.0 || recent

    val rocker = @Composable {
        if (zoom) {
            Rocker("T", "W", "Zoom", metrics.rocker, Chrome.groupColor(group), onStep = ::step, onReset = ::reset) {
                rate = it
                model.rampZoom(it * MAX_ZOOM_RATE)
            }
        } else {
            Rocker(highLabel(group, tint), lowLabel(group, tint), "${title(group)} ${ramping?.label ?: ""}", metrics.rocker,
                   Chrome.groupColor(group), onStep = ::step, onReset = ::reset) { rate = it }
        }
    }
    val value = @Composable {
        // Grows and turns bright while adjusting (as iOS: x1.25, a moment after a tap too).
        val grow by animateFloatAsState(if (adjusting) 1.25f else 1f, tween(150), label = "value")
        val tone = if (auto) Color.White else Chrome.yellow
        Box(Modifier.height(Chrome.valueHeight).graphicsLayer { scaleX = grow; scaleY = grow }
                .background(Color.Black.copy(alpha = if (adjusting) 0.75f else 0.55f), CircleShape)
                .padding(horizontal = 8.dp), contentAlignment = Alignment.Center) {
            Text(text, color = if (adjusting) tone else tone.copy(alpha = 0.8f), fontSize = 13.sp, style = Chrome.tabular,
                 fontWeight = FontWeight.SemiBold, maxLines = 1, softWrap = false)
        }
    }
    val inner = @Composable {
        Column(Modifier.height(metrics.rocker), verticalArrangement = Arrangement.SpaceBetween) {
            GroupSelector(groups, group, metrics.segment, onSelect)
            when {
                zoom -> LensSwitch(model, metrics.segment)
                modeKey(group) in descriptors -> ModeSwitch(model, group, mode, metrics.segment)
            }
        }
    }
    Column(modifier, horizontalAlignment = if (leading) Alignment.Start else Alignment.End,
           verticalArrangement = Arrangement.spacedBy(Chrome.gap)) {
        Row(horizontalArrangement = Arrangement.spacedBy(Chrome.gap)) {
            // The rocker at the screen edge; hidden while the screen is locked, its place kept.
            val edge = @Composable { if (locked) Box(Modifier.width(Chrome.control).height(metrics.rocker)) else rocker() }
            if (leading) { edge(); inner() } else { inner(); edge() }
        }
        // The value under the rocker; the K / Tint switch under it takes no
        // room, so both sides stay level.
        Layout({ value(); if (hasTint) TintSwitch(tint) { tint = it; rate = 0.0 } }) { measurables, constraints ->
            val loose = constraints.copy(minWidth = 0, minHeight = 0)
            val placeables = measurables.map { it.measure(loose) }
            val width = placeables.maxOf { it.width }
            val first = placeables.first()
            fun x(w: Int) = if (leading) 0 else width - w
            layout(width, first.height) {
                first.place(x(first.width), 0)
                placeables.getOrNull(1)?.let { it.place(x(it.width), first.height + Chrome.gap.roundToPx()) }
            }
        }
    }
}

/** The side controls' sizes, the same on both sides: segment height and rocker length. */
class SideMetrics(val segment: Dp, val rocker: Dp) {
    companion object {
        /**
         * Segments 30 dp where [height] allows (smaller on a landscape phone);
         * the rocker as long as the selector (4 segments) and what is under it
         * ([under] segments: 2 for A / M, or the lenses).
         */
        fun fitting(height: Dp, under: Int): SideMetrics {
            val k = maxOf(2, under)
            // Paddings (3 + 3 per switch), 2 between segments, the gap between the switches.
            val fixed = 12.dp + 2.dp * (4 - 1 + k - 1) + Chrome.gap
            val segment = ((height - fixed) / (4 + k)).coerceIn(20.dp, 30.dp)
            return SideMetrics(segment, segment * (4 + k) + fixed)
        }
    }
}

/** The zoom factor against the wide lens, as the lens buttons count: "2.3×", "12×". */
private fun zoomFactorText(model: AppModel): String {
    val factor = lensFactor(model) * model.shading.values.optDouble("zoom", 1.0)
    return if (factor >= 10) "%.0f×".format(factor) else "%.1f×".format(factor)
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
private fun ModeSwitch(model: AppModel, group: String, mode: String, segment: Dp) {
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
                Modifier.fillMaxWidth().height(segment)
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

/**
 * ZOOM / EXP / WB / FOC beside the upper half of the rocker, the same look
 * as the mode switch: the chosen group filled in its colour; a tap puts that group
 * on this side (docs/UI_DESIGN.md, "Side control").
 */
@Composable
private fun GroupSelector(groups: List<String>, selected: String, segment: Dp, onSelect: (String) -> Unit) {
    Segments(SIDE_GROUPS.map { Segment(shortName(it), on = it == selected, enabled = it in groups, description = title(it)) },
             segment, fill = Chrome.groupColor(selected), onText = Color.White, onTap = { onSelect(SIDE_GROUPS[it]) })
}

/** The lens buttons under the zoom selector, longest on top, the active one filled. */
@Composable
private fun LensSwitch(model: AppModel, segment: Dp) {
    val lenses = model.camera.lenses
    if (lenses.size < 2) return
    val current = model.shading.values.optString("lens")
    val ordered = lenses.sortedByDescending { it.focalLength }
    Segments(ordered.map { Segment(lensLabel(model, it), on = it.key == current, description = it.label) }, segment,
             onTap = { model.selectLens(ordered[it].key) })
}

/** One segment of a vertical switch. */
private class Segment(val label: String, val on: Boolean, val enabled: Boolean = true, val description: String = label)

/**
 * A vertical switch of [segments], 46 dp wide, the look of the mode switch:
 * the one that is on filled ([fill], the selector's in its group colour); a
 * tap on another calls [onTap].
 */
@Composable
private fun Segments(segments: List<Segment>, segment: Dp, fill: Color = Color.White, onText: Color = Color.Black,
                     onTap: (Int) -> Unit) {
    Column(
        Modifier.width(Chrome.control).background(Color.Black.copy(alpha = 0.35f), RoundedCornerShape(Chrome.control / 2))
            .border(1.dp, Color.White.copy(alpha = 0.25f), RoundedCornerShape(Chrome.control / 2)).padding(3.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        segments.forEachIndexed { index, item ->
            Box(
                Modifier.fillMaxWidth().height(segment).background(if (item.on) fill else Color.Transparent, CircleShape)
                    .clickable(enabled = item.enabled && !item.on) { onTap(index) }
                    .semantics {
                        contentDescription = item.description
                        selected = item.on
                    },
                contentAlignment = Alignment.Center,
            ) {
                Text(item.label, color = if (item.on) onText else Color.White.copy(alpha = if (item.enabled) 0.75f else 0.3f),
                     fontSize = if (segment < 26.dp) 10.sp else 11.sp, fontWeight = FontWeight.Bold, maxLines = 1, softWrap = false)
            }
        }
    }
}

/** The groups a side control can run, in the selector's order. */
val SIDE_GROUPS = listOf("zoom", "exposure", "white_balance", "focus")

private fun shortName(group: String) = when (group) {
    "zoom" -> "ZOOM"
    "exposure" -> "EXP"
    "white_balance" -> "WB"
    else -> "FOC"
}

private fun title(group: String) = when (group) {
    "zoom" -> "Zoom"
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
/** Zoom doublings per second at full deflection, and per tap on an end. */
private const val MAX_ZOOM_RATE = 2.0
private const val ZOOM_TAP = 0.25
