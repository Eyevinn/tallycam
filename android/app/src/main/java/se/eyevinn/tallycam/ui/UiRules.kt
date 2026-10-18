package se.eyevinn.tallycam.ui

import org.json.JSONObject
import se.eyevinn.tallycam.control.Shading
import java.util.Locale
import kotlin.math.abs
import kotlin.math.log2
import kotlin.math.pow
import kotlin.math.roundToLong

/*
 * The shooting screen's rules as plain functions, free of Compose, so the
 * shared vectors (core/tests/vectors/ui_values.json) check them on both
 * apps: the values row, the rockers' steps and neutral values, and which
 * group each side control runs (docs/UI_DESIGN.md).
 */

/** How a value is drawn: white (auto), yellow (held), or a warning colour. */
enum class Tone { AUTO, HELD, ORANGE, RED }

/** One value in the values row. */
data class ValueItem(
    val text: String,
    val flag: String? = null,
    val tone: Tone = Tone.AUTO,
    /** The group a tap on it chooses; null opens Settings. */
    val group: String? = null,
    /** The controls it shows: it flashes when the hub changes one. */
    val ids: List<String> = emptyList(),
)

/** Numbers as both apps print them, whatever the phone's language ("0.42", never "0,42"). */
internal fun fmt(pattern: String, vararg args: Any): String = String.format(Locale.ROOT, pattern, *args)

/**
 * The values row (docs/UI_DESIGN.md, "Values"): format, ISO (with CLIP or
 * DARK from the image statistics' [warning]), shutter, EV (auto and not
 * 0), white balance (K with the tint when not 0, or AWB where the camera
 * reports no temperature) and focus (AF, or F 0.42 when held). The modes are
 * auto and manual; "locked", from older hubs, reads as manual.
 */
fun valueItems(values: JSONObject, format: String, warning: String?): List<ValueItem> {
    val exposureHeld = values.has("exposure_mode") && values.optString("exposure_mode") != "auto"
    val held = if (exposureHeld) Tone.HELD else Tone.AUTO
    val list = mutableListOf<ValueItem>()
    if (format.isNotEmpty()) list += ValueItem(format, ids = listOf("resolution", "frame_rate"))
    if (values.has("iso")) {
        val iso = fmt("ISO %.0f", values.optDouble("iso"))
        val ids = listOf("iso", "exposure_mode")
        list += when (warning) {
            "too_dark" -> ValueItem(iso, "DARK", Tone.ORANGE, "exposure", ids)
            "clipping" -> ValueItem(iso, "CLIP", Tone.RED, "exposure", ids)
            else -> ValueItem(iso, tone = held, group = "exposure", ids = ids)
        }
    }
    values.optDouble("shutter", 0.0).takeIf { it > 0 }?.let {
        list += ValueItem("1/${Math.round(it)}", tone = held, group = "exposure", ids = listOf("shutter"))
    }
    val bias = values.optDouble("ev_bias", 0.0)
    if (!exposureHeld && abs(bias) >= 0.05) list += ValueItem(fmt("%+.1f EV", bias), group = "exposure", ids = listOf("ev_bias"))
    if (values.has("wb_mode")) {
        val mode = values.optString("wb_mode")
        val tone = if (mode == "auto") Tone.AUTO else Tone.HELD
        val ids = listOf("wb_mode", "wb_temperature", "wb_tint")
        list += if (!values.has("wb_temperature")) {
            ValueItem("AWB", tone = tone, group = "white_balance", ids = ids)
        } else {
            val tint = values.optDouble("wb_tint", 0.0).takeIf { abs(it) >= 0.5 }?.let { fmt(" %+.0f", it) } ?: ""
            ValueItem(fmt("%.0f K", values.optDouble("wb_temperature")) + tint, tone = tone, group = "white_balance", ids = ids)
        }
    }
    if (values.has("focus_mode")) {
        val ids = listOf("focus_mode", "focus_position")
        list += when (values.optString("focus_mode")) {
            "auto" -> ValueItem("AF", group = "focus", ids = ids)
            else -> ValueItem(if (values.has("focus_position")) fmt("F %.2f", values.optDouble("focus_position")) else "MF",
                              tone = Tone.HELD, group = "focus", ids = ids)
        }
    }
    return list
}

/**
 * The value under a side control's rocker: what it moves, as it stands
 * ("+0.3 EV", "ISO 400", "5200 K", "Tint +4", "AF", "F 0.42"). Without a
 * temperature (Android's auto white balance) "AWB". Zoom's is the factor
 * (zoomFactorText).
 */
fun adjustValueText(group: String, values: JSONObject, tint: Boolean): String {
    val auto = values.optString(modeKey(group), "auto") == "auto"
    return when (group) {
        "exposure" -> if (auto) fmt("%+.1f EV", values.optDouble("ev_bias", 0.0))
                      else if (values.has("iso")) fmt("ISO %.0f", values.optDouble("iso")) else "ISO"
        "white_balance" -> when {
            tint -> if (values.has("wb_tint")) fmt("Tint %+.0f", values.optDouble("wb_tint")) else "Tint"
            values.has("wb_temperature") -> fmt("%.0f K", values.optDouble("wb_temperature"))
            else -> "AWB"
        }
        else -> if (auto) "AF" else if (values.has("focus_position")) fmt("F %.2f", values.optDouble("focus_position")) else "F"
    }
}

/** The zoom factor against the wide lens, as the lens buttons count it: "2.3×" below 10, "12×" from 10. */
fun zoomFactorText(factor: Double): String = if (factor >= 10) fmt("%.0f×", factor) else fmt("%.1f×", factor)

/**
 * The time-of-day timecode in the top bar (docs/UI_DESIGN.md, zone 1):
 * "14:51:46:12", hours, minutes, seconds in local time ([utcOffsetSeconds]
 * east of UTC) and the frame within the second at [fps], from the
 * disciplined clock's UTC; "14:51:46:--" while it is not synchronised.
 */
fun timecode(utcNs: Long, utcOffsetSeconds: Long, fps: Int, synced: Boolean): String {
    val local = utcNs + utcOffsetSeconds * 1_000_000_000L
    val second = Math.floorMod(Math.floorDiv(local, 1_000_000_000L), 86_400L)
    val hms = fmt("%02d:%02d:%02d", second / 3600, second / 60 % 60, second % 60)
    if (!synced || fps <= 0) return "$hms:--"
    val frame = Math.floorMod(local, 1_000_000_000L) * fps / 1_000_000_000L
    return hms + fmt(":%02d", frame)
}

/**
 * On a narrow screen the values row leaves these out, in this order: the
 * focus value, white balance, the compensation, the shutter; it never wraps.
 */
val VALUE_DROP_ORDER = listOf("focus", "white_balance", "ev_bias", "shutter")

/**
 * The landscape top bar's order (docs/UI_DESIGN.md): focus, white balance,
 * the chip's ON AIR / PREVIEW, the compensation, the shutter; only then is
 * the name cut with an ellipsis.
 */
val TOP_BAR_DROP_ORDER = listOf("focus", "white_balance", "tally_label", "ev_bias", "shutter")

/** Which of [VALUE_DROP_ORDER] a value is, if any. */
fun ValueItem.dropKey(): String? = when {
    group == "focus" -> "focus"
    group == "white_balance" -> "white_balance"
    "ev_bias" in ids -> "ev_bias"
    "shutter" in ids -> "shutter"
    else -> null
}

/** The mode control of a shading group. */
fun modeKey(group: String): String = Shading.MODE_CONTROLS.first { it.first == group }.second

/**
 * What a group's rocker moves: full speed in the control's scale per
 * second, a tap's step in the same scale (log2 for ISO), and the mode to
 * switch to first.
 */
class Ramped(val id: String, val label: String, val speed: Double, val tap: Double, val log: Boolean = false,
             val mode: String? = null) {
    fun scale(x: Double) = if (log) log2(x) else x
    fun unscale(x: Double) = if (log) 2.0.pow(x) else x
}

/** The control a group's rocker moves now (docs/UI_DESIGN.md, the side control's table); null for zoom or a missing one. */
fun ramped(values: JSONObject, descriptors: Map<String, JSONObject>, group: String, tint: Boolean): Ramped? {
    if (group == "zoom") return null
    val mode = values.optString(modeKey(group), "auto")
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
 * [raw] clamped to the control's range and snapped to its step, as the set
 * to send (the mode first where it has to change); null when the control
 * has no range.
 */
fun rampSet(r: Ramped, group: String, descriptors: Map<String, JSONObject>, raw: Double): JSONObject? {
    val d = descriptors[r.id] ?: return null
    val low = d.optDouble("min", Double.NaN)
    val high = d.optDouble("max", Double.NaN)
    if (low.isNaN() || high.isNaN() || high <= low) return null
    var value = raw
    val step = d.optDouble("step", 0.0)
    if (step > 0) value = (value / step).roundToLong() * step
    value = value.coerceIn(low, high)
    // Clean decimals: 0.30000000000000004 is 0.3.
    value = Math.round(value * 1e6) / 1e6
    val set = JSONObject().put(r.id, value)
    r.mode?.let { set.put(modeKey(group), it) }
    return set
}

/** Zoom doublings per second at full deflection, and per tap on an end. */
const val MAX_ZOOM_RATE = 2.0
const val ZOOM_TAP = 0.25

/**
 * A tap on an end of a group's rocker: one step up (+1) or down (-1) from
 * [base] (a quick tap's last value) or the camera's value, as the set to
 * send. Without a value (Android's auto white balance reports no
 * temperature) the step starts from the middle of the range, as the drag.
 */
fun tapStep(group: String, values: JSONObject, descriptors: Map<String, JSONObject>, tint: Boolean, direction: Int,
            base: Double? = null): JSONObject? {
    if (group == "zoom") {
        val d = descriptors["zoom"] ?: return null
        val current = base ?: values.optDouble("zoom", 1.0)
        val zoom = (Math.round(current * 2.0.pow(ZOOM_TAP * direction) * 100) / 100.0)
            .coerceIn(d.optDouble("min", 1.0), d.optDouble("max", 1.0))
        return JSONObject().put("zoom", zoom)
    }
    val r = ramped(values, descriptors, group, tint) ?: return null
    val d = descriptors[r.id] ?: return null
    val current = base ?: values.optDouble(r.id, Double.NaN).takeIf { !it.isNaN() }
        ?: ((d.optDouble("min") + d.optDouble("max")) / 2).takeIf { !it.isNaN() } ?: return null
    if (r.log && current <= 0) return null
    return rampSet(r, group, descriptors, r.unscale(r.scale(current) + direction * r.tap))
}

/** What a double tap on the knob does: a set, or a command (white balance once). */
sealed class Neutral {
    data class Set(val values: JSONObject) : Neutral()
    data class Command(val name: String) : Neutral()
}

/**
 * A double tap on the knob: the neutral value (docs/UI_DESIGN.md). EV 0 in
 * auto exposure, else back to auto; white balance once (K) or tint 0 (in
 * manual white balance only); focus back to auto; zoom the lens's own 1×.
 */
fun neutral(group: String, values: JSONObject, tint: Boolean): Neutral? {
    if (group == "zoom") return Neutral.Set(JSONObject().put("zoom", 1.0))
    val mode = values.optString(modeKey(group), "auto")
    return when {
        // Tint 0 only in manual white balance: the camera refuses a tint outside it.
        group == "white_balance" && tint -> if (mode == "manual") Neutral.Set(JSONObject().put("wb_tint", 0.0)) else null
        group == "white_balance" -> Neutral.Command("wb_one_shot")
        group == "exposure" && mode == "auto" -> Neutral.Set(JSONObject().put("ev_bias", 0.0))
        mode != "auto" -> Neutral.Set(JSONObject().put(modeKey(group), "auto"))
        else -> null
    }
}

/** The groups a side control can run, in the selector's order. */
val SIDE_GROUPS = listOf("zoom", "exposure", "white_balance", "focus")

/**
 * What the two side controls run (docs/UI_DESIGN.md, "Side control"): zoom
 * on the left and exposure on the right until changed; never the same
 * group on both sides. A side may be hidden ("–"), folded into a tab at
 * the edge; it keeps its group for when it comes back.
 */
data class Sides(val left: String?, val right: String?, val leftHidden: Boolean = false, val rightHidden: Boolean = false) {
    /** The choice where the camera has the group, else the first it has that the other side does not run. */
    fun resolved(available: List<String>): Sides {
        val r = right?.takeIf { it in available } ?: available.firstOrNull { it != left }
        val l = left?.takeIf { it in available && it != r } ?: available.firstOrNull { it != r }
        return copy(left = l, right = r)
    }

    /**
     * A selector tap on one side: the chosen group again does nothing; the
     * other side's group swaps the two (each side keeps whether it is
     * hidden).
     */
    fun choose(leftSide: Boolean, group: String): Sides = when {
        group == (if (leftSide) left else right) -> this
        leftSide -> copy(left = group, right = if (group == right) left else right)
        else -> copy(left = if (group == left) right else left, right = group)
    }

    /** A tap on a shown side's tab: it folds away, keeping its group. */
    fun hide(leftSide: Boolean): Sides = if (leftSide) copy(leftHidden = true) else copy(rightHidden = true)

    /** A tap on a hidden side's tab: it comes back with its group. */
    fun show(leftSide: Boolean): Sides = if (leftSide) copy(leftHidden = false) else copy(rightHidden = false)

    /**
     * A tap in the values row: nothing while a shown side runs the group; a
     * hidden side running it comes back; else the group goes to the right,
     * which shows.
     */
    fun fromValues(group: String): Sides = when {
        group == left && !leftHidden || group == right && !rightHidden -> this
        group == left -> copy(leftHidden = false)
        group == right -> copy(rightHidden = false)
        else -> copy(right = group, rightHidden = false)
    }

    companion object {
        val DEFAULT = Sides("zoom", "exposure")
    }
}
