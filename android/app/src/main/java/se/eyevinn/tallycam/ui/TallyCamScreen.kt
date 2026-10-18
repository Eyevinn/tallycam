package se.eyevinn.tallycam.ui

import android.graphics.SurfaceTexture
import android.view.TextureView
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.calculateEndPadding
import androidx.compose.foundation.layout.calculateStartPadding
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.layout
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import se.eyevinn.tallycam.AppModel
import se.eyevinn.tallycam.control.ProvisioningLink
import se.eyevinn.tallycam.control.Shading
import se.eyevinn.tallycam.media.LiveStream

/**
 * The shooting screen, as on iOS (ios/TallyCam/ContentView.swift): the
 * full-bleed preview and tally, and the chrome on one grid ([Chrome]).
 * Information in the middle, controls at the sides, under the thumbs. Top:
 * the status line and the camera values row, with one slot under them for
 * banners and toasts. Left edge: lenses and zoom. Right edge: the adjust
 * control of the value being changed. Bottom: tools left, the microphone
 * meter right, and the live button in the bottom-right corner (bottom
 * centre in portrait). Everything else is in Settings.
 */
@Composable
fun TallyCamScreen(model: AppModel?, permitted: Boolean, onScan: () -> Unit, pendingLink: ProvisioningLink? = null,
                   onJoin: (ProvisioningLink) -> Unit = {}, onDecline: () -> Unit = {}) {
    MaterialTheme(colorScheme = darkColorScheme()) {
        Box(Modifier.fillMaxSize().background(Color.Black)) {
            if (model == null) {
                Column(Modifier.align(Alignment.Center).padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Text(if (permitted) "Starting…" else "TallyCam needs the camera and the microphone.", color = Color.White)
                    if (!permitted) {
                        val context = LocalContext.current
                        TextButton(onClick = {
                            context.startActivity(android.content.Intent(
                                android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                                android.net.Uri.fromParts("package", context.packageName, null)))
                        }) { Text("Open settings") }
                    }
                }
                return@Box
            }
            ShootingScreen(model, onScan)
            pendingLink?.let { JoinDialog(it, live = model.streamState != LiveStream.State.IDLE, onJoin, onDecline) }
        }
    }
}

@Composable
private fun BoxScope.ShootingScreen(model: AppModel, onScan: () -> Unit) {
    val context = LocalContext.current
    val aids = remember { Aids(context) }
    var showSettings by remember { mutableStateOf(false) }
    var connectCardDismissed by remember { mutableStateOf(false) }
    // Touches blocked except the slide to unlock; not persisted.
    var screenLocked by remember { mutableStateOf(false) }
    // The group the adjust control at the right edge has: exposure until
    // another is picked, remembered across launches.
    val prefs = remember { context.getSharedPreferences("chrome", android.content.Context.MODE_PRIVATE) }
    var chosenGroup by remember { mutableStateOf(prefs.getString("group", null) ?: "exposure") }
    fun choose(group: String) {
        chosenGroup = group
        prefs.edit().putString("group", group).apply()
    }
    // The chosen one where the camera has it, else the first it has.
    val openGroup = Shading.MODE_CONTROLS.map { it.first }.filter { modeKey(it) in model.shading.descriptors }
        .let { groups -> chosenGroup.takeIf { it in groups } ?: groups.firstOrNull() }
    var toast by remember { mutableStateOf<String?>(null) }
    var toastSerial by remember { mutableIntStateOf(0) }
    fun showToast(text: String) {
        toast = text
        toastSerial++
    }
    LaunchedEffect(toastSerial) {
        if (toast != null) {
            delay(4000)
            toast = null
        }
    }
    LaunchedEffect(model.streamMessage) { model.streamMessage?.let { showToast(it) } }

    val idle = model.streamState == LiveStream.State.IDLE
    val tools = @Composable {
        // Aids and screen lock; the QR scanner too while no hub is connected and not live.
        Row(horizontalArrangement = Arrangement.spacedBy(Chrome.gap)) {
            AidsMenu(aids)
            if (model.link != AppModel.Link.CONNECTED && idle) {
                Box(Modifier.semantics { contentDescription = "Scan QR code" }) { ChromeButton("⌗", onClick = onScan) }
            }
            Box(Modifier.semantics { contentDescription = "Lock screen" }) {
                ChromeButton("🔒") { screenLocked = true }
            }
        }
    }
    val liveButton = @Composable { hintAbove: Boolean ->
        LiveButton(model.streamState, hintAbove, onStart = {
            model.goLive()?.let { showToast(it) } ?: run {
                // Suggested, not forced: a camera on a tripod may need its controls.
                if (!screenLocked) showToast("Lock the screen against stray touches: lock button, bottom left")
            }
        }, onStop = { model.stopLive() })
    }

    Preview(model, aids, Modifier.fillMaxSize())
    TallyOverlay(model.tally)
    if (screenLocked) ScreenLockShield()

    // Inside the camera cutout's safe area (whichever side it is on), plus
    // the edge that clears the tally border. The system bars are hidden and
    // only swipe in over the chrome, so they do not move it.
    val insets = WindowInsets.displayCutout.asPaddingValues()
    val direction = LocalLayoutDirection.current
    fun inside(inset: Dp) = inset + Chrome.edge(inset)
    BoxWithConstraints(Modifier.fillMaxSize().padding(
        start = inside(insets.calculateStartPadding(direction)),
        end = inside(insets.calculateEndPadding(direction)),
        top = inside(insets.calculateTopPadding()),
        bottom = inside(insets.calculateBottomPadding()),
    )) {
        val landscape = maxWidth > maxHeight
        // Narrower than this (portrait), the camera's name gets a row of its own.
        val nameInline = maxWidth >= 560.dp
        val rows = if (nameInline) 2 else 3
        val top = Chrome.rowHeight * rows + Chrome.rowGap * (rows - 1)
        // The rockers' length: the full one, or what a landscape phone has
        // between the top rows and the bottom row (both the same).
        val rocker = if (!landscape) Chrome.rocker
                     else (maxHeight - top - Chrome.zoneGap * 2 - Chrome.liveButton).coerceIn(100.dp, Chrome.rocker)
        // The height the adjust control has for its parts stacked over the rocker.
        val edgeHeight = maxHeight - top - Chrome.zoneGap * 2 - Chrome.liveButton -
            (if (landscape) 0.dp else Chrome.control + Chrome.zoneGap * 2)

        val nameChip = @Composable { TallyChip(model.tally, model.displayName) }
        val zoom = @Composable {
            ZoomColumn(model, rocker, edgeHeight, screenLocked, Modifier.touchable(screenLocked))
        }
        val adjust = @Composable {
            openGroup?.let { AdjustControl(model, it, rocker, edgeHeight, screenLocked, Modifier.touchable(screenLocked)) }
        }
        val messages = @Composable { MessageSlot(model.banner, toast) }
        val unlock = @Composable { if (screenLocked) SlideToUnlock({ screenLocked = false }) }

        Column(verticalArrangement = Arrangement.spacedBy(Chrome.zoneGap)) {
            Column(Modifier.fillMaxWidth().touchable(screenLocked), horizontalAlignment = Alignment.CenterHorizontally,
                   verticalArrangement = Arrangement.spacedBy(Chrome.rowGap)) {
                StatusStrip(model, if (nameInline) nameChip else null) { showSettings = true }
                if (!nameInline) nameChip()
                CameraValuesRow(model, openGroup, ::choose, onFormat = { showSettings = true })
            }
            if (landscape) {
                // Left: zoom over the tools. Centre: messages, the slide to
                // unlock, the meter. Right: the adjust control over the live
                // button. The bottom row is the live button's height, so both
                // edge columns centre on the same line.
                Row(Modifier.weight(1f), horizontalArrangement = Arrangement.spacedBy(Chrome.gap)) {
                    Column(Modifier.fillMaxHeight(), verticalArrangement = Arrangement.spacedBy(Chrome.zoneGap)) {
                        Box(Modifier.weight(1f), contentAlignment = Alignment.CenterStart) { zoom() }
                        Box(Modifier.height(Chrome.liveButton).touchable(screenLocked), contentAlignment = Alignment.BottomStart) {
                            tools()
                        }
                    }
                    Column(Modifier.weight(1f).fillMaxHeight(), horizontalAlignment = Alignment.CenterHorizontally,
                           verticalArrangement = Arrangement.spacedBy(Chrome.zoneGap)) {
                        Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.TopCenter) { messages() }
                        Box(Modifier.height(Chrome.liveButton).fillMaxWidth(), contentAlignment = Alignment.BottomCenter) {
                            unlock()
                            AudioMeter(Modifier.align(Alignment.BottomEnd))
                        }
                    }
                    Column(Modifier.fillMaxHeight(), horizontalAlignment = Alignment.End,
                           verticalArrangement = Arrangement.spacedBy(Chrome.zoneGap)) {
                        // Keeps the live button in the corner while nothing is being adjusted.
                        Box(Modifier.weight(1f), contentAlignment = Alignment.CenterEnd) { adjust() }
                        Box(Modifier.touchable(screenLocked)) { liveButton(true) }
                    }
                }
            } else {
                // Mirror images, the two rockers level; messages between them.
                Box(Modifier.weight(1f).fillMaxWidth()) {
                    Box(Modifier.align(Alignment.CenterStart)) { zoom() }
                    Box(Modifier.align(Alignment.CenterEnd)) { adjust() }
                    Box(Modifier.align(Alignment.TopCenter).padding(horizontal = Chrome.control + Chrome.gap)) { messages() }
                }
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { unlock() }
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Bottom) {
                    Box(Modifier.touchable(screenLocked)) { tools() }
                    Spacer(Modifier.weight(1f))
                    AudioMeter()
                }
                Box(Modifier.fillMaxWidth().touchable(screenLocked), contentAlignment = Alignment.Center) { liveButton(false) }
            }
        }
    }
    if (model.link == AppModel.Link.NOT_SET_UP && !connectCardDismissed && !showSettings && !screenLocked) {
        ConnectCard(onScan, onSettings = { showSettings = true }, onDismiss = { connectCardDismissed = true },
                    Modifier.align(Alignment.Center).padding(24.dp))
    }
    if (showSettings) SettingsScreen(model, onScan = { showSettings = false; onScan() }) { showSettings = false }
}

/**
 * Lens buttons above the zoom rocker; beside it, on the inner side, where
 * the screen is too low for both. The rocker hides while the screen is locked.
 */
@Composable
private fun ZoomColumn(model: AppModel, rocker: Dp, height: Dp, locked: Boolean, modifier: Modifier) {
    val lenses = model.camera.lenses.size.takeIf { it > 1 } ?: 0
    // Hidden while the screen is locked.
    val hasZoom = model.camera.zoomRange != null && !locked
    val lensHeight = Chrome.control * lenses + 6.dp * (lenses - 1).coerceAtLeast(0)
    val rockerView = @Composable { if (hasZoom) ZoomRocker(rocker) { model.rampZoom(it) } }
    if (lenses == 0 || lensHeight + Chrome.gap + rocker <= height) {
        Column(modifier, verticalArrangement = Arrangement.spacedBy(Chrome.gap)) {
            LensButtons(model)
            rockerView()
        }
    } else {
        Row(modifier, horizontalArrangement = Arrangement.spacedBy(Chrome.gap), verticalAlignment = Alignment.CenterVertically) {
            rockerView()
            LensButtons(model)
        }
    }
}

/** Operator messages and toasts, centred under the values row, over the picture only. */
@Composable
private fun MessageSlot(banner: String?, toast: String?) {
    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(Chrome.rowGap)) {
        banner?.let {
            Text(it, color = Color.Black, fontSize = 20.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center,
                 modifier = Modifier.background(Chrome.yellow, RoundedCornerShape(10.dp))
                     .padding(horizontal = 18.dp, vertical = 10.dp))
        }
        toast?.let {
            Text("⚠  $it", color = Color.White, fontSize = 13.sp, fontWeight = FontWeight.Medium, maxLines = 3,
                 overflow = TextOverflow.Ellipsis,
                 modifier = Modifier.widthIn(max = 460.dp).background(Color.Black.copy(alpha = 0.7f), RoundedCornerShape(18.dp))
                     .padding(horizontal = 14.dp, vertical = 9.dp))
        }
    }
}

@Composable
private fun Preview(model: AppModel, aids: Aids, modifier: Modifier) {
    Box(modifier, contentAlignment = Alignment.Center) {
        Box(Modifier.fillMaxSize().aspectRatio(16f / 9f, matchHeightConstraintsFirst = true)) {
            PreviewView(model, Modifier.fillMaxSize())
            // Over the preview only, never in the stream.
            if (aids.peaking) PeakingOverlay(model)
            FrameGuides(aids)
            if (aids.horizon) HorizonLevel()
        }
    }
}

@Composable
private fun PreviewView(model: AppModel, modifier: Modifier) {
    AndroidView(
        // Tap to focus and expose there, as on iOS (the `focus_at` command).
        modifier = modifier
            .pointerInput(Unit) {
                detectTapGestures(
                    onTap = { p -> model.focusAt(p.x / size.width, p.y / size.height) },
                    // Long press: focus there and lock it (docs/SHADING.md).
                    onLongPress = { p -> model.focusLockAt(p.x / size.width, p.y / size.height) },
                )
            },
        factory = { context ->
            TextureView(context).apply {
                surfaceTextureListener = object : TextureView.SurfaceTextureListener {
                    override fun onSurfaceTextureAvailable(texture: SurfaceTexture, w: Int, h: Int) =
                        model.attachPreview(texture)
                    override fun onSurfaceTextureSizeChanged(texture: SurfaceTexture, w: Int, h: Int) = Unit
                    override fun onSurfaceTextureDestroyed(texture: SurfaceTexture): Boolean {
                        // Freed only once the camera no longer renders into it.
                        model.detachPreview { texture.release() }
                        return false
                    }
                    override fun onSurfaceTextureUpdated(texture: SurfaceTexture) = Unit
                }
            }
        },
    )
}

private fun tallyColor(tally: AppModel.Tally): Color? = when (tally) {
    AppModel.Tally.PROGRAM -> Chrome.red
    AppModel.Tally.PREVIEW -> Chrome.green
    AppModel.Tally.OFF -> null
}

/** The tally border around the screen: green on preview, red on air. Edge only, never over the centre. */
@Composable
private fun TallyOverlay(tally: AppModel.Tally) {
    val color = tallyColor(tally) ?: return
    Box(Modifier.fillMaxSize().border(14.dp, color))
}

/**
 * The camera's name, with PREVIEW or ON AIR in the tally colour: a compact
 * chip in the status line (its own row on a narrow screen), so the person
 * at the camera knows which camera this is without it standing in the
 * picture.
 */
@Composable
private fun TallyChip(tally: AppModel.Tally, name: String) {
    val label = when (tally) {
        AppModel.Tally.PROGRAM -> "ON AIR"
        AppModel.Tally.PREVIEW -> "PREVIEW"
        AppModel.Tally.OFF -> null
    }
    Row(
        Modifier.height(Chrome.rowHeight - 4.dp).background(tallyColor(tally) ?: Color.Black.copy(alpha = 0.55f), CircleShape)
            .padding(horizontal = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        label?.let { Text(it, color = Color.White, fontSize = 15.sp, fontWeight = FontWeight.Black) }
        Text(name, color = Color.White, fontSize = 15.sp, fontWeight = FontWeight.Bold, maxLines = 1,
             overflow = TextOverflow.Ellipsis)
    }
}

/**
 * One compact, low-contrast line of status along the top edge: live state
 * and uptime, bitrate (yellow while adaptive bitrate holds the encoder
 * down), the camera's name with its tally (where the screen is wide
 * enough), hub link, clock and battery. The gear opens Settings.
 */
@Composable
private fun StatusStrip(model: AppModel, nameChip: (@Composable () -> Unit)?, onSettings: () -> Unit) {
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(1000)
            now = System.currentTimeMillis()
        }
    }
    var liveSince by remember { mutableStateOf<Long?>(null) }
    LaunchedEffect(model.streamState) {
        liveSince = if (model.streamState == LiveStream.State.LIVE) liveSince ?: System.currentTimeMillis() else null
    }
    val clockHealth = remember(now) { model.clockSummary()?.health }
    val battery = remember(now) { model.batteryPercent() }
    val dim = Color.White.copy(alpha = 0.85f)
    @Composable
    fun label(text: String, color: Color = dim, weight: FontWeight = FontWeight.Medium) =
        Text(text, color = color, fontSize = 12.sp, fontFamily = FontFamily.Monospace, fontWeight = weight, maxLines = 1,
             softWrap = false)

    Row(
        Modifier.fillMaxWidth().height(Chrome.rowHeight).background(Color.Black.copy(alpha = 0.35f), CircleShape)
            .padding(start = 12.dp, end = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        val dot = when (model.streamState) {
            LiveStream.State.IDLE -> Color.White.copy(alpha = 0.4f)
            LiveStream.State.CONNECTING -> Chrome.orange
            LiveStream.State.LIVE -> Color.Red
        }
        Box(Modifier.size(8.dp).background(dot, CircleShape))
        when (model.streamState) {
            LiveStream.State.IDLE -> label("STANDBY")
            LiveStream.State.CONNECTING -> label("CONNECTING")
            LiveStream.State.LIVE -> {
                label("LIVE", Color.White, FontWeight.Bold)
                liveSince?.let { label(uptime((now - it) / 1000), Color.White) }
                val stats = model.lastStats
                val ceiling = model.settings.bitrateKbps
                val adapting = stats?.link != null && ceiling > 0 && stats.targetKbps in 1 until ceiling
                label(bitrate(model.bitrateKbps), if (adapting) Chrome.yellow else dim)
            }
        }
        Spacer(Modifier.weight(1f))
        nameChip?.let {
            Box(Modifier.weight(3f, fill = false)) { it() }
            Spacer(Modifier.weight(1f))
        }
        val link = when (model.link) {
            AppModel.Link.CONNECTED -> Chrome.green
            AppModel.Link.CONNECTING -> Chrome.orange
            AppModel.Link.NOT_SET_UP -> Color.White.copy(alpha = 0.4f)
        }
        Box(Modifier.size(9.dp).background(link, CircleShape).semantics { contentDescription = "Control hub link" })
        val clock = when (clockHealth) {
            "healthy" -> Chrome.green
            "degraded" -> Chrome.orange
            else -> Color(0xFFFF453A)
        }
        Box(Modifier.size(9.dp).background(clock, CircleShape).semantics { contentDescription = "Clock ${clockHealth ?: "not running"}" })
        battery?.let { label("$it%", if (it < 20) Color(0xFFFF453A) else dim) }
        Box(Modifier.size(width = 36.dp, height = 30.dp).clickable(onClick = onSettings)
                .semantics { contentDescription = "Settings" }, contentAlignment = Alignment.Center) {
            Text("⚙", color = dim, fontSize = 18.sp)
        }
    }
}

private fun uptime(seconds: Long): String {
    val s = seconds.coerceAtLeast(0)
    val h = s / 3600
    val m = s / 60 % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, s % 60) else "%02d:%02d".format(m, s % 60)
}

/** "6.4 Mb/s", "800 kb/s", as on iOS. */
internal fun bitrate(kbps: Int): String = if (kbps >= 1000) "%.1f Mb/s".format(kbps / 1000.0) else "$kbps kb/s"

/**
 * The large round Go Live button, as on iOS: a tap starts the stream;
 * stopping takes a deliberate hold ([HOLD_TO_STOP_MS]), with a ring filling
 * up while held, so a stray touch never ends a live stream. A tap while
 * live shows "Hold to stop". Follows the stream state, also when the hub
 * starts or stops it; a haptic when it goes live or stops.
 */
@Composable
private fun LiveButton(state: LiveStream.State, hintAbove: Boolean, onStart: () -> Unit, onStop: () -> Unit) {
    val haptics = LocalHapticFeedback.current
    var last by remember { mutableStateOf(state) }
    LaunchedEffect(state) {
        if (state == LiveStream.State.LIVE || (last != LiveStream.State.IDLE && state == LiveStream.State.IDLE)) {
            haptics.performHapticFeedback(HapticFeedbackType.LongPress)
        }
        last = state
    }
    val scope = rememberCoroutineScope()
    val ring = remember { Animatable(0f) }
    var hintSerial by remember { mutableIntStateOf(0) }
    var showHint by remember { mutableStateOf(false) }
    LaunchedEffect(hintSerial) {
        if (hintSerial > 0) {
            showHint = true
            delay(2000)
            showHint = false
        }
    }
    val idle = state == LiveStream.State.IDLE
    val fill = when (state) {
        LiveStream.State.IDLE -> Color.Black.copy(alpha = 0.35f)
        LiveStream.State.CONNECTING -> Chrome.orange.copy(alpha = 0.85f)
        LiveStream.State.LIVE -> Color.Red
    }
    val currentState by androidx.compose.runtime.rememberUpdatedState(state)
    Box {
        Box(
            Modifier.size(Chrome.liveButton)
                .semantics { contentDescription = if (idle) "Go live" else "Stop live stream, touch and hold" }
                .pointerInput(Unit) {
                    detectTapGestures(onPress = {
                        if (currentState == LiveStream.State.IDLE) {
                            if (tryAwaitRelease()) onStart()
                            return@detectTapGestures
                        }
                        hintSerial++
                        val fillUp = scope.launch {
                            ring.snapTo(0f)
                            ring.animateTo(1f, tween(HOLD_TO_STOP_MS.toInt(), easing = LinearEasing))
                        }
                        val released = withTimeoutOrNull(HOLD_TO_STOP_MS) { tryAwaitRelease() }
                        fillUp.cancel()
                        scope.launch { ring.snapTo(0f) }
                        if (released == null) onStop()
                    })
                },
            contentAlignment = Alignment.Center,
        ) {
            Canvas(Modifier.fillMaxSize()) {
                drawCircle(fill)
                val w = 4.dp.toPx()
                drawCircle(Color.White.copy(alpha = if (idle) 0.9f else 1f), radius = size.minDimension / 2 - w / 2, style = Stroke(w))
                if (ring.value > 0f) {
                    val r = 6.dp.toPx()
                    drawArc(Color.White, -90f, 360f * ring.value, useCenter = false,
                            topLeft = Offset(3.dp.toPx() + r / 2, 3.dp.toPx() + r / 2),
                            size = androidx.compose.ui.geometry.Size(size.width - 6.dp.toPx() - r, size.height - 6.dp.toPx() - r),
                            style = Stroke(r, cap = StrokeCap.Round))
                }
            }
            when (state) {
                LiveStream.State.IDLE -> Text("GO\nLIVE", color = Color.White, fontSize = 13.sp, fontWeight = FontWeight.Black,
                                              textAlign = TextAlign.Center, lineHeight = 14.sp)
                LiveStream.State.CONNECTING -> CircularProgressIndicator(Modifier.size(22.dp), color = Color.White, strokeWidth = 2.dp)
                LiveStream.State.LIVE -> Box(Modifier.size(22.dp).background(Color.White, RoundedCornerShape(4.dp)))
            }
        }
        if (showHint) {
            // Over the picture, clear of the bottom row and the adjust
            // control: above and left of the button (landscape) or left of
            // it (portrait). Takes no room, so the button never moves.
            val gap = Chrome.gap
            Text("Hold to stop", color = Color.White, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, softWrap = false,
                 modifier = Modifier.align(Alignment.TopStart)
                     .layout { measurable, _ ->
                         val p = measurable.measure(Constraints())
                         layout(0, 0) {
                             if (hintAbove) p.place(-p.width + Chrome.liveButton.roundToPx() / 2, -p.height - gap.roundToPx())
                             else p.place(-p.width - gap.roundToPx(), (Chrome.liveButton.roundToPx() - p.height) / 2)
                         }
                     }
                     .background(Color.Black.copy(alpha = 0.6f), CircleShape)
                     .padding(horizontal = 10.dp, vertical = 6.dp))
        }
    }
}

private const val HOLD_TO_STOP_MS = 600L

/** Shown while no control hub is set up, until dismissed; the same instructions stay in Settings. */
@Composable
private fun ConnectCard(onScan: () -> Unit, onSettings: () -> Unit, onDismiss: () -> Unit, modifier: Modifier) {
    Column(
        modifier.widthIn(max = 320.dp).background(Color(0xFF1C1C1E), RoundedCornerShape(18.dp)).padding(22.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("Not connected", color = Color.White, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
        Text("Scan the QR code in the control hub, here or with the Camera app.", color = Color.White.copy(alpha = 0.8f),
             fontSize = 15.sp, textAlign = TextAlign.Center)
        Button(onClick = onScan, modifier = Modifier.fillMaxWidth().padding(top = 4.dp),
               colors = ButtonDefaults.buttonColors(containerColor = Color.White, contentColor = Color.Black)) {
            Text("Scan QR code")
        }
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            OutlinedButton(onClick = onSettings) { Text("Enter manually", color = Color.White) }
            OutlinedButton(onClick = onDismiss) { Text("Not now", color = Color.White) }
        }
    }
}

/**
 * "Join <hub>?" before a connect link from a QR code takes the camera to
 * another control hub (or the first): the hub and where the stream goes,
 * so a stray code never takes the camera over (D1). A link for the hub in
 * use applies without asking.
 */
@Composable
private fun JoinDialog(link: ProvisioningLink, live: Boolean, onJoin: (ProvisioningLink) -> Unit, onDecline: () -> Unit) {
    val hub = hostLabel(link.controlUrl) ?: link.controlUrl
    AlertDialog(
        onDismissRequest = onDecline,
        title = { Text("Join $hub?") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text("Control hub: $hub")
                Text(link.srtUrl?.let { "Stream to: ${hostLabel(it) ?: it}" } ?: "Stream: unchanged")
                if (live && link.srtUrl != null) {
                    Text("The stream URL applies on the next Go Live", color = Color.White.copy(alpha = 0.7f), fontSize = 13.sp)
                }
            }
        },
        confirmButton = { TextButton(onClick = { onJoin(link) }) { Text("Join") } },
        dismissButton = { TextButton(onClick = onDecline) { Text("Cancel") } },
    )
}

/** The host, and the port when the URL has one. */
private fun hostLabel(url: String): String? = runCatching { android.net.Uri.parse(url) }.getOrNull()
    ?.takeIf { !it.host.isNullOrEmpty() }?.let { if (it.port > 0) "${it.host}:${it.port}" else it.host }
