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
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.wrapContentWidth
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
import androidx.compose.ui.unit.Constraints
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
 * is under it (on every screen), the same on both sides. On its inner side: the
 * selector beside its upper half (a tap puts that group on this side; the
 * other side's group swaps), and beside its lower half the A / M mode
 * switch (M is manual, the values held as numbers; a tap on the other cell
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
 */
@Composable
fun SideControl(model: AppModel, group: String, groups: List<String>, leading: Boolean, readoutAbove: Boolean = false,
                locked: Boolean, onSelect: (String) -> Unit, modifier: Modifier = Modifier) {
    val zoom = group == "zoom"
    val descriptors = model.shading.descriptors
    val hasTint = group == "white_balance" && "wb_tint" in descriptors
    // K each time colour is chosen.
    var tint by remember(group) { mutableStateOf(false) }
    var rate by remember(group) { mutableDoubleStateOf(0.0) }
    val ramping by rememberUpdatedState(ramped(model.shading.values, descriptors, group, tint))
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

    /** Sends a set from the rocker, with its tick (firmer at the end of the range); returns the value sent. */
    fun sendSet(set: JSONObject): Double? {
        val mode = if (zoom) null else modeKey(group)
        val id = set.keys().asSequence().firstOrNull { it != mode } ?: return null
        val value = set.optDouble(id)
        val d = model.shading.descriptors[id]
        model.localSet(set, quiet = true)
        haptics.step(atEnd = d != null && (value <= d.optDouble("min") || value >= d.optDouble("max")))
        touched = android.os.SystemClock.uptimeMillis()
        return value
    }
    /** [raw] clamped, snapped and sent unless it is the value [last] sent. */
    fun send(r: Ramped, raw: Double, last: Double? = null): Double? {
        val set = rampSet(r, group, model.shading.descriptors, raw) ?: return null
        val value = set.optDouble(r.id)
        return if (value == last) value else sendSet(set)
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
    /** A tap on an end: one step up (+1) or down (-1), from the last tap's value within a second (as iOS). */
    fun step(direction: Int) {
        val now = android.os.SystemClock.uptimeMillis()
        val base = lastTap?.takeIf { now - it.first < 1000 }?.second
        val set = tapStep(group, model.shading.values, model.shading.descriptors, tint, direction, base) ?: return
        sendSet(set)?.let { lastTap = now to it }
    }
    /** A double tap on the knob: the neutral value (docs/UI_DESIGN.md). */
    fun reset() {
        lastTap = null
        when (val action = neutral(group, model.shading.values, tint)) {
            is Neutral.Set -> sendSet(action.values)
            is Neutral.Command -> {
                if (action.name == "wb_one_shot") model.localWhiteBalanceOnce()
                haptics.step()
                touched = android.os.SystemClock.uptimeMillis()
            }
            null -> Unit
        }
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
    val text = if (zoom) zoomFactorText(model) else adjustValueText(group, model.shading.values, tint)
    val adjusting = rate != 0.0 || recent

    val rocker = @Composable {
        if (zoom) {
            Rocker("T", "W", "Zoom", SIDE_HEIGHT, Chrome.groupColor(group), onStep = ::step, onReset = ::reset) {
                rate = it
                model.rampZoom(it * MAX_ZOOM_RATE)
            }
        } else {
            // Read-only where the camera cannot set it (white balance without
            // manual post-processing, ISO without a manual sensor): dimmed.
            val settable = ramping?.let { r -> descriptors[r.id]?.optBoolean("readonly") != true } ?: false
            Rocker(highLabel(group, tint), lowLabel(group, tint), "${title(group)} ${ramping?.label ?: ""}", SIDE_HEIGHT,
                   Chrome.groupColor(group), enabled = settable, onStep = ::step, onReset = ::reset) { rate = it }
        }
    }
    val value = @Composable {
        // Grows and turns bright while adjusting (as iOS: x1.25, a moment after a tap too).
        val grow by animateFloatAsState(if (adjusting) 1.25f else 1f, tween(150), label = "value")
        val tone = if (auto) Color.White else Chrome.yellow
        Box(Modifier.height(Chrome.valueHeight).graphicsLayer { scaleX = grow; scaleY = grow }
                .background(if (adjusting) Color.Black.copy(alpha = 0.5f) else Chrome.surface, RoundedCornerShape(Chrome.cellRadius))
                .border(1.dp, Chrome.border, RoundedCornerShape(Chrome.cellRadius))
                .padding(horizontal = 8.dp), contentAlignment = Alignment.Center) {
            Text(text, color = if (adjusting) tone else tone.copy(alpha = 0.8f), fontSize = 13.sp, style = Chrome.tabular,
                 fontWeight = FontWeight.SemiBold, maxLines = 1, softWrap = false)
        }
    }
    val block = @Composable {
        // The 2 x 2 selector over the mode row (or zoom's lenses), beside the rocker.
        Column(Modifier.width(Chrome.blockWidth).height(SIDE_HEIGHT), verticalArrangement = Arrangement.SpaceBetween) {
            GroupSelector(groups, group, onSelect)
            when {
                zoom -> LensRow(model)
                modeKey(group) in descriptors -> ModeRow(model, group, mode)
                else -> Box(Modifier.height(Chrome.cell))
            }
        }
    }
    // The rocker at the screen edge, exactly as tall as the selector and the
    // mode row; hidden while the screen is locked, its place kept.
    val edge = @Composable { if (locked) Box(Modifier.width(Chrome.rockerWidth).height(SIDE_HEIGHT)) else rocker() }
    // The value and K / Tint beside the block on its inner side, level with
    // the mode row (docs/UI_DESIGN.md, "Sizes"); the same size whatever the group.
    if (!readoutAbove) {
        Row(modifier.height(SIDE_HEIGHT), horizontalArrangement = Arrangement.spacedBy(Chrome.gap)) {
            // The value centred on the mode row, K / Tint above it; the slot is
            // there for every group.
            val side = @Composable {
                Column(Modifier.width(Chrome.readoutWidth).fillMaxHeight()
                           .padding(bottom = (Chrome.cell - Chrome.valueHeight) / 2),
                       horizontalAlignment = Alignment.CenterHorizontally,
                       verticalArrangement = Arrangement.spacedBy(Chrome.gap, Alignment.Bottom)) {
                    if (hasTint) TintSwitch(tint) { tint = it; rate = 0.0 }
                    Box(Modifier.wrapContentWidth(unbounded = true)) { value() }
                }
            }
            if (leading) { edge(); block(); side() } else { side(); block(); edge() }
        }
    } else {
        // Too narrow for the value beside the block (a phone in portrait):
        // the value and K / Tint above it, the row kept for every group.
        Column(modifier.width(Chrome.blockWidth + Chrome.gap + Chrome.rockerWidth), verticalArrangement = Arrangement.spacedBy(Chrome.gap)) {
            Row(Modifier.fillMaxWidth().height(Chrome.valueHeight),
                horizontalArrangement = Arrangement.spacedBy(Chrome.gap, if (leading) Alignment.Start else Alignment.End),
                verticalAlignment = Alignment.CenterVertically) {
                val v = @Composable { Box(Modifier.wrapContentWidth(unbounded = true)) { value() } }
                val t = @Composable { if (hasTint) TintSwitch(tint) { tint = it; rate = 0.0 } }
                if (leading) { v(); t() } else { t(); v() }
            }
            Row(Modifier.height(SIDE_HEIGHT), horizontalArrangement = Arrangement.spacedBy(Chrome.gap)) {
                if (leading) { edge(); block() } else { block(); edge() }
            }
        }
    }
}

/** A side control's height: the 2 x 2 selector and the mode row; the rocker is as tall, on every screen. */
val SIDE_HEIGHT = Chrome.cell * 3 + Chrome.cellGap + Chrome.gap

/** The zoom factor against the wide lens, as the lens buttons count: "2.3×", "12×". */
internal fun zoomFactorText(model: AppModel): String {
    val factor = lensFactor(model) * model.shading.values.optDouble("zoom", 1.0)
    return zoomFactorText(factor)
}

/**
 * The group's mode as two cells side by side, A and M: both always shown,
 * the one in use filled (white for auto, yellow for manual), a tap on the
 * other switches to it, keeping the values as numbers. "locked" from an
 * older hub reads as M. For white balance a third cell, ◎, sets it once
 * from the picture and holds it (as does a long press on A or M).
 */
@Composable
private fun ModeRow(model: AppModel, group: String, mode: String) {
    val auto = mode == "auto"
    val held = "manual"
    val wbOnce: (() -> Unit)? = if (group == "white_balance") ({ model.localWhiteBalanceOnce() }) else null
    val cells = mutableListOf(
        Cell("A", on = auto, fill = Color.White, description = "${title(group)} auto", longPress = wbOnce),
        Cell("M", on = !auto, fill = Chrome.yellow, description = "${title(group)} $held", longPress = wbOnce),
    )
    // White balance: ◎ sets it once from the picture and holds it (M).
    if (wbOnce != null) cells += Cell("◎", on = false, description = "White balance once")
    Cells(listOf(cells), fontSize = 17.sp) { _, column ->
        when (column) {
            0 -> model.localSet(JSONObject().put(modeKey(group), "auto"))
            1 -> model.localSet(JSONObject().put(modeKey(group), held))
            else -> model.localWhiteBalanceOnce()
        }
    }
}

/** K or Tint for colour's rocker. */
@Composable
private fun TintSwitch(tint: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.background(Chrome.surface, RoundedCornerShape(Chrome.cellRadius))
            .border(1.dp, Chrome.border, RoundedCornerShape(Chrome.cellRadius)).padding(2.dp)) {
        for ((title, on) in listOf("K" to !tint, "Tint" to tint)) {
            Text(title, color = if (on) Color.Black else Color.White.copy(alpha = 0.9f), fontSize = 12.sp, fontWeight = FontWeight.Bold,
                 style = if (on) androidx.compose.ui.text.TextStyle.Default else Chrome.shadowed,
                 modifier = Modifier.height(Chrome.tintHeight).background(if (on) Color.White else Color.Transparent, RoundedCornerShape(6.dp))
                     .clickable { if (!on) onChange(title == "Tint") }
                     .semantics { contentDescription = if (title == "K") "Rocker moves the temperature" else "Rocker moves the tint" }
                     .padding(horizontal = 8.dp, vertical = 5.dp))
        }
    }
}

/**
 * ZOOM EXP over WB FOC beside the upper half of the rocker: the chosen
 * group filled in its colour with white text; a tap puts that group on
 * this side, the chosen one again hides it (docs/UI_DESIGN.md, "Side control").
 */
@Composable
private fun GroupSelector(groups: List<String>, selected: String, onSelect: (String) -> Unit) {
    val cells = SIDE_GROUPS.map {
        Cell(shortName(it), on = it == selected, enabled = it in groups, fill = Chrome.groupColor(it), onText = Color.White,
             description = title(it))
    }
    // The chosen cell again hides this side.
    Cells(cells.chunked(2), tapOn = true) { row, column -> onSelect(SIDE_GROUPS[row * 2 + column]) }
}

/** The lens buttons in the mode row on zoom, shortest left, the active one filled. */
@Composable
private fun LensRow(model: AppModel) {
    val lenses = model.camera.lenses
    if (lenses.size < 2) {
        Box(Modifier.height(Chrome.cell))
        return
    }
    val current = model.shading.values.optString("lens")
    val ordered = lenses.sortedBy { it.focalLength }
    Cells(listOf(ordered.map { Cell(lensLabel(model, it), on = it.key == current, description = it.label) })) { _, column ->
        model.selectLens(ordered[column].key)
    }
}

/** One cell of the selector, the mode row or the lens row. */
private class Cell(val label: String, val on: Boolean, val enabled: Boolean = true, val fill: Color = Color.White,
                   val onText: Color = Color.Black, val description: String = label, val longPress: (() -> Unit)? = null)

/**
 * A grid of [rows] of cells, at least 48 dp square-ish, an 8 dp corner
 * radius (docs/UI_DESIGN.md, "Shape"): off cells dark with a thin outline,
 * the one that is on filled. A tap on another calls [onTap] (row, column).
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun Cells(rows: List<List<Cell>>, fontSize: androidx.compose.ui.unit.TextUnit = 12.sp, tapOn: Boolean = false,
                  onTap: (Int, Int) -> Unit) {
    val shape = RoundedCornerShape(Chrome.cellRadius)
    Column(Modifier.width(Chrome.blockWidth), verticalArrangement = Arrangement.spacedBy(Chrome.cellGap)) {
        rows.forEachIndexed { r, row ->
            Row(horizontalArrangement = Arrangement.spacedBy(Chrome.cellGap)) {
                row.forEachIndexed { c, cell ->
                    Box(
                        Modifier.weight(1f).height(Chrome.cell)
                            .background(if (cell.on) Chrome.chosen(cell.fill) else Chrome.surface, shape)
                            .border(1.dp, if (cell.on) Color.Transparent else Chrome.border, shape)
                            .combinedClickable(enabled = cell.enabled, onClick = { if (!cell.on || tapOn) onTap(r, c) },
                                               onLongClick = cell.longPress,
                                               onLongClickLabel = cell.longPress?.let { "White balance once" })
                            .semantics {
                                contentDescription = cell.description
                                selected = cell.on
                            },
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(cell.label, color = if (cell.on) cell.onText else Color.White.copy(alpha = if (cell.enabled) 0.8f else 0.3f),
                             fontSize = fontSize, fontWeight = FontWeight.Bold, maxLines = 1, softWrap = false,
                             style = if (cell.on && cell.onText == Color.Black) androidx.compose.ui.text.TextStyle.Default else Chrome.shadowed)
                    }
                }
            }
        }
    }
}

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


/** How often a held rocker changes its value (about 10 Hz). */
private const val RAMP_MS = 100L

