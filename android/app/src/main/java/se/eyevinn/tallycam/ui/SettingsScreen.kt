package se.eyevinn.tallycam.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
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
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInParent
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import org.json.JSONObject
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
 * Settings (docs/UI_DESIGN.md, "Settings"; iOS
 * ios/TallyCam/Views/SettingsView.swift): everything not needed while
 * shooting, in the same sections, order and words: camera name, control
 * hub, stream, exposure, orientation, clock, about. Back is Done.
 * docs/APPS.md lists what differs.
 */
@Composable
fun SettingsScreen(model: AppModel, atPicture: Boolean = false, onScan: () -> Unit, onDone: () -> Unit) {
    val scroll = rememberScrollState()
    // Opened from the format in the values row: scrolled to Picture.
    var pictureTop by remember { mutableStateOf<Int?>(null) }
    LaunchedEffect(pictureTop) { if (atPicture) pictureTop?.let { scroll.scrollTo(it) } }
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
    BackHandler(onBack = ::done)
    Box(Modifier.fillMaxSize().background(Color.Black)) {
        Column(Modifier.fillMaxSize()) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                Spacer(Modifier.weight(1f))
                Text("Settings", color = Color.White, fontWeight = FontWeight.SemiBold, fontSize = 17.sp)
                Spacer(Modifier.weight(1f))
                TextButton(onClick = ::done) { Text("Done", color = Color(0xFF0A84FF), fontWeight = FontWeight.SemiBold) }
            }
            Column(
                Modifier.fillMaxSize().verticalScroll(scroll).padding(horizontal = 16.dp),
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
                val live = model.streamState != LiveStream.State.IDLE
                Section("Control hub", if (!live) "To connect, scan the QR code in the control hub here or with the Camera app."
                                       else "Stop the stream to scan here; the Camera app works while live.", width) {
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
                    Action("Scan QR code", Color(0xFF0A84FF), enabled = !live) { onScan() }
                    if (model.link != AppModel.Link.NOT_SET_UP) {
                        Divider()
                        Action("Disconnect and forget server", Red) { confirmForget = true }
                    }
                }
                // Stream (SRT).
                Section("Stream (SRT)", (if (!live) "Set by the QR code. Edit only if you know the address." else "Stop the stream to change the address.") +
                        " Adaptive bitrate lowers the video bitrate when the network cannot carry it, and raises it again up to the bitrate set.", width) {
                    Field(srt, { srt = it }, "srt://host:port?streamid=...", url = true, enabled = !live)
                    Divider()
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text("Adaptive bitrate", color = Color.White)
                        Spacer(Modifier.weight(1f))
                        androidx.compose.material3.Switch(checked = model.adaptiveOn, onCheckedChange = model::setAdaptiveBitrate)
                    }
                    model.lastStats?.takeIf { model.streamState == LiveStream.State.LIVE }?.let { s ->
                        if (model.adaptiveOn && s.targetKbps > 0) {
                            Divider()
                            Labeled("Encoder now", "${s.targetKbps} kbit/s")
                        }
                        Divider()
                        Labeled("Round trip", fmt("%.0f ms", s.rttMs))
                        Divider()
                        Labeled("Retransmitted / dropped", "${s.retransmits} / ${s.dropped}")
                    }
                }
                // Picture: what the active camera offers, through the same
                // controls as the hub's sets; not while live.
                Box(Modifier.onGloballyPositioned { pictureTop = it.positionInParent().y.toInt() }) {
                    PictureSection(model, live, width)
                }
                SyncSection(model, width)
                // Mains (docs/SHADING.md, "Shutter"): the phone and the hub
                // both set it, the last write wins; not while live.
                val mainsOptions = model.mainsOptions
                if (mainsOptions.size > 1) {
                    Section("Exposure", when {
                        live -> "Stop the stream to change."
                        model.hubProduction -> "The control hub sets this for all cameras; a change here applies to this camera until the hub changes it."
                        else -> "The shutter follows the mains frequency so mains-lit light does not flicker. Off outdoors, where the shutter is free."
                    }, width) {
                        // The mains in use: the phone's, or the hub production's.
                        Segmented(mainsOptions.map { it to if (it == 0) "Off" else "$it Hz" },
                                  model.shading.values.optInt("mains_hz", model.ownMainsHz), enabled = !live) { model.setOwnMains(it) }
                    }
                }
                // Orientation: Android streams landscape only for now (docs/APPS.md).
                Section("Orientation", "Android streams in landscape for now.", width) {
                    Labeled("Orientation", "Landscape")
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

/** One choice of a few, as iOS's segmented picker. */
@Composable
private fun <T> Segmented(options: List<Pair<T, String>>, selected: T, enabled: Boolean, onPick: (T) -> Unit) {
    Row(Modifier.fillMaxWidth().background(Color(0xFF2C2C2E), RoundedCornerShape(8.dp)).padding(2.dp)) {
        for ((value, label) in options) {
            val on = value == selected
            Box(
                Modifier.weight(1f).background(if (on) Color(0xFF636366) else Color.Transparent, RoundedCornerShape(6.dp))
                    .clickable(enabled = enabled && !on) { onPick(value) }.padding(vertical = 6.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(label, color = if (enabled) Color.White else Secondary, fontSize = 14.sp,
                     fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal)
            }
        }
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
private fun Action(label: String, color: Color, enabled: Boolean = true, onClick: () -> Unit) {
    TextButton(onClick = onClick, enabled = enabled, modifier = Modifier.fillMaxWidth()) {
        Text(label, color = if (enabled) color else Secondary, modifier = Modifier.fillMaxWidth())
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

/** A control's options from its descriptor: value and label. */
private fun options(descriptor: JSONObject?): List<Pair<Any, String>> {
    val a = descriptor?.optJSONArray("options") ?: return emptyList()
    return (0 until a.length()).mapNotNull { a.optJSONObject(it) }.map { it.get("value") to it.optString("label") }
}

/**
 * Picture (docs/UI_DESIGN.md, "Settings"): lens, resolution and frame rate
 * (what the active camera offers: the `resolution` and `frame_rate`
 * controls' lists), video codec and bitrate. All through the hub's control
 * table (AppModel.localSet), disabled while live.
 */
@Composable
private fun PictureSection(model: AppModel, live: Boolean, width: Modifier) {
    val v = model.shading.values
    val d = model.shading.descriptors
    Section("Picture", if (live) "Stop the stream to change." else null, width) {
        val lenses = model.camera.lenses.sortedBy { it.focalLength }
        if (lenses.size > 1) {
            Labeled("Lens", "")
            Segmented(lenses.map { it.key to lensLabel(model, it) }, v.optString("lens"), !live) { model.selectLens(it) }
            Divider()
        }
        options(d["resolution"]).takeIf { it.size > 1 }?.let { opts ->
            Labeled("Resolution", "")
            Segmented(opts, v.opt("resolution") ?: "", !live) { model.localSet(JSONObject().put("resolution", it)) }
            Divider()
        }
        options(d["frame_rate"]).takeIf { it.size > 1 }?.let { opts ->
            Labeled("Frame rate", "")
            // Numbers compare as numbers (30 and 30.0).
            val current = opts.firstOrNull { (it.first as? Number)?.toDouble() == v.optDouble("frame_rate") }?.first ?: ""
            Segmented(opts, current, !live) { model.localSet(JSONObject().put("frame_rate", it)) }
            Divider()
        }
        if (model.videoCodecs.size > 1) {
            Labeled("Video codec", "")
            Segmented(model.videoCodecs, model.settings.videoCodec, !live) { model.localSet(JSONObject().put("video_codec", it)) }
            Divider()
        }
        // Bitrate in 500 kbit/s steps, within the control's range.
        val range = AppModel.BITRATE_KBPS
        val kbps = v.optInt("bitrate_kbps", 0)
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("Bitrate", color = Color.White)
            Spacer(Modifier.weight(1f))
            TextButton(onClick = { model.localSet(JSONObject().put("bitrate_kbps", (kbps - 500).coerceAtLeast(range.first))) },
                       enabled = !live && kbps > range.first) { Text("−", fontSize = 20.sp) }
            Text("$kbps kbit/s" + if (model.settings.bitrateKbps <= 0) " (auto)" else "", color = Secondary, style = Chrome.tabular)
            TextButton(onClick = { model.localSet(JSONObject().put("bitrate_kbps", (kbps + 500).coerceAtMost(range.last))) },
                       enabled = !live && kbps < range.last) { Text("+", fontSize = 20.sp) }
        }
        Divider()
        Labeled("Picture", model.pictureSummary)
    }
}

/**
 * Sync (docs/UI_DESIGN.md, "Settings"): genlock with its state (Locked,
 * Locking, Off; the phase error when locked) and stabilization, both
 * through the hub's control table. Left out where the camera has neither.
 */
@Composable
private fun SyncSection(model: AppModel, width: Modifier) {
    val v = model.shading.values
    val d = model.shading.descriptors
    if ("genlock" !in d && "stabilization" !in d) return
    // Refreshed with the screen's tick (the parent recomposes each second).
    val state = model.genlockState()
    Section("Sync", "Genlock aligns frames with other cameras on the same clock. Stabilization off for multi-camera sync tests.", width) {
        if ("genlock" in d) {
            val on = v.optBoolean("genlock")
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text("Genlock", color = Color.White)
                    val text = when {
                        !on -> "Off"
                        state?.first == "locked" -> "Locked, " + fmt("%.2f ms", state.second)
                        else -> "Locking"
                    }
                    Text(text, color = if (on && state?.first == "locked") Chrome.green else Secondary, fontSize = 13.sp)
                }
                androidx.compose.material3.Switch(checked = on, onCheckedChange = { model.localSet(JSONObject().put("genlock", it)) })
            }
        }
        if ("stabilization" in d) {
            if ("genlock" in d) Divider()
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Stabilization", color = Color.White)
                Spacer(Modifier.weight(1f))
                androidx.compose.material3.Switch(checked = v.optString("stabilization") == "standard",
                    onCheckedChange = { model.localSet(JSONObject().put("stabilization", if (it) "standard" else "off")) })
            }
        }
    }
}
