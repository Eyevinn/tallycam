package se.eyevinn.tallycam.ui

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import android.view.HapticFeedbackConstants
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import se.eyevinn.tallycam.AppModel
import kotlin.math.abs
import kotlin.math.roundToInt

/**
 * A spring-loaded vertical rocker, the same on both side controls and for
 * every group (docs/UI_DESIGN.md, "Side control"):
 * - drag the knob up or down: [onRate] gets -1 (full down) to 1 (full up),
 *   rising with the square of the deflection in ten steps so small moves
 *   are slow, and 0 when it is let go and springs back;
 * - tap an end: [onStep] with +1 (top) or -1 (bottom);
 * - double tap the knob: [onReset].
 * A touch that moves less than [Chrome.tapSlop] is a tap; the knob zone is
 * the knob and 4 dp round it; a double tap is two within 350 ms (as iOS).
 * [accent] is the group's colour: the outline and the knob's ring.
 */
@Composable
fun Rocker(high: String, low: String, label: String, length: Dp = Chrome.rocker, accent: Color = Color.White.copy(alpha = 0.25f),
           onStep: (Int) -> Unit = {}, onReset: () -> Unit = {}, onRate: (Double) -> Unit) {
    val travel = (length - Chrome.control - 10.dp) / 2
    var deflection by remember { mutableFloatStateOf(0f) }
    var sentStep by remember { mutableIntStateOf(0) }
    val shown by animateFloatAsState(deflection, label = "rocker")
    val report by rememberUpdatedState(onRate)
    val step by rememberUpdatedState(onStep)
    val reset by rememberUpdatedState(onReset)
    Box(
        Modifier.width(Chrome.control).height(length)
            .semantics {
                contentDescription = label
                customActions = listOf(CustomAccessibilityAction(high) { step(1); true },
                                       CustomAccessibilityAction(low) { step(-1); true })
            }
            .pointerInput(length) {
                val travelPx = travel.toPx()
                // The knob and 4 dp round it, as iOS.
                val knobZone = 19.dp.toPx()
                val slop = Chrome.tapSlop.toPx()
                var lastKnobTap = 0L
                fun move(d: Float) {
                    deflection = d
                    val q = (d * ROCKER_STEPS).roundToInt()
                    if (q == sentStep) return
                    sentStep = q
                    val share = abs(q).toDouble() / ROCKER_STEPS
                    report(if (q == 0) 0.0 else (if (q > 0) 1 else -1) * share * share)
                }
                awaitEachGesture {
                    val down = awaitFirstDown()
                    var dragging = false
                    while (true) {
                        val change = awaitPointerEvent().changes.firstOrNull { it.id == down.id } ?: break
                        if (!change.pressed) break
                        val dy = change.position.y - down.position.y
                        if (!dragging && abs(dy) > slop) dragging = true
                        if (dragging) {
                            change.consume()
                            move((-dy / travelPx).coerceIn(-1f, 1f))
                        }
                    }
                    if (dragging) {
                        move(0f)
                        return@awaitEachGesture
                    }
                    val y = down.position.y - size.height / 2
                    when {
                        y < -knobZone -> step(1)
                        y > knobZone -> step(-1)
                        down.uptimeMillis - lastKnobTap < DOUBLE_TAP_MS -> { lastKnobTap = 0; reset() }
                        else -> lastKnobTap = down.uptimeMillis
                    }
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.matchParentSize().background(Color.Black.copy(alpha = 0.35f), CircleShape).border(1.5.dp, accent, CircleShape))
        Column(Modifier.fillMaxSize().padding(vertical = 8.dp), horizontalAlignment = Alignment.CenterHorizontally,
               verticalArrangement = Arrangement.SpaceBetween) {
            // Smaller for words (Far, Near) than for signs.
            val size = if (high.length > 2 || low.length > 2) 11.sp else 13.sp
            Text(high, color = Color.White.copy(alpha = 0.6f), fontSize = size, fontWeight = FontWeight.SemiBold, maxLines = 1)
            Text(low, color = Color.White.copy(alpha = 0.6f), fontSize = size, fontWeight = FontWeight.SemiBold, maxLines = 1)
        }
        Box(Modifier.offset { IntOffset(0, (-shown * travel.toPx()).roundToInt()) }.size(30.dp)
                .background(Color.White.copy(alpha = if (sentStep == 0) 0.7f else 0.95f), CircleShape)
                .border(2.dp, accent, CircleShape))
    }
}

/** The rockers' haptics: a light tick per step or tap, a firmer one at the end of the range. */
class Haptics(private val view: android.view.View) {
    fun step(atEnd: Boolean = false) {
        view.performHapticFeedback(if (atEnd) HapticFeedbackConstants.LONG_PRESS else HapticFeedbackConstants.CLOCK_TICK)
    }
}

@Composable
fun rememberHaptics(): Haptics {
    val view = LocalView.current
    return remember(view) { Haptics(view) }
}

private const val ROCKER_STEPS = 10
private const val DOUBLE_TAP_MS = 350L

/** A lens's factor against the wide lens, from the focal lengths (1 without a wide lens). */
fun lensFactor(model: AppModel, lens: se.eyevinn.tallycam.media.CameraCapture.Lens? = null): Double {
    val lenses = model.camera.lenses
    val current = lens ?: lenses.firstOrNull { it.key == model.shading.values.optString("lens") } ?: return 1.0
    val wide = lenses.firstOrNull { it.key == "wide" }?.focalLength ?: return 1.0
    return if (wide > 0) current.focalLength.toDouble() / wide else 1.0
}

/** "0.5×", "1×", "3×": the lens's factor, as the camera apps label them. */
fun lensLabel(model: AppModel, lens: se.eyevinn.tallycam.media.CameraCapture.Lens): String {
    val f = lensFactor(model, lens)
    return when {
        f < 0.95 -> fmt("%.1f×", f)
        kotlin.math.abs(f - Math.round(f)) < 0.15 -> "${Math.round(f)}×"
        else -> fmt("%.1f×", f)
    }
}
