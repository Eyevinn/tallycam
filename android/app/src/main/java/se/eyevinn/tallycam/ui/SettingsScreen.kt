package se.eyevinn.tallycam.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import se.eyevinn.tallycam.AppModel
import se.eyevinn.tallycam.BuildConfig
import se.eyevinn.tallycam.control.ClockSummary
import se.eyevinn.tallycam.control.ProvisioningLink
import se.eyevinn.tallycam.media.LiveStream

private val Card = Color(0xFF1C1C1E)
private val Secondary = Color(0xFF8E8E93)
private val Green = Color(0xFF34C759)
private val Orange = Color(0xFFFF9F0A)
private val Red = Color(0xFFFF453A)

/**
 * Settings, as on iOS (ios/TallyCam/Views/SettingsView.swift): everything not
 * needed while shooting, in the same sections and words: camera name,
 * control hub, stream, clock, about. docs/APPS.md lists what differs.
 */
@Composable
fun SettingsScreen(model: AppModel, onScan: () -> Unit, onDone: () -> Unit) {
    var name by remember { mutableStateOf(model.cameraNameSetting) }
    var control by remember { mutableStateOf(model.settings.controlUrl) }
    var srt by remember { mutableStateOf(model.settings.srtUrl) }
    var confirmForget by remember { mutableStateOf(false) }
    var tick by remember { mutableStateOf(0) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(1000)
            tick++
        }
    }
    fun done() {
        model.setCameraName(name)
        if (srt != model.settings.srtUrl && model.streamState == LiveStream.State.IDLE) model.setSrtUrl(srt)
        if (control != model.settings.controlUrl) model.setControlUrl(control)
        onDone()
    }
    Box(Modifier.fillMaxSize().background(Color.Black)) {
        Column(Modifier.fillMaxSize()) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                Spacer(Modifier.weight(1f))
                Text("Settings", color = Color.White, fontWeight = FontWeight.SemiBold, fontSize = 17.sp)
                Spacer(Modifier.weight(1f))
                TextButton(onClick = ::done) { Text("Done", color = Color(0xFF0A84FF), fontWeight = FontWeight.SemiBold) }
            }
            Column(
                Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                val width = Modifier.widthIn(max = 640.dp).fillMaxWidth()
                // Camera name.
                Section("Camera name", "How the hub and Strom show this camera. The operator can rename it too; empty uses the model.", width) {
                    Field(name, { name = it }, model.defaultName)
                }
                // Control hub.
                val (linkColor, linkText) = when (model.link) {
                    AppModel.Link.CONNECTED -> Green to "Connected"
                    AppModel.Link.CONNECTING -> Orange to "Reconnecting"
                    AppModel.Link.NOT_SET_UP -> Color.Gray to "Not set up"
                }
                Section("Control hub", "To connect, scan the QR code in the control hub with the phone's camera, or here.", width) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Box(Modifier.size(9.dp).background(linkColor, CircleShape))
                        Text("  $linkText", color = Color.White)
                        Spacer(Modifier.weight(1f))
                        model.host?.let { Text(it, color = Secondary, maxLines = 1) }
                    }
                    Divider()
                    Field(control, { control = it }, "ws://host:8090/lcp/v0/device", url = true,
                          onDone = { model.setControlUrl(control) })
                    Divider()
                    Action("Scan QR code", Color(0xFF0A84FF)) { onScan() }
                    if (model.link != AppModel.Link.NOT_SET_UP) {
                        Divider()
                        Action("Disconnect and forget server", Red) { confirmForget = true }
                    }
                }
                // Stream (SRT).
                val live = model.streamState != LiveStream.State.IDLE
                Section("Stream (SRT)", if (!live) "Set by the QR code. Edit only if you know the address." else "Stop the stream to change the address.", width) {
                    Field(srt, { srt = it }, "srt://host:port?streamid=...", url = true, enabled = !live)
                    Divider()
                    Labeled("Video", model.videoSummary)
                    model.lastStats?.takeIf { model.streamState == LiveStream.State.LIVE }?.let { s ->
                        Divider()
                        Labeled("Round trip", "%.0f ms".format(s.rttMs))
                        Divider()
                        Labeled("Retransmitted / dropped", "${s.retransmits} / ${s.dropped}")
                    }
                }
                // Orientation: Android streams landscape only for now (docs/APPS.md).
                Section("Orientation", "Android streams in landscape for now.", width) {
                    Labeled("Picture", model.pictureSummary)
                }
                // Clock.
                Section("Clock", "The phone's own NTP-disciplined clock, used to timestamp frames.", width) {
                    val clock = remember(tick) { model.clockSummary() }
                    if (clock == null) {
                        Text("Clock not running", color = Secondary)
                    } else {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text("Health", color = Color.White)
                            Spacer(Modifier.weight(1f))
                            val c = when (clock.health) { "healthy" -> Green; "degraded" -> Orange; else -> Red }
                            Box(Modifier.size(9.dp).background(c, CircleShape))
                            Text("  " + clock.health.replaceFirstChar { it.uppercase() }, color = Secondary)
                        }
                        Divider(); Labeled("Synchronised", if (clock.synced) "Yes" else "No")
                        Divider(); Labeled("Estimated error", ClockSummary.formatMicroseconds(clock.estErrorUs))
                        Divider(); Labeled("Maximum error", ClockSummary.formatMicroseconds(clock.maxErrorUs))
                        Divider(); Labeled("Reference", clock.reference ?: "-")
                        clock.stratum?.let { Divider(); Labeled("Stratum", "$it") }
                        if (clock.reachable != null && clock.total != null) {
                            Divider(); Labeled("Sources reachable", "${clock.reachable} of ${clock.total}")
                        }
                        clock.findings.forEach { Text(it, color = Secondary, fontSize = 13.sp, modifier = Modifier.padding(top = 6.dp)) }
                    }
                }
                // About.
                Section("About", null, width) {
                    val id = model.deviceId
                    if (id != null) {
                        Labeled("App version", "${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})")
                        Divider(); Labeled("Camera name", model.displayName)
                        Divider(); Labeled("Model", model.model)
                        Divider(); Labeled("Android", android.os.Build.VERSION.RELEASE)
                        Divider(); Labeled("Stream id", ProvisioningLink.streamId(id))
                        Divider(); Labeled("Device key", model.fingerprint?.take(16)?.chunked(4)?.joinToString(" ") ?: "-", mono = true)
                    } else {
                        Labeled("Device key", "unavailable")
                    }
                }
                Spacer(Modifier.padding(16.dp))
            }
        }
        if (confirmForget) {
            AlertDialog(
                onDismissRequest = { confirmForget = false },
                title = { Text("Forget the control hub?") },
                text = { Text("The phone disconnects from the hub. Scan its QR code to connect again.") },
                confirmButton = {
                    TextButton(onClick = {
                        confirmForget = false
                        model.forgetServer()
                        control = ""
                    }) { Text("Forget server", color = Red) }
                },
                dismissButton = { TextButton(onClick = { confirmForget = false }) { Text("Cancel") } },
            )
        }
    }
}

@Composable
private fun Section(header: String, footer: String?, modifier: Modifier, content: @Composable () -> Unit) {
    Column(modifier.padding(top = 22.dp)) {
        Text(header.uppercase(), color = Secondary, fontSize = 13.sp, modifier = Modifier.padding(start = 16.dp, bottom = 6.dp))
        Column(Modifier.fillMaxWidth().background(Card, RoundedCornerShape(10.dp)).padding(horizontal = 16.dp, vertical = 10.dp),
               verticalArrangement = Arrangement.spacedBy(8.dp)) { content() }
        footer?.let { Text(it, color = Secondary, fontSize = 13.sp, modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 6.dp)) }
    }
}

@Composable
private fun Divider() = Box(Modifier.fillMaxWidth().size(height = 0.5.dp, width = 0.dp).background(Color(0xFF38383A)))

@Composable
private fun Labeled(label: String, value: String, mono: Boolean = false) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(label, color = Color.White)
        Spacer(Modifier.weight(1f))
        Text(value, color = Secondary, maxLines = 1, fontFamily = if (mono) FontFamily.Monospace else FontFamily.Default)
    }
}

@Composable
private fun Action(label: String, color: Color, onClick: () -> Unit) {
    TextButton(onClick = onClick, modifier = Modifier.fillMaxWidth()) {
        Text(label, color = color, modifier = Modifier.fillMaxWidth())
    }
}

@Composable
private fun Field(value: String, onChange: (String) -> Unit, placeholder: String, url: Boolean = false,
                  enabled: Boolean = true, onDone: () -> Unit = {}) {
    TextField(
        value = value, onValueChange = onChange, enabled = enabled, singleLine = true,
        placeholder = { Text(placeholder, color = Secondary) },
        textStyle = androidx.compose.ui.text.TextStyle(
            color = if (enabled) Color.White else Secondary,
            fontFamily = if (url) FontFamily.Monospace else FontFamily.Default, fontSize = if (url) 14.sp else 16.sp,
        ),
        keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, imeAction = ImeAction.Done,
                                          keyboardType = if (url) KeyboardType.Uri else KeyboardType.Text),
        keyboardActions = KeyboardActions(onDone = { onDone() }),
        colors = TextFieldDefaults.colors(focusedContainerColor = Card, unfocusedContainerColor = Card,
                                          disabledContainerColor = Card, focusedIndicatorColor = Color.Transparent,
                                          unfocusedIndicatorColor = Color.Transparent, disabledIndicatorColor = Color.Transparent),
        modifier = Modifier.fillMaxWidth(),
    )
}
