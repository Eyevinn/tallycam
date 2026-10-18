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
import androidx.compose.foundation.layout.width
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
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.findRootCoordinates
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
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
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.foundation.layout.offset
import kotlin.math.roundToInt
import se.eyevinn.tallycam.AppModel
import se.eyevinn.tallycam.control.ProvisioningLink
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
    // Opened from the format in the values row: at Picture.
    var settingsAtPicture by remember { mutableStateOf(false) }
    var connectCardDismissed by remember { mutableStateOf(false) }
    // Touches blocked except the slide to unlock; not persisted.
    var screenLocked by remember { mutableStateOf(false) }
    // What each side control runs (docs/UI_DESIGN.md, "Side control"):
    // zoom on the left and exposure on the right until changed, each side
    // remembered across launches.
    val prefs = remember { context.getSharedPreferences("chrome", android.content.Context.MODE_PRIVATE) }
    var sides by remember {
        mutableStateOf(Sides(prefs.getString("left", null) ?: Sides.DEFAULT.left,
                             prefs.getString("right", null) ?: Sides.DEFAULT.right,
                             prefs.getBoolean("leftHidden", false), prefs.getBoolean("rightHidden", false)))
    }
    // The groups this camera has, in the selector's order.
    val groups = SIDE_GROUPS.filter {
        if (it == "zoom") model.camera.zoomRange != null || model.camera.lenses.size > 1 else modeKey(it) in model.shading.descriptors
    }
    val shown = sides.resolved(groups)
    val left = shown.left
    val right = shown.right
    fun place(next: Sides) {
        sides = next
        prefs.edit().putString("left", next.left).putString("right", next.right)
            .putBoolean("leftHidden", next.leftHidden).putBoolean("rightHidden", next.rightHidden).apply()
    }
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
    val tools = @Composable { vertical: Boolean ->
        // Aids and screen lock; the QR scanner too while no hub is connected and not live.
        val buttons = @Composable {
            AidsMenu(aids, model)
            if (model.link != AppModel.Link.CONNECTED && idle) {
                Box(Modifier.semantics { contentDescription = "Scan QR code" }) { ChromeButton("⌗", onClick = onScan) }
            }
            Box(Modifier.semantics { contentDescription = "Lock screen" }) {
                ChromeButton("🔒") { screenLocked = true }
            }
        }
        if (vertical) Column(verticalArrangement = Arrangement.spacedBy(Chrome.rowGap)) { buttons() }
        else Row(horizontalArrangement = Arrangement.spacedBy(Chrome.gap)) { buttons() }
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
        // A phone: the side controls in the bottom corners (a tablet keeps the bottom row).
        val phone = androidx.compose.ui.platform.LocalConfiguration.current.smallestScreenWidthDp < 600
        // Narrower than this (portrait), the camera's name gets a row of its own.
        // Landscape: one top bar (status, name and values); portrait: the
        // status row over the values, the name on its own row when narrow.
        val nameInline = maxWidth >= 560.dp
        val rows = if (landscape) 1 else if (nameInline) 2 else 3
        val top = Chrome.rowHeight * rows + Chrome.rowGap * (rows - 1)
        val nameChip = @Composable { TallyChip(model.tally, model.displayName) }
        val side = @Composable { leftSide: Boolean ->
            (if (leftSide) left else right)?.let { group ->
                val hidden = if (leftSide) shown.leftHidden else shown.rightHidden
                val readoutAbove = phone && !landscape
                // The tab just above the rocker at the screen edge, at one fixed
                // place: a tap hides the side control, or brings it back
                // (docs/UI_DESIGN.md, "Hide and show: one tab").
                Column(horizontalAlignment = if (leftSide) Alignment.Start else Alignment.End,
                       verticalArrangement = Arrangement.spacedBy(Chrome.gap)) {
                    Box(Modifier.padding(horizontal = (Chrome.rockerWidth - Chrome.control) / 2)) {
                        EdgeTab(group, leftSide, hidden, Modifier.touchable(screenLocked)) {
                            place(if (hidden) shown.show(leftSide) else shown.hide(leftSide))
                        }
                    }
                    if (hidden) {
                        // Its place kept, so the tab does not move.
                        Box(Modifier.height(SIDE_HEIGHT + if (readoutAbove) Chrome.valueHeight + Chrome.gap else 0.dp))
                    } else {
                        SideControl(model, group, groups, leading = leftSide, readoutAbove = readoutAbove, locked = screenLocked,
                                    onSelect = { place(shown.choose(leftSide, it)) }, modifier = Modifier.touchable(screenLocked))
                    }
                }
            }
        }
        val messages = @Composable { MessageSlot(model.banner, toast) }
        val unlock = @Composable { if (screenLocked) SlideToUnlock({ screenLocked = false }) }

        Column(verticalArrangement = Arrangement.spacedBy(Chrome.zoneGap)) {
            Column(Modifier.fillMaxWidth().touchable(screenLocked), horizontalAlignment = Alignment.CenterHorizontally,
                   verticalArrangement = Arrangement.spacedBy(Chrome.rowGap)) {
                val values = @Composable { dropped: Int, bare: Boolean ->
                    // Underlined: the groups a shown side runs.
                    CameraValuesRow(model, listOfNotNull(left.takeIf { !shown.leftHidden }, right.takeIf { !shown.rightHidden }),
                                    { place(shown.fromValues(it)) },
                                    onFormat = { settingsAtPicture = true; showSettings = true }, bare = bare, dropped = dropped)
                }
                if (landscape) {
                    StatusStrip(model, centre = {
                        TopBarCentre({ TallyChip(model.tally, model.displayName) },
                                     { TallyChip(model.tally, model.displayName, short = true) }) { values(it, true) }
                    }) { showSettings = true }
                } else {
                    StatusStrip(model, if (nameInline) nameChip else null) { showSettings = true }
                    if (!nameInline) nameChip()
                    values(0, false)
                }
            }
            if (phone) {
                // A phone (docs/UI_DESIGN.md, zones 4 to 6): the side controls
                // in the bottom corners against the bottom edge, the tools in
                // the upper-left corner under the top rows (a column in
                // landscape, a row in portrait), the live button in the
                // upper-right corner with the meter beside it; messages and the
                // slide to unlock in the middle.
                Box(Modifier.weight(1f).fillMaxWidth()) {
                    // A row in both orientations: in landscape the left tab and side
                    // control leave no room for a column (docs/UI_DESIGN.md).
                    Box(Modifier.align(Alignment.TopStart).touchable(screenLocked)) { tools(false) }
                    Row(Modifier.align(Alignment.TopEnd), horizontalArrangement = Arrangement.spacedBy(Chrome.gap),
                        verticalAlignment = Alignment.CenterVertically) {
                        AudioMeter()
                        Box(Modifier.touchable(screenLocked)) { liveButton(false) }
                    }
                    Box(Modifier.align(Alignment.TopCenter).padding(horizontal = Chrome.liveButton + Chrome.gap)) { messages() }
                    Box(Modifier.align(Alignment.BottomStart)) { side(true) }
                    Box(Modifier.align(Alignment.BottomEnd)) { side(false) }
                    // The slide to unlock (only while locked) at the bottom edge between
                    // the side controls; a portrait phone has no room there, so above them.
                    Box(Modifier.align(Alignment.BottomCenter).padding(bottom = if (landscape) 0.dp else SIDE_HEIGHT + Chrome.zoneGap)) {
                        unlock()
                    }
                }
            } else if (landscape) {
                // Left: zoom over the tools. Centre: messages, the slide to
                // unlock, the meter. Right: the adjust control over the live
                // button. The bottom row is the live button's height, so both
                // edge columns centre on the same line.
                Row(Modifier.weight(1f), horizontalArrangement = Arrangement.spacedBy(Chrome.gap)) {
                    Column(Modifier.fillMaxHeight(), verticalArrangement = Arrangement.spacedBy(Chrome.zoneGap)) {
                        SideSlot(leading = true, Modifier.weight(1f)) { side(true) }
                        Box(Modifier.height(Chrome.liveButton).touchable(screenLocked), contentAlignment = Alignment.BottomStart) {
                            tools(false)
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
                        SideSlot(leading = false, Modifier.weight(1f)) { side(false) }
                        Box(Modifier.touchable(screenLocked)) { liveButton(true) }
                    }
                }
            } else {
                // Mirror images, the two rockers level; messages between them.
                Box(Modifier.weight(1f).fillMaxWidth()) {
                    SideSlot(leading = true, Modifier.fillMaxSize()) { side(true) }
                    SideSlot(leading = false, Modifier.fillMaxSize()) { side(false) }
                    Box(Modifier.align(Alignment.TopCenter).padding(horizontal = Chrome.control + Chrome.gap)) { messages() }
                }
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { unlock() }
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Bottom) {
                    Box(Modifier.touchable(screenLocked)) { tools(false) }
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
    if (showSettings) {
        SettingsScreen(model, atPicture = settingsAtPicture, onScan = { showSettings = false; onScan() }) {
            showSettings = false
            settingsAtPicture = false
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
    // Where the last tap or hold was, for the focus square (as iOS).
    var mark by remember { mutableStateOf<FocusMark?>(null) }
    Box(modifier, contentAlignment = Alignment.Center) {
        Box(Modifier.fillMaxSize().aspectRatio(16f / 9f, matchHeightConstraintsFirst = true)) {
            PreviewView(model, Modifier.fillMaxSize()) { mark = it }
            // Over the preview only, never in the stream.
            if (aids.peaking) PeakingOverlay(model)
            FrameGuides(aids)
            if (aids.horizon) HorizonLevel()
            mark?.let { FocusSquare(it) { if (mark === it) mark = null } }
        }
    }
}

/** A tap (focus there) or a hold (AE/AF lock there) on the picture, in pixels of the preview. */
private class FocusMark(val at: Offset, val lock: String?)

/**
 * The yellow square where the picture was touched: 72 dp, springing in
 * from 1.35×, gone after 1.2 s; a hold labels it "AE/AF LOCK" (or "AF LOCK"
 * where exposure is not held), as on iOS.
 */
@Composable
private fun FocusSquare(mark: FocusMark, onDone: () -> Unit) {
    val scale = remember(mark) { Animatable(1.35f) }
    LaunchedEffect(mark) {
        scale.animateTo(1f, androidx.compose.animation.core.spring(dampingRatio = 0.6f))
        delay(1200)
        onDone()
    }
    val side = 72.dp
    Box(Modifier.layout { measurable, constraints ->
        val p = measurable.measure(Constraints.fixed(side.roundToPx(), side.roundToPx()))
        layout(constraints.maxWidth, constraints.maxHeight) {
            p.place((mark.at.x - p.width / 2).roundToInt(), (mark.at.y - p.height / 2).roundToInt())
        }
    }) {
        Box(Modifier.fillMaxSize().graphicsLayer { scaleX = scale.value; scaleY = scale.value }
                .border(1.5.dp, Chrome.yellow, RoundedCornerShape(2.dp)))
        mark.lock?.let {
            Text(it, color = Color.Black, fontSize = 10.sp, fontWeight = FontWeight.Bold, softWrap = false,
                 modifier = Modifier.align(Alignment.TopCenter).offset(y = (-20).dp)
                     .background(Chrome.yellow, RoundedCornerShape(3.dp)).padding(horizontal = 4.dp, vertical = 1.dp))
        }
    }
}

/**
 * The camera's picture and its gestures, as on iOS: a tap focuses and
 * exposes there (and releases an AE/AF lock); a touch held 0.55 s locks
 * focus and exposure there; a pinch zooms from where it started.
 */
@Composable
private fun PreviewView(model: AppModel, modifier: Modifier, onMark: (FocusMark) -> Unit) {
    val haptics = rememberHaptics()
    val currentModel by androidx.compose.runtime.rememberUpdatedState(model)
    AndroidView(
        modifier = modifier
            .pointerInput(Unit) {
                val slop = 12.dp.toPx()
                awaitEachGesture {
                    val down = awaitFirstDown()
                    // What the touch turns into before the hold time is up.
                    var outcome = "hold"
                    withTimeoutOrNull(HOLD_MS) {
                        while (true) {
                            val event = awaitPointerEvent()
                            if (event.changes.count { it.pressed } >= 2) { outcome = "pinch"; break }
                            val change = event.changes.firstOrNull { it.id == down.id } ?: break
                            if (!change.pressed) { outcome = "tap"; break }
                            if ((change.position - down.position).getDistance() > slop) { outcome = "move"; break }
                        }
                    }
                    val m = currentModel
                    val x = down.position.x / size.width
                    val y = down.position.y / size.height
                    when (outcome) {
                        "tap" -> {
                            m.focusAt(x, y)
                            onMark(FocusMark(down.position, null))
                        }
                        "hold" -> {
                            val exposure = m.shading.descriptors.containsKey("exposure_mode") &&
                                m.shading.values.optString("exposure_mode") == "auto"
                            m.focusLockAt(x, y)
                            haptics.step()
                            onMark(FocusMark(down.position, if (exposure) "AE/AF LOCK" else "AF LOCK"))
                        }
                        "pinch" -> pinchZoom(m)
                    }
                    // The rest of the gesture is spent.
                    while (true) {
                        val event = awaitPointerEvent()
                        if (event.changes.none { it.pressed }) break
                    }
                }
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
private fun TallyChip(tally: AppModel.Tally, name: String, short: Boolean = false) {
    // Short: the name alone (the tally border still shows tally).
    val label = when (tally) {
        AppModel.Tally.PROGRAM -> "ON AIR"
        AppModel.Tally.PREVIEW -> "PREVIEW"
        AppModel.Tally.OFF -> null
    }?.takeIf { !short }
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
private fun StatusStrip(model: AppModel, centre: (@Composable () -> Unit)?, onSettings: () -> Unit) {
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
        Text(text, color = color, fontSize = 12.sp, style = Chrome.tabular, fontWeight = weight, maxLines = 1,
             softWrap = false)

    Row(
        Modifier.fillMaxWidth().height(Chrome.rowHeight).background(Chrome.surface, CircleShape).border(1.dp, Chrome.border, CircleShape)
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
        centre?.let {
            Box(Modifier.weight(6f, fill = false)) { it() }
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

internal fun uptime(seconds: Long): String {
    val s = seconds.coerceAtLeast(0)
    val h = s / 3600
    val m = s / 60 % 60
    return if (h > 0) fmt("%d:%02d:%02d", h, m, s % 60) else fmt("%02d:%02d", m, s % 60)
}

/** "6.4 Mb/s", "800 kb/s", as on iOS. */
internal fun bitrate(kbps: Int): String = if (kbps >= 1000) fmt("%.1f Mb/s", kbps / 1000.0) else "$kbps kb/s"

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
        LiveStream.State.IDLE -> Chrome.surface
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
/** A touch on the picture held this long is an AE/AF lock (as iOS). */
private const val HOLD_MS = 550L

/**
 * A pinch on the picture: the zoom at its start times the spread, sent at
 * most every 50 ms and rounded to 0.01 (as iOS).
 */
private suspend fun androidx.compose.ui.input.pointer.AwaitPointerEventScope.pinchZoom(model: AppModel) {
    val d = model.shading.descriptors["zoom"] ?: return
    val low = d.optDouble("min", 1.0)
    val high = d.optDouble("max", 1.0)
    val start = model.shading.values.optDouble("zoom", 1.0)
    var startSpan = 0f
    var sentAt = 0L
    while (true) {
        val event = awaitPointerEvent()
        val pressed = event.changes.filter { it.pressed }
        if (pressed.size < 2) break
        val span = (pressed[0].position - pressed[1].position).getDistance()
        if (startSpan == 0f) { startSpan = span; continue }
        pressed.forEach { it.consume() }
        val now = event.changes.first().uptimeMillis
        if (now - sentAt < 50) continue
        sentAt = now
        val zoom = (Math.round(start * span / startSpan * 100) / 100.0).coerceIn(low, high)
        model.localSet(org.json.JSONObject().put("zoom", zoom), quiet = true)
    }
}

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

/**
 * The tab above a side control's rocker at the screen edge, in its group's
 * colour: a chevron pointing out while the control shows (a tap hides it),
 * inward while hidden (a tap brings it back); one fixed place
 * (docs/UI_DESIGN.md, "Hide and show: one tab").
 */
@Composable
private fun EdgeTab(group: String, leading: Boolean, hidden: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val shape = RoundedCornerShape(Chrome.cellRadius)
    val side = if (leading) "left" else "right"
    Box(
        modifier.size(Chrome.control).background(Chrome.chosen(Chrome.groupColor(group)), shape)
            .border(1.dp, Chrome.border, shape)
            .clickable(onClick = onClick)
            .semantics { contentDescription = if (hidden) "Show the $side control" else "Hide the $side control" },
        contentAlignment = Alignment.Center,
    ) {
        // Out towards the edge while shown (it folds away), inward while hidden (it comes out).
        val outward = if (leading) "‹" else "›"
        val inward = if (leading) "›" else "‹"
        Text(if (hidden) inward else outward, color = Color.White, fontSize = 26.sp, fontWeight = FontWeight.Bold, style = Chrome.shadowed)
    }
}

/**
 * The centre of the landscape top bar: the name chip and the values. Where
 * they do not fit, the values drop focus, then white balance, then the
 * chip shortens to the name alone (docs/UI_DESIGN.md, "Landscape: one top bar").
 */
@Composable
private fun TopBarCentre(chip: @Composable () -> Unit, shortChip: @Composable () -> Unit, values: @Composable (Int) -> Unit) {
    Layout({ chip(); shortChip(); values(0); values(1); values(2) }) { measurables, constraints ->
        val loose = constraints.copy(minWidth = 0, minHeight = 0)
        val gap = Chrome.gap.roundToPx()
        val p = measurables.map { it.measure(loose.copy(maxWidth = androidx.compose.ui.unit.Constraints.Infinity)) }
        // In order: the full chip with all values, without focus, without white balance, then the short chip.
        val choices = listOf(0 to 2, 0 to 3, 0 to 4, 1 to 4)
        val (c, v) = choices.firstOrNull { (c, v) -> p[c].width + gap + p[v].width <= constraints.maxWidth } ?: choices.last()
        val height = maxOf(p[c].height, p[v].height)
        val width = minOf(constraints.maxWidth, p[c].width + gap + p[v].width)
        layout(width, height) {
            p[c].placeRelative(0, (height - p[c].height) / 2)
            p[v].placeRelative(p[c].width + gap, (height - p[v].height) / 2)
        }
    }
}

/**
 * Where a side control sits in its zone (docs/UI_DESIGN.md, "Zones"): its
 * centre at 65 % of the screen's height from the top, moved up only as far
 * as it must to clear the bottom row (and down to clear the top rows);
 * both sides the same.
 */
@Composable
private fun SideSlot(leading: Boolean, modifier: Modifier, content: @Composable () -> Unit) {
    // The zone's top in the window and the window's height, once laid out.
    var place by remember { mutableStateOf<Pair<Float, Float>?>(null) }
    Layout(content, modifier.onGloballyPositioned {
        place = it.positionInRoot().y to it.findRootCoordinates().size.height.toFloat()
    }) { measurables, constraints ->
        val p = measurables.map { it.measure(constraints.copy(minWidth = 0, minHeight = 0)) }
        // As wide as the control (or the zone, where it fills), as tall as the zone.
        val w = maxOf(constraints.minWidth, p.maxOfOrNull { it.width } ?: 0)
        val h = constraints.maxHeight
        layout(w, h) {
            p.forEach { child ->
                val centre = place?.let { (top, root) -> root * SIDE_CENTRE - top } ?: (h / 2f)
                val y = (centre - child.height / 2f).coerceIn(0f, (h - child.height).coerceAtLeast(0).toFloat())
                child.placeRelative(if (leading) 0 else w - child.width, y.roundToInt())
            }
        }
    }
}

/** A side control's centre, as a share of the screen's height from the top. */
private const val SIDE_CENTRE = 0.65f
