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
 * manual is yellow, auto is white; the groups the side controls run are
 * underlined in their colours. An exposure warning colours ISO and follows it ("ISO 100
 * DARK", "CLIP"). Follows
 * the values the hub sets too, and a value the hub just changed flashes
 * cyan for a second. On a narrow screen focus goes first, then
 * white balance; the line never wraps.
 *
 * Android's manual white balance is an AWB preset and its locked one keeps
 * what auto found: "AWB" unless a temperature is set.
 */
@Composable
fun CameraValuesRow(model: AppModel, onSides: Collection<String>, onSelect: (String) -> Unit, onFormat: () -> Unit,
                    modifier: Modifier = Modifier, bare: Boolean = false, dropped: Int = 0) {
    // [dropped]: groups left out up front, in the drop order (the landscape top bar picks how many fit).
    val gone = VALUE_DROP_ORDER.take(dropped)
    val items = valueItems(model.shading.values, model.shading.format, model.shading.exposureWarning).filter { it.group !in gone }
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
    // In the landscape top bar, on the bar's own background.
    Box(modifier.height(Chrome.rowHeight).then(if (bare) Modifier else Modifier.background(Chrome.surface, CircleShape))
            .padding(horizontal = 4.dp), contentAlignment = Alignment.Center) {
        // Focus is last; colour is dropped after it.
        val drop = VALUE_DROP_ORDER.mapNotNull { group -> items.indexOfFirst { it.group == group }.takeIf { it >= 0 } }
        DroppingRow(drop) {
            items.forEachIndexed { index, item ->
                val enabled = item.group == null || modeKey(item.group) in descriptors
                ValueButton(item, separator = index > 0, selected = item.group != null && item.group in onSides,
                            enabled = enabled, flash = item.ids.any { it in flashing }) {
                    if (item.group == null) onFormat() else onSelect(item.group)
                }
            }
        }
    }
}

/** A tone as drawn. */
private fun Tone.color(): Color = when (this) {
    Tone.AUTO -> Color.White
    Tone.HELD -> Chrome.yellow
    Tone.ORANGE -> Chrome.orange
    Tone.RED -> Color(0xFFFF453A)
}

@Composable
private fun ValueButton(item: ValueItem, separator: Boolean, selected: Boolean, enabled: Boolean, flash: Boolean,
                        onClick: () -> Unit) {
    val tone by animateColorAsState(if (flash) HUB_CYAN else item.tone.color(), tween(if (flash) 80 else 400), label = "flash")
    Row(Modifier.height(Chrome.rowHeight), verticalAlignment = Alignment.CenterVertically) {
        if (separator) Box(Modifier.width(1.dp).height(14.dp).background(Color.White.copy(alpha = 0.25f)))
        Box(
            Modifier.fillMaxHeight().clickable(enabled = enabled, onClick = onClick)
                .semantics { contentDescription = listOfNotNull(item.text, item.flag).joinToString(" ") }
                .padding(horizontal = 9.dp),
            contentAlignment = Alignment.Center,
        ) {
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(item.text, color = tone, fontSize = 14.sp, style = Chrome.tabular,
                     fontWeight = FontWeight.SemiBold, maxLines = 1, softWrap = false)
                item.flag?.let {
                    Text(it, color = tone, fontSize = 14.sp, fontWeight = FontWeight.Black, maxLines = 1, softWrap = false)
                }
            }
            if (selected) {
                Box(Modifier.align(Alignment.BottomCenter).padding(bottom = 4.dp).width(24.dp).height(2.dp)
                        .background(Chrome.groupColor(item.group ?: ""), CircleShape))
            }
        }
    }
}

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
