package se.eyevinn.tallycam.ui

import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.view.Surface
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CheckboxDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import se.eyevinn.tallycam.AppModel
import se.eyevinn.tallycam.media.MicLevels
import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.log10
import kotlin.math.roundToInt

/**
 * The operator's framing and focus aids, each a persisted toggle, as on
 * iOS (ios/TallyCam/Views/Aids.swift). Drawn over the preview only, never
 * into the stream (docs/SHADING.md). Grid and horizon level are on until
 * first turned off, the others off (docs/UI_DESIGN.md).
 */
class Aids(context: Context) {
    private val prefs = context.getSharedPreferences("aids", Context.MODE_PRIVATE)

    var grid by mutableStateOf(prefs.getBoolean("grid", true)); private set
    var horizon by mutableStateOf(prefs.getBoolean("horizon", true)); private set
    var safeAreas by mutableStateOf(prefs.getBoolean("safeAreas", false)); private set
    var centreCuts by mutableStateOf(prefs.getBoolean("centreCuts", false)); private set
    var peaking by mutableStateOf(prefs.getBoolean("peaking", false)); private set

    val anyOn get() = grid || horizon || safeAreas || centreCuts || peaking

    fun set(key: String, on: Boolean) {
        prefs.edit().putBoolean(key, on).apply()
        when (key) {
            "grid" -> grid = on
            "horizon" -> horizon = on
            "safeAreas" -> safeAreas = on
            "centreCuts" -> centreCuts = on
            "peaking" -> peaking = on
        }
    }

    companion object {
        /** Focus peaking edges: one colour, red, as most camera monitors. */
        val PEAKING_TINT = Color(1f, 0.15f, 0.1f)
    }
}

/** The button that opens the aids menu. */
@Composable
fun AidsMenu(aids: Aids, model: AppModel) {
    var open by remember { mutableStateOf(false) }
    Box {
        ChromeButton("▦", active = aids.anyOn) { open = true }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            @Composable
            fun item(key: String, label: String, on: Boolean) = DropdownMenuItem(
                text = { Text(label) },
                leadingIcon = {
                    Checkbox(on, onCheckedChange = null, colors = CheckboxDefaults.colors(checkedColor = Color(0xFFFFD60A)))
                },
                onClick = { aids.set(key, !on) },
            )
            item("grid", "Grid (thirds)", aids.grid)
            item("horizon", "Horizon level", aids.horizon)
            item("safeAreas", "Safe areas (93% / 90%)", aids.safeAreas)
            item("centreCuts", "Centre cuts (4:3, 9:16)", aids.centreCuts)
            HorizontalDivider()
            item("peaking", "Focus peaking (sharp edges in red)", aids.peaking)
            // The torch: a light on the subject, kept with the switches flipped
            // while filming; only where the lens has one, and the hub can switch it too.
            if ("torch" in model.shading.descriptors) {
                HorizontalDivider()
                val torch = model.shading.values.optBoolean("torch")
                DropdownMenuItem(
                    text = { Text("Torch") },
                    leadingIcon = {
                        Checkbox(torch, onCheckedChange = null, colors = CheckboxDefaults.colors(checkedColor = Color(0xFFFFD60A)))
                    },
                    onClick = { model.localSet(org.json.JSONObject().put("torch", !torch)) },
                )
            }
        }
    }
}

/** Rule of thirds, safe areas and centre cuts over the whole (16:9) picture. */
@Composable
fun FrameGuides(aids: Aids, modifier: Modifier = Modifier) {
    if (!aids.grid && !aids.safeAreas && !aids.centreCuts) return
    Canvas(modifier.fillMaxSize()) {
        val stroke = Stroke(width = 1.dp.toPx())
        if (aids.grid) {
            for (i in 1..2) {
                val x = size.width * i / 3
                val y = size.height * i / 3
                drawLine(Color.White.copy(alpha = 0.4f), Offset(x, 0f), Offset(x, size.height), stroke.width)
                drawLine(Color.White.copy(alpha = 0.4f), Offset(0f, y), Offset(size.width, y), stroke.width)
            }
        }
        if (aids.safeAreas) {
            rect(inset(0.93f), Color.White.copy(alpha = 0.5f), stroke)
            rect(inset(0.90f), Color.White.copy(alpha = 0.35f),
                 Stroke(stroke.width, pathEffect = PathEffect.dashPathEffect(floatArrayOf(6.dp.toPx(), 4.dp.toPx()))))
        }
        if (aids.centreCuts) {
            for (aspect in floatArrayOf(4f / 3f, 9f / 16f)) {
                centreCut(aspect)?.let {
                    rect(it, Color.Cyan.copy(alpha = 0.55f),
                         Stroke(stroke.width, pathEffect = PathEffect.dashPathEffect(floatArrayOf(3.dp.toPx(), 3.dp.toPx()))))
                }
            }
        }
    }
}

private fun DrawScope.inset(share: Float): Rect {
    val dx = size.width * (1 - share) / 2
    val dy = size.height * (1 - share) / 2
    return Rect(dx, dy, size.width - dx, size.height - dy)
}

/** The largest rect of `aspect` centred in the picture, or null when the picture already has about that aspect. */
private fun DrawScope.centreCut(aspect: Float): Rect? {
    val frame = size.width / size.height
    if (abs(frame / aspect - 1) <= 0.02f) return null
    val cut = if (frame > aspect) Size(size.height * aspect, size.height) else Size(size.width, size.width / aspect)
    return Rect(Offset((size.width - cut.width) / 2, (size.height - cut.height) / 2), cut)
}

private fun DrawScope.rect(r: Rect, color: Color, stroke: Stroke) =
    drawRect(color, r.topLeft, r.size, style = stroke)

/**
 * A horizon line from the gravity sensor, green within a degree of level.
 * Shown only while the phone is held roughly upright (not lying flat, and
 * within 30 degrees of level).
 */
@Composable
fun HorizonLevel(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    var angle by remember { mutableStateOf<Float?>(null) }
    DisposableEffect(Unit) {
        val sensors = context.getSystemService(Context.SENSOR_SERVICE) as SensorManager
        val gravity = sensors.getDefaultSensor(Sensor.TYPE_GRAVITY) ?: sensors.getDefaultSensor(Sensor.TYPE_ACCELEROMETER)
        var smoothed: Float? = null
        val listener = object : SensorEventListener {
            override fun onSensorChanged(event: SensorEvent) {
                val (gx, gy, gz) = Triple(event.values[0], event.values[1], event.values[2])
                // "Up" in screen coordinates (x right, y down) for the display's rotation.
                @Suppress("DEPRECATION")
                val rotation = (context.getSystemService(Context.WINDOW_SERVICE) as android.view.WindowManager)
                    .defaultDisplay.rotation
                val (ux, uy) = when (rotation) {
                    Surface.ROTATION_90 -> -gy to -gx
                    Surface.ROTATION_270 -> gy to gx
                    Surface.ROTATION_180 -> -gx to gy
                    else -> gx to -gy
                }
                val norm = kotlin.math.sqrt(gx * gx + gy * gy + gz * gz).coerceAtLeast(0.1f)
                val roll = atan2(ux, -uy)
                if (abs(gz) / norm > 0.8f || abs(roll) > MAX_ROLL) {
                    smoothed = null
                    angle = null
                    return
                }
                val value = smoothed?.let { it + 0.3f * (roll - it) } ?: roll
                smoothed = value
                angle = value
            }

            override fun onAccuracyChanged(sensor: Sensor?, accuracy: Int) = Unit
        }
        gravity?.let { sensors.registerListener(listener, it, SensorManager.SENSOR_DELAY_GAME) }
        onDispose { sensors.unregisterListener(listener) }
    }
    val a = angle ?: return
    val level = abs(a) <= LEVEL_TOLERANCE
    val color = if (level) Color(0xFF34C759) else Color.White.copy(alpha = 0.75f)
    Canvas(modifier.fillMaxSize().rotate(Math.toDegrees(a.toDouble()).toFloat())) {
        val width = minOf(size.width, size.height) * 0.6f
        val c = center
        val bar = width * 0.4f
        val w = 2.dp.toPx()
        drawLine(color, Offset(c.x - width / 2, c.y), Offset(c.x - width / 2 + bar, c.y), w)
        drawLine(color, Offset(c.x + width / 2 - bar, c.y), Offset(c.x + width / 2, c.y), w)
        drawCircle(color, 5.dp.toPx(), c, style = Stroke(1.5.dp.toPx()))
    }
}

private const val LEVEL_TOLERANCE = (1.0 * Math.PI / 180).toFloat()
private const val MAX_ROLL = (30.0 * Math.PI / 180).toFloat()

/** Focus peaking: the camera's edge mask, tinted, over the picture (D10). */
@Composable
fun PeakingOverlay(model: AppModel, modifier: Modifier = Modifier) {
    var mask by remember { mutableStateOf<android.graphics.Bitmap?>(null) }
    var serial by remember { mutableIntStateOf(0) }
    DisposableEffect(Unit) {
        val main = android.os.Handler(android.os.Looper.getMainLooper())
        model.camera.onPeaking = { bitmap -> main.post { mask = bitmap; serial++ } }
        onDispose {
            model.camera.onPeaking = null
            mask = null
        }
    }
    val bitmap = mask ?: return
    // `serial`: a new mask in a reused bitmap still redraws.
    androidx.compose.runtime.key(serial) {
        Image(bitmap.asImageBitmap(), contentDescription = null, modifier = modifier.fillMaxSize(),
              contentScale = ContentScale.FillBounds, colorFilter = ColorFilter.tint(Aids.PEAKING_TINT))
    }
}

/**
 * A small vertical microphone meter, as on iOS: RMS as the bar, the peak as
 * a line falling back slowly, and a clip marker that holds red for a second
 * after a peak above -1 dBFS. Shown while the microphone is open (live).
 */
@Composable
fun AudioMeter(modifier: Modifier = Modifier) {
    var rmsDb by remember { mutableFloatStateOf(-90f) }
    var peakDb by remember { mutableFloatStateOf(-90f) }
    var clipUntil by remember { mutableStateOf(0L) }
    var active by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        var lastChunks = MicLevels.chunks
        var still = 0
        while (true) {
            delay(66)
            val chunks = MicLevels.chunks
            still = if (chunks == lastChunks) still + 1 else 0
            lastChunks = chunks
            active = still < 8
            val (peak, rms) = MicLevels.read()
            val peakNow = db(peak)
            rmsDb = db(rms)
            peakDb = maxOf(peakNow, peakDb - 1.3f)
            if (peakNow > -1f) clipUntil = System.currentTimeMillis() + 1000
        }
    }
    if (!active) return
    val clipping = clipUntil > System.currentTimeMillis()
    Column(
        modifier.background(Chrome.surface, CircleShape).border(1.dp, Chrome.border, CircleShape).padding(horizontal = 5.dp, vertical = 6.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(3.dp),
    ) {
        Box(Modifier.width(12.dp).height(6.dp)
                .background(if (clipping) Color.Red else Color.White.copy(alpha = 0.2f), RoundedCornerShape(1.5.dp)))
        Canvas(Modifier.width(12.dp).height(75.dp)) {
            drawRect(Color.White.copy(alpha = 0.12f))
            val h = size.height
            val rms = position(rmsDb) * h
            drawRect(color(rmsDb), Offset(0f, h - rms), Size(size.width, rms))
            if (peakDb > FLOOR_DB) {
                val y = h - position(peakDb) * h
                drawRect(color(peakDb), Offset(0f, y - 1.5f), Size(size.width, 1.5.dp.toPx()))
            }
        }
    }
}

private const val FLOOR_DB = -60f
private fun db(linear: Float) = if (linear > 0) 20 * log10(linear) else -90f
private fun position(db: Float) = ((db - FLOOR_DB) / -FLOOR_DB).coerceIn(0f, 1f)
private fun color(db: Float) = if (db > -6) Color.Red else if (db > -18) Color(0xFFFFD60A) else Color(0xFF34C759)

/**
 * Under the chrome while the screen is locked: swallows every touch on the
 * picture (no tap to focus). Remote control, tally and the stream keep
 * running; only touches are blocked (docs/SHADING.md). The chrome itself
 * takes no touches either ([touchable]) but the slide to unlock.
 */
@Composable
fun ScreenLockShield() {
    Box(Modifier.fillMaxSize().pointerInput(Unit) {
        awaitPointerEventScope { while (true) awaitPointerEvent().changes.forEach { it.consume() } }
    })
}

/** Drag the knob to the end to unlock the screen. */
@Composable
fun SlideToUnlock(onUnlock: () -> Unit, modifier: Modifier = Modifier) {
    val width = 240.dp
    val knob = 46.dp
    var offset by remember { mutableFloatStateOf(0f) }
    val shown by animateFloatAsState(offset, label = "unlock")
    Box(modifier.width(width).height(knob + 6.dp).background(Color.Black.copy(alpha = 0.5f), CircleShape)
            .pointerInput(Unit) {
                val travel = (width - knob - 6.dp).toPx()
                detectHorizontalDragGestures(
                    onDragEnd = {
                        if (offset > travel * 0.85f) onUnlock()
                        offset = 0f
                    },
                    onDragCancel = { offset = 0f },
                ) { change, amount ->
                    change.consume()
                    offset = (offset + amount).coerceIn(0f, travel)
                }
            },
        contentAlignment = Alignment.CenterStart,
    ) {
        Text("slide to unlock", color = Color.White.copy(alpha = 0.6f), fontSize = 13.sp,
             modifier = Modifier.fillMaxWidth().padding(start = knob), textAlign = androidx.compose.ui.text.style.TextAlign.Center)
        Box(Modifier.offset { IntOffset((3.dp.toPx() + shown).roundToInt(), 0) }.size(knob)
                .background(Color.White.copy(alpha = 0.9f), CircleShape), contentAlignment = Alignment.Center) {
            Text("🔓", fontSize = 18.sp)
        }
    }
}
