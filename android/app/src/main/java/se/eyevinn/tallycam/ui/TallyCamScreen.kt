package se.eyevinn.tallycam.ui

import android.graphics.SurfaceTexture
import android.view.TextureView
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.input.pointer.pointerInput
import se.eyevinn.tallycam.AppModel
import se.eyevinn.tallycam.media.LiveStream

/**
 * The shooting screen, as on iOS: full-bleed preview, the tally border with
 * the camera's name in a chip, one status line on top, the live button on
 * the right edge. Everything else is in Settings.
 */
@Composable
fun TallyCamScreen(model: AppModel?, permitted: Boolean, onScan: () -> Unit) {
    MaterialTheme(colorScheme = darkColorScheme()) {
        Box(Modifier.fillMaxSize().background(Color.Black)) {
            if (model == null) {
                Column(Modifier.align(Alignment.Center).padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Text(if (permitted) "Starting…" else "TallyCam needs the camera and the microphone.", color = Color.White)
                    if (!permitted) {
                        val context = androidx.compose.ui.platform.LocalContext.current
                        TextButton(onClick = {
                            context.startActivity(android.content.Intent(
                                android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                                android.net.Uri.fromParts("package", context.packageName, null)))
                        }) { Text("Open settings") }
                    }
                }
                return@Box
            }
            var showSettings by remember { mutableStateOf(false) }
            Preview(model, Modifier.align(Alignment.Center))
            TallyOverlay(model)
            Column(Modifier.align(Alignment.TopCenter).padding(top = 12.dp, start = 16.dp, end = 16.dp),
                   verticalArrangement = Arrangement.spacedBy(6.dp)) {
                StatusStrip(model, onSettings = { showSettings = true }, modifier = Modifier)
                // Who owns exposure, colour and focus, as on iOS.
                GroupChips(model, Modifier.padding(start = 4.dp))
            }
            LiveButton(model, Modifier.align(Alignment.CenterEnd).padding(end = 28.dp))
            if (model.link == AppModel.Link.NOT_SET_UP && !showSettings) {
                ConnectCard(onScan, Modifier.align(Alignment.Center))
            }
            if (showSettings) SettingsScreen(model, onScan = { showSettings = false; onScan() }) { showSettings = false }
        }
    }
}

@Composable
private fun Preview(model: AppModel, modifier: Modifier) {
    AndroidView(
        // Tap to focus and expose there, as on iOS (the `focus_at` command).
        modifier = modifier.fillMaxSize().aspectRatio(16f / 9f, matchHeightConstraintsFirst = true)
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

@Composable
private fun TallyOverlay(model: AppModel) {
    val color = when (model.tally) {
        AppModel.Tally.PROGRAM -> Color(0xFFE5322D)
        AppModel.Tally.PREVIEW -> Color(0xFF34C759)
        AppModel.Tally.OFF -> null
    }
    val label = when (model.tally) {
        AppModel.Tally.PROGRAM -> "ON AIR"
        AppModel.Tally.PREVIEW -> "PREVIEW"
        AppModel.Tally.OFF -> null
    }
    Box(Modifier.fillMaxSize()) {
        if (color != null) Box(Modifier.fillMaxSize().border(14.dp, color))
        Column(Modifier.align(Alignment.TopCenter).padding(top = 84.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            // The camera's name, always large: the person at the camera
            // knows which camera they are.
            Row(
                Modifier.background(color ?: Color.Black.copy(alpha = 0.45f), CircleShape)
                    .padding(horizontal = 18.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (label != null) {
                    Text(label, color = Color.White, fontSize = 20.sp, fontWeight = FontWeight.Black)
                    Text("  ·  ", color = Color.White, fontSize = 20.sp)
                }
                Text(model.displayName, color = Color.White, fontSize = 24.sp, fontWeight = FontWeight.Bold,
                     maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            model.banner?.let {
                Spacer(Modifier.height(8.dp))
                Text(it, color = Color.Black, fontSize = 20.sp, fontWeight = FontWeight.SemiBold,
                     modifier = Modifier.background(Color(0xFFFFD60A), RoundedCornerShape(10.dp))
                         .padding(horizontal = 18.dp, vertical = 10.dp))
            }
        }
    }
}

@Composable
private fun StatusStrip(model: AppModel, onSettings: () -> Unit, modifier: Modifier) {
    val (text, dot) = when (model.streamState) {
        LiveStream.State.IDLE -> "STANDBY" to Color.White.copy(alpha = 0.4f)
        LiveStream.State.CONNECTING -> "CONNECTING" to Color(0xFFFF9F0A)
        LiveStream.State.LIVE -> "LIVE  ${model.bitrateKbps / 1000.0} Mbit/s" to Color(0xFFE5322D)
    }
    val linkColor = when (model.link) {
        AppModel.Link.CONNECTED -> Color(0xFF34C759)
        AppModel.Link.CONNECTING -> Color(0xFFFF9F0A)
        AppModel.Link.NOT_SET_UP -> Color.Gray
    }
    Row(
        modifier.fillMaxWidth().height(32.dp).background(Color.Black.copy(alpha = 0.35f), CircleShape)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Box(Modifier.size(8.dp).background(dot, CircleShape))
        Text(text, color = Color.White, fontSize = 13.sp, fontWeight = FontWeight.Medium)
        model.streamMessage?.let {
            Text(it, color = Color(0xFFFF9F0A), fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
                 modifier = Modifier.weight(1f, fill = false))
        }
        Spacer(Modifier.weight(1f))
        // As iOS: link, clock and battery, refreshed every second.
        var tick by remember { mutableStateOf(0) }
        androidx.compose.runtime.LaunchedEffect(Unit) {
            while (true) {
                kotlinx.coroutines.delay(1000)
                tick++
            }
        }
        val clockHealth = remember(tick) { model.clockSummary()?.health }
        val battery = remember(tick) { model.batteryPercent() }
        Box(Modifier.size(9.dp).background(linkColor, CircleShape))
        Box(Modifier.size(9.dp).background(when (clockHealth) {
            "healthy" -> Color(0xFF34C759)
            "degraded" -> Color(0xFFFF9F0A)
            null -> Color.Gray
            else -> Color(0xFFFF453A)
        }, CircleShape))
        battery?.let { Text("$it%", color = Color.White, fontSize = 13.sp) }
        Text("⚙", color = Color.White, fontSize = 18.sp, modifier = Modifier.clickable(onClick = onSettings).padding(4.dp))
    }
}

@Composable
private fun LiveButton(model: AppModel, modifier: Modifier) {
    val live = model.streamState != LiveStream.State.IDLE
    // As iOS: a haptic when the stream goes live or stops; orange while connecting.
    val haptics = androidx.compose.ui.platform.LocalHapticFeedback.current
    var wasLive by remember { mutableStateOf(model.streamState == LiveStream.State.LIVE) }
    androidx.compose.runtime.LaunchedEffect(model.streamState) {
        val isLive = model.streamState == LiveStream.State.LIVE
        if (isLive != wasLive && model.streamState != LiveStream.State.CONNECTING) {
            haptics.performHapticFeedback(androidx.compose.ui.hapticfeedback.HapticFeedbackType.LongPress)
        }
        if (model.streamState != LiveStream.State.CONNECTING) wasLive = isLive
    }
    val fill = when (model.streamState) {
        LiveStream.State.IDLE -> Color.Black.copy(alpha = 0.35f)
        LiveStream.State.CONNECTING -> Color(0xFFFF9F0A).copy(alpha = 0.85f)
        LiveStream.State.LIVE -> Color(0xFFE5322D)
    }
    Box(
        modifier.size(76.dp).border(4.dp, Color.White, CircleShape).padding(8.dp)
            .background(fill, CircleShape)
            .clickable { model.toggleLive() },
        contentAlignment = Alignment.Center,
    ) {
        Text(if (live) "STOP" else "LIVE", color = Color.White, fontWeight = FontWeight.Bold, fontSize = 13.sp)
    }
}

@Composable
private fun ConnectCard(onScan: () -> Unit, modifier: Modifier) {
    Column(
        modifier.background(Color.Black.copy(alpha = 0.7f), RoundedCornerShape(16.dp)).padding(24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text("Not connected", color = Color.White, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Text("Scan the QR code in the control hub, with the phone's camera or here.", color = Color.White,
             fontSize = 14.sp, modifier = Modifier.width(300.dp))
        TextButton(onClick = onScan) { Text("Scan QR code") }
    }
}
