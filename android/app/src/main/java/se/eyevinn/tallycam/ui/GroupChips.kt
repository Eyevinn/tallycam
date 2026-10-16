package se.eyevinn.tallycam.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONArray
import org.json.JSONObject
import se.eyevinn.tallycam.AppModel
import se.eyevinn.tallycam.control.isRemote
import se.eyevinn.tallycam.control.shortName
import se.eyevinn.tallycam.control.title
import se.eyevinn.tallycam.core.GroupView
import kotlin.math.exp
import kotlin.math.ln
import kotlin.math.roundToInt

/**
 * Who owns exposure, colour and focus: one chip per group (`EXP AUTO`,
 * `WB LOCKED`, `FOC REMOTE`), as on iOS. Tapping a chip opens the group's
 * controls, read-only while the group is remote; a long press on a
 * disconnected remote group offers to take it local (docs/SHADING.md, D2).
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun GroupChips(model: AppModel, modifier: Modifier = Modifier) {
    var open by remember { mutableStateOf<String?>(null) }
    var confirmTake by remember { mutableStateOf<GroupView?>(null) }
    Row(modifier, horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
        for (view in model.shading.groups) {
            GroupChip(view, Modifier.combinedClickable(
                onClick = { open = view.group },
                onLongClick = { if (view.indicator == "remote_disconnected") confirmTake = view },
            ))
        }
        ExposureWarning(model.shading.exposureWarning)
    }
    open?.let { GroupSheet(model, it) { open = null } }
    confirmTake?.let { view -> TakeLocalDialog(model, view) { confirmTake = null } }
}

@Composable
private fun GroupChip(view: GroupView, modifier: Modifier) {
    val (state, color) = when (view.indicator) {
        "locked" -> "LOCKED" to Color(0xFFFFD60A)
        "remote" -> "REMOTE" to Color(0xFF64D2FF)
        "remote_disconnected" -> "REMOTE · OFFLINE" to Color(0xFFFF9F0A)
        else -> "AUTO" to Color.White
    }
    Row(
        modifier.background(Color.Black.copy(alpha = 0.55f), CircleShape)
            .border(1.dp, if (view.indicator == "auto") Color.Transparent else color.copy(alpha = 0.8f), CircleShape)
            .padding(horizontal = 8.dp, vertical = 4.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Text(view.shortName, color = Color.White.copy(alpha = 0.7f), fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
        Text(state, color = color, fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
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

@Composable
private fun TakeLocalDialog(model: AppModel, view: GroupView, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Take ${view.title.lowercase()} back from the hub?") },
        text = { Text("The values stay. The hub cannot change them until it takes over again.") },
        confirmButton = {
            TextButton(onClick = { model.takeLocal(view.group); onDismiss() }) {
                Text("Take local control", color = Color(0xFFFF453A))
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/**
 * The controls of one group: the mode (Auto, Locked, Manual) and the manual
 * values. Read-only while the group is remote; while the hub is away the
 * operator may take the group back (D2).
 */
@Composable
private fun GroupSheet(model: AppModel, group: String, onDismiss: () -> Unit) {
    val view = model.shading.view(group) ?: return onDismiss()
    // Re-read with every change the operator makes here.
    var serial by remember { mutableStateOf(0) }
    val descriptors = remember(serial, view) { model.cameraDescriptors().associateBy { it.optString("id") } }
    val values = remember(serial, view) { model.cameraValues() }
    var confirmTake by remember { mutableStateOf(false) }
    val remote = view.isRemote
    fun set(id: String, value: Any) {
        model.localSet(group, JSONObject().put(id, value))
        serial++
    }
    Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.4f)).combinedClickableNoRipple(onDismiss)) {
        Column(
            Modifier.align(Alignment.CenterStart).padding(start = 24.dp).width(360.dp)
                .background(Color(0xFF1C1C1E), RoundedCornerShape(16.dp))
                .combinedClickableNoRipple {}
                .verticalScroll(rememberScrollState())
                .padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(view.title, color = Color.White, fontSize = 18.sp, fontWeight = FontWeight.Bold,
                     modifier = Modifier.weight(1f))
                TextButton(onClick = onDismiss) { Text("Done") }
            }
            if (remote) {
                val holder = view.holderLabel ?: "the hub"
                Text(if (view.indicator == "remote") "Controlled by $holder"
                     else "Controlled by $holder, which is not connected",
                     color = Color(0xFF64D2FF), fontSize = 14.sp)
                if (view.indicator == "remote_disconnected") {
                    TextButton(onClick = { confirmTake = true }) { Text("Take local control", color = Color(0xFFFF453A)) }
                }
            }
            val modeKey = when (group) {
                "exposure" -> "exposure_mode"
                "white_balance" -> "wb_mode"
                else -> "focus_mode"
            }
            ModePicker(descriptors[modeKey], values.optString(modeKey, "auto"), enabled = !remote) { mode ->
                if (mode == "auto") model.localAuto(group) else model.localSet(group, JSONObject().put(modeKey, mode))
                serial++
            }
            val slider = @Composable { id: String, label: String, format: (Double) -> String ->
                descriptors[id]?.let { RangeControl(it, label, values, format, enabled = !remote) { v -> set(id, v) } }
            }
            when (group) {
                "exposure" -> {
                    slider("iso", "ISO") { "%.0f".format(it) }
                    slider("ev_bias", "Compensation") { "%+.1f EV".format(it) }
                    if (values.has("shutter")) Row {
                        Text("Shutter", color = Color.White, fontSize = 14.sp, modifier = Modifier.weight(1f))
                        Text("1/${values.optInt("shutter")}", color = Color.White.copy(alpha = 0.6f), fontSize = 14.sp)
                    }
                }
                "white_balance" -> {
                    slider("wb_temperature", "Temperature") { "%.0f K".format(it) }
                    TextButton(enabled = !remote, onClick = {
                        model.localSet(group, JSONObject())
                        model.localWhiteBalanceOnce()
                        serial++
                    }) { Text("White balance once") }
                }
                else -> slider("focus_position", "Position") { "%.2f".format(it) }
            }
        }
    }
    if (confirmTake) TakeLocalDialog(model, view) { confirmTake = false }
}

@Composable
private fun ModePicker(descriptor: JSONObject?, current: String, enabled: Boolean, onPick: (String) -> Unit) {
    val options = descriptor?.optJSONArray("options") ?: JSONArray()
    Row(
        Modifier.fillMaxWidth().background(Color.White.copy(alpha = 0.08f), RoundedCornerShape(8.dp)).padding(2.dp)
            .alpha(if (enabled) 1f else 0.5f),
    ) {
        for (i in 0 until options.length()) {
            val option = options.optJSONObject(i) ?: continue
            val value = option.optString("value")
            val selected = value == current
            Box(
                Modifier.weight(1f)
                    .background(if (selected) Color.White.copy(alpha = 0.22f) else Color.Transparent, RoundedCornerShape(6.dp))
                    .combinedClickableNoRipple { if (enabled && !selected) onPick(value) }
                    .padding(vertical = 7.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(option.optString("label"), color = Color.White, fontSize = 13.sp,
                     fontWeight = if (selected) FontWeight.SemiBold else FontWeight.Normal)
            }
        }
    }
}

/** A slider for a range control, writable only where its descriptor's `enabled_when` holds. */
@Composable
private fun RangeControl(descriptor: JSONObject, label: String, values: JSONObject, format: (Double) -> String,
                         enabled: Boolean, onSet: (Double) -> Unit) {
    val min = descriptor.optDouble("min", Double.NaN)
    val max = descriptor.optDouble("max", Double.NaN)
    val value = values.optDouble(descriptor.optString("id"), Double.NaN)
    if (min.isNaN() || max.isNaN() || value.isNaN() || max <= min) return
    val writable = enabled && !descriptor.optBoolean("readonly") && (descriptor.optJSONObject("enabled_when")?.let { cond ->
        cond.keys().asSequence().all { key ->
            when (val wanted = cond.get(key)) {
                is JSONArray -> (0 until wanted.length()).any { wanted.get(it) == values.opt(key) }
                else -> wanted == values.opt(key)
            }
        }
    } ?: true)
    val log = descriptor.optString("scale") == "log" && min > 0
    val step = descriptor.optDouble("step", 0.0)
    var position by remember(value) { mutableStateOf((if (log) ln(value) else value).toFloat()) }
    Column(Modifier.alpha(if (writable) 1f else 0.5f)) {
        Row {
            Text(label, color = Color.White, fontSize = 14.sp, modifier = Modifier.weight(1f))
            val shown = if (log) exp(position.toDouble()) else position.toDouble()
            Text(format(shown), color = Color.White.copy(alpha = 0.6f), fontSize = 14.sp)
        }
        Slider(
            value = position,
            onValueChange = { position = it },
            onValueChangeFinished = {
                val raw = if (log) exp(position.toDouble()) else position.toDouble()
                onSet(if (step > 0) (raw / step).roundToInt() * step else raw)
            },
            valueRange = (if (log) ln(min) else min).toFloat()..(if (log) ln(max) else max).toFloat(),
            enabled = writable,
        )
    }
}

/** A click with no ripple: the sheet's backdrop and its segments. */
@Composable
private fun Modifier.combinedClickableNoRipple(onClick: () -> Unit): Modifier =
    clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, onClick = onClick)
