package se.eyevinn.tallycam.ui

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
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
import androidx.compose.ui.semantics.contentDescription
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
 * A vertical zoom rocker: push up to zoom in, down to zoom out; how far
 * sets the speed, for a usable zoom move on air. Letting go stops the zoom
 * where it is.
 */
@Composable
fun ZoomRocker(length: Dp = Chrome.rocker, onRate: (Double) -> Unit) {
    Rocker("T", "W", "Zoom", length) { onRate(it * MAX_ZOOM_RATE) }
}

/**
 * A spring-loaded vertical rocker (zoom on the left edge, the adjust
 * control on the right), as on iOS: drag the knob up or down; how far sets
 * the speed. [onRate] gets -1 (full down) to 1 (full up), rising with the
 * square of the deflection in ten steps so small moves are slow, and 0 when
 * the knob is let go and springs back.
 */
@Composable
fun Rocker(high: String, low: String, label: String, length: Dp = Chrome.rocker, onRate: (Double) -> Unit) {
    val travel = (length - Chrome.control - 10.dp) / 2
    var deflection by remember { mutableFloatStateOf(0f) }
    var sentStep by remember { mutableIntStateOf(0) }
    val shown by animateFloatAsState(deflection, label = "rocker")
    val report by rememberUpdatedState(onRate)
    Box(
        Modifier.width(Chrome.control).height(length).background(Color.Black.copy(alpha = 0.35f), CircleShape)
            .semantics { contentDescription = label }
            .pointerInput(length) {
                val travelPx = travel.toPx()
                var dy = 0f
                fun move(d: Float) {
                    deflection = d
                    val step = (d * ROCKER_STEPS).roundToInt()
                    if (step == sentStep) return
                    sentStep = step
                    val share = abs(step).toDouble() / ROCKER_STEPS
                    report(if (step == 0) 0.0 else (if (step > 0) 1 else -1) * share * share)
                }
                detectDragGestures(
                    onDragStart = { dy = 0f },
                    onDragEnd = { move(0f) },
                    onDragCancel = { move(0f) },
                ) { change, amount ->
                    change.consume()
                    dy += amount.y
                    move((-dy / travelPx).coerceIn(-1f, 1f))
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        Column(Modifier.fillMaxSize().padding(vertical = 8.dp), horizontalAlignment = Alignment.CenterHorizontally,
               verticalArrangement = Arrangement.SpaceBetween) {
            Text(high, color = Color.White.copy(alpha = 0.6f), fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
            Text(low, color = Color.White.copy(alpha = 0.6f), fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
        }
        Box(Modifier.offset { IntOffset(0, (-shown * travel.toPx()).roundToInt()) }.size(30.dp)
                .background(Color.White.copy(alpha = if (sentStep == 0) 0.7f else 0.95f), CircleShape))
    }
}

private const val ROCKER_STEPS = 10
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
