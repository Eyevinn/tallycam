package se.eyevinn.tallycam.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * The shooting screen's one grid: every piece of chrome is placed with
 * these, never with its own numbers (docs/APPS.md, "The shooting screen"),
 * as on iOS (ios/TallyCam/Views/Chrome.swift).
 */
object Chrome {
    /** Width of the tally border plus a margin: chrome stays clear of it. */
    val tallyClearance = 22.dp
    /** Least padding inside a safe-area inset. */
    val minEdge = 8.dp
    /** Round buttons, rockers and the bottom row. */
    val control = 46.dp
    /** The status and camera values rows. */
    val rowHeight = 34.dp
    /** Between the controls of one group. */
    val gap = 10.dp
    /** Between the top rows. */
    val rowGap = 6.dp
    /** Between the zones (top, middle, bottom). */
    val zoneGap = 10.dp
    /** The live button. */
    val liveButton = 76.dp
    /** A side control's cells: easy to hit without looking (docs/UI_DESIGN.md, "Sizes", "Shape"). */
    val cell = 48.dp
    val cellGap = 4.dp
    val cellRadius = 8.dp
    /** The selector and the mode row: two cells wide. */
    val blockWidth = cell * 2 + cellGap
    /** The rockers: as big as the screen allows (docs/UI_DESIGN.md, "Sizes"). */
    val rockerWidth = 56.dp
    /** The rocker track's and knob's corners. */
    val rockerRadius = 10.dp
    val knobRadius = 8.dp
    /** The readout's slot when it sits beside the block. */
    val readoutWidth = 84.dp
    /** The value label under a side control's rocker, and the K / Tint row under it. */
    val valueHeight = 28.dp
    val tintHeight = 24.dp
    /** A rocker touch that moves less than this is a tap (as iOS's 8 pt). */
    val tapSlop = 8.dp

    /** Padding inside the safe area so chrome clears the tally border on edges without an inset. */
    fun edge(inset: Dp): Dp = maxOf(minEdge, tallyClearance - inset)

    /** Values: tabular digits in the default face (iOS `monospacedDigit`). */
    val tabular = TextStyle(fontFeatureSettings = "tnum")

    /**
     * A side-control group's colour: the rocker's outline and knob ring, the
     * chosen selector segment, the underline of its values; never value text.
     */
    fun groupColor(group: String): Color = when (group) {
        "zoom" -> Color(0xFF3B82F6)
        "exposure" -> Color(0xFFEF4444)
        "white_balance" -> Color(0xFFA855F7)
        else -> Color(0xFF22C55E)
    }

    val yellow = Color(0xFFFFD60A)
    val orange = Color(0xFFFF9F0A)
    val red = Color(0xFFE5322D)
    val green = Color(0xFF34C759)
}

/** A small round translucent button face, shared by the operator's chrome. */
@Composable
fun ChromeButton(label: String, active: Boolean = false, size: Dp = Chrome.control, onClick: () -> Unit) {
    Box(
        Modifier.size(size).background(Color.Black.copy(alpha = 0.35f), CircleShape)
            .border(1.dp, Color.White.copy(alpha = 0.25f), CircleShape)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = if (active) Chrome.yellow else Color.White.copy(alpha = 0.85f),
             fontSize = (size.value * 0.39f).sp, fontWeight = FontWeight.Medium)
    }
}

/**
 * While [locked], nothing inside gets a touch (it stays visible): every
 * pointer change is consumed before the children see it.
 */
fun Modifier.touchable(locked: Boolean): Modifier =
    if (!locked) this else pointerInput(Unit) {
        awaitPointerEventScope {
            while (true) awaitPointerEvent(PointerEventPass.Initial).changes.forEach { it.consume() }
        }
    }
