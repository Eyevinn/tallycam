package se.eyevinn.tallycam.ui

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateOf
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
import androidx.compose.ui.layout.layout
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import org.json.JSONObject
import se.eyevinn.tallycam.AppModel
import kotlin.math.abs
import kotlin.math.pow
import kotlin.math.roundToInt

/**
 * The zoom rocker at the left edge (docs/UI_DESIGN.md, "Adjust control"):
 * drag to zoom, faster the further (2 doublings a second at full
 * deflection), letting go stops where it is; tap an end for a quarter
 * doubling; double tap the knob for the lens's own 1× (no digital zoom).
 * While it moves, the zoom factor shows beside the knob.
 */
@Composable
fun ZoomRocker(model: AppModel, length: Dp = Chrome.rocker) {
    val haptics = rememberHaptics()
    var moving by remember { mutableStateOf(false) }
    var stepped by remember { mutableIntStateOf(0) }
    // Shown a moment after a move or a step, so the factor reached can be read.
    var showFactor by remember { mutableStateOf(false) }
    LaunchedEffect(moving, stepped) {
        if (moving) showFactor = true
        else if (stepped > 0) { showFactor = true; delay(1000); showFactor = false }
        else { delay(1000); showFactor = false }
    }
    fun zoomTo(factor: Double) {
        val d = model.shading.descriptors["zoom"] ?: return
        val low = d.optDouble("min", 1.0)
        val high = d.optDouble("max", 1.0)
        val value = factor.coerceIn(low, high)
        model.localSet(JSONObject().put("zoom", value), quiet = true)
        haptics.step(atEnd = value <= low || value >= high)
        stepped++
    }
    val zoom = model.shading.values.optDouble("zoom", 1.0)
    Rocker("T", "W", "Zoom", length,
           knobLabel = if (showFactor) "%.1f×".format(zoom) else null,
           onStep = { zoomTo(zoom * 2.0.pow(0.25 * it)) },
           onReset = { zoomTo(1.0) }) { rate ->
        moving = rate != 0.0
        model.rampZoom(rate * MAX_ZOOM_RATE)
    }
}

/**
 * A spring-loaded vertical rocker (zoom on the left edge, the adjust
 * control on the right), the same three gestures everywhere
 * (docs/UI_DESIGN.md, "Adjust control"):
 * - drag the knob up or down: [onRate] gets -1 (full down) to 1 (full up),
 *   rising with the square of the deflection in ten steps so small moves
 *   are slow, and 0 when it is let go and springs back;
 * - tap an end: [onStep] with +1 (top) or -1 (bottom);
 * - double tap the knob: [onReset].
 * [knobLabel] shows beside the knob, on the inner side.
 */
@Composable
fun Rocker(high: String, low: String, label: String, length: Dp = Chrome.rocker, knobLabel: String? = null,
           knobLabelLeading: Boolean = false, onStep: (Int) -> Unit = {}, onReset: () -> Unit = {},
           onRate: (Double) -> Unit) {
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
                val knobZone = 26.dp.toPx()
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
                        if (!dragging && abs(dy) > viewConfiguration.touchSlop) dragging = true
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
        Box(Modifier.matchParentSize().background(Color.Black.copy(alpha = 0.35f), CircleShape))
        Column(Modifier.fillMaxSize().padding(vertical = 8.dp), horizontalAlignment = Alignment.CenterHorizontally,
               verticalArrangement = Arrangement.SpaceBetween) {
            Text(high, color = Color.White.copy(alpha = 0.6f), fontSize = 13.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
            Text(low, color = Color.White.copy(alpha = 0.6f), fontSize = 13.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
        }
        Box(Modifier.offset { IntOffset(0, (-shown * travel.toPx()).roundToInt()) }.size(30.dp)
                .background(Color.White.copy(alpha = if (sentStep == 0) 0.7f else 0.95f), CircleShape))
        knobLabel?.let {
            // Beside the knob, outside the rocker, taking no room.
            Text(it, color = Color.White, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, softWrap = false,
                 modifier = Modifier
                     .layout { measurable, _ ->
                         val p = measurable.measure(Constraints())
                         layout(0, 0) {
                             val x = if (knobLabelLeading) -Chrome.control.roundToPx() / 2 - Chrome.gap.roundToPx() - p.width
                                     else Chrome.control.roundToPx() / 2 + Chrome.gap.roundToPx()
                             p.place(x, (-shown * travel.toPx()).roundToInt() - p.height / 2)
                         }
                     }
                     .background(Color.Black.copy(alpha = 0.55f), CircleShape)
                     .padding(horizontal = 8.dp, vertical = 4.dp))
        }
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
/** Zoom doublings per second at full deflection. */
private const val MAX_ZOOM_RATE = 2.0

/** One button per lens the camera has, longest on top (like the rocker's T); the active one highlighted. */
@Composable
fun LensButtons(model: AppModel) {
    val lenses = model.camera.lenses
    if (lenses.size < 2) return
    // The camera's own value, so a lens the hub picks shows too.
    val current = model.shading.values.optString("lens")
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        for (lens in lenses.reversed()) {
            val label = when (lens.key) {
                "ultra_wide" -> "0.5×"
                "wide" -> "1×"
                else -> "TELE"
            }
            Box(
                Modifier.size(Chrome.control).background(Color.Black.copy(alpha = 0.35f), CircleShape)
                    .border(1.dp, Color.White.copy(alpha = 0.25f), CircleShape)
                    .clickable { model.selectLens(lens.key) }
                    .semantics { contentDescription = lens.label },
                contentAlignment = Alignment.Center,
            ) {
                Text(label, color = if (lens.key == current) Chrome.yellow else Color.White.copy(alpha = 0.85f),
                     fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            }
        }
    }
}
