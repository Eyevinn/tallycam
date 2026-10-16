package se.eyevinn.tallycam

import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.graphics.SurfaceTexture
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.BatteryManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.util.Base64
import android.util.Log
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import org.json.JSONObject
import se.eyevinn.tallycam.control.CameraControls
import se.eyevinn.tallycam.control.CameraName
import se.eyevinn.tallycam.control.Controls
import se.eyevinn.tallycam.control.DeviceKey
import se.eyevinn.tallycam.control.LcpClient
import se.eyevinn.tallycam.control.ProvisioningLink
import se.eyevinn.tallycam.control.Settings
import se.eyevinn.tallycam.control.Shading
import se.eyevinn.tallycam.control.ShadingMessages
import se.eyevinn.tallycam.control.capitalizedFirst
import se.eyevinn.tallycam.control.isRemote
import se.eyevinn.tallycam.control.title
import se.eyevinn.tallycam.core.shadingCommandGroup
import se.eyevinn.tallycam.core.shadingControlGroup
import se.eyevinn.tallycam.media.CameraCapture.Shutter
import se.eyevinn.tallycam.core.ClockMode
import se.eyevinn.tallycam.core.ClockOptions
import se.eyevinn.tallycam.core.ClockService
import se.eyevinn.tallycam.core.NetworkPath
import se.eyevinn.tallycam.core.mainsHzForRegion
import se.eyevinn.tallycam.control.SrtUrl
import se.eyevinn.tallycam.media.VideoEncoder
import se.eyevinn.tallycam.media.CameraCapture
import se.eyevinn.tallycam.media.LiveStream

/**
 * The app: camera, stream, clock and the control channel to the hub (the
 * device side of the Live Contribution Protocol, docs/PROTOCOL.md). State
 * the UI shows is Compose state; everything runs on the main thread except
 * the media threads inside CameraCapture and LiveStream.
 */
class AppModel(private val context: Context) : LcpClient.Listener, LiveStream.Listener {
    enum class Tally { OFF, PREVIEW, PROGRAM }
    enum class Link { NOT_SET_UP, CONNECTING, CONNECTED }

    val settings = Settings(context)
    val camera = CameraCapture(context)
    private val cameraControls = CameraControls(camera, { streamState != LiveStream.State.IDLE },
                                                { settings.mainsHz = it })
    private val stream = LiveStream(this)
    val shading = Shading(context)
    private val client = LcpClient(this)
    private val main = Handler(Looper.getMainLooper())
    private var clock: ClockService? = null
    private val key: DeviceKey? by lazy { DeviceKey.current(context) }

    var tally by mutableStateOf(Tally.OFF)
        private set
    var link by mutableStateOf(if (settings.controlUrl.isEmpty()) Link.NOT_SET_UP else Link.CONNECTING)
        private set
    var streamState by mutableStateOf(LiveStream.State.IDLE)
        private set
    var streamMessage by mutableStateOf<String?>(null)
        private set
    var banner by mutableStateOf<String?>(null)
        private set
    var bitrateKbps by mutableStateOf(0)
        private set
    var cameraNameSetting by mutableStateOf(settings.cameraName ?: "")
        private set
    var displayName by mutableStateOf("")
        private set

    private var admitted = false
    /** Bumped per connection, so an old admission timer does nothing. */
    private var connectionSerial = 0
    private var challengeSeen = false
    private var helloName: String? = null
    /** The latest stream stats (Settings shows round trip and losses). */
    var lastStats by mutableStateOf<LiveStream.Stats?>(null)
        private set
    private var sentValues: String? = null
    /** The server asked for `image_stats`, at most this often (ns); null: not asked. */
    private var statsIntervalNs: Long? = null
    private var lastStatsSentNs = 0L

    val model: String = CameraName.model(Build.MANUFACTURER, Build.MODEL)
    val deviceId: String? get() = key?.deviceId
    val fingerprint: String? get() = key?.identity?.fingerprint

    init {
        camera.onError = { message -> main.post { showBanner(message); sendLog("error", message) } }
        // A new lens or size changes the controls' ranges: hello again.
        camera.onControlsChanged = { main.post { shadingGroups(); if (admitted) sendHello() } }
        shading.onChange = { message -> if (admitted) client.send(message) }
        camera.onStats = { stats -> main.post { statsMeasured(stats) } }
        camera.onAutoIsoLimit = { limit, iso ->
            if (limit != se.eyevinn.tallycam.core.Hot.AUTO_ISO_LIMIT_NONE) {
                val hint = if (limit == se.eyevinn.tallycam.core.Hot.AUTO_ISO_LIMIT_TOO_DARK) "too dark: use a longer shutter or more light"
                    else if (camera.settings.mainsHz == 0) "too bright: use less light or an ND filter"
                    else "too bright: the shutter is held flicker-free for mains light; outdoors set mains_hz off"
                main.post { sendLog("warn", "Auto ISO at ISO $iso, $hint") }
            }
        }
        displayName = CameraName.display(settings.cameraName, model, deviceId)
        // Mains frequency: as set, else the region's (the core's table, as on iOS).
        val hz = settings.mainsHz.takeIf { it >= 0 }
            ?: runCatching { mainsHzForRegion(java.util.Locale.getDefault().country).toInt() }.getOrDefault(50)
        cameraControls.setMainsHz(hz)
        shadingGroups()
        restoreShading()
        startClock()
        if (settings.controlUrl.isNotEmpty()) client.connect(settings.controlUrl)
        // Through a lambda: statusTick is declared below.
        main.postDelayed({ statusTick.run() }, 1000)
        main.postDelayed({ stateTick.run() }, 250)
    }

    // MARK: Provisioning and settings

    /** A tallycam:// link: saves the URLs and reconnects. */
    fun provision(link: ProvisioningLink) {
        if (server(link.controlUrl) != server(settings.controlUrl)) newProduction()
        settings.controlUrl = link.controlUrl
        link.srtUrl?.let { settings.srtUrl = it }
        this.link = Link.CONNECTING
        client.connect(link.controlUrl)
        showBanner("Connecting to ${android.net.Uri.parse(link.controlUrl).host}")
    }

    fun setControlUrl(url: String) {
        if (server(url) != server(settings.controlUrl)) newProduction()
        settings.controlUrl = url.trim()
        if (settings.controlUrl.isEmpty()) {
            client.close()
            link = Link.NOT_SET_UP
        } else {
            link = Link.CONNECTING
            client.connect(settings.controlUrl)
        }
    }

    /** The name Settings shows as the placeholder: the model and the start of the id. */
    val defaultName: String get() = CameraName.display(null, model, deviceId)

    /** The hub's host, for Settings. */
    val host: String? get() = runCatching { android.net.Uri.parse(settings.controlUrl).host }.getOrNull()

    /** "H.264, 6000 kbit/s (auto)", as iOS shows it. */
    val videoSummary: String get() {
        val codec = if (settings.videoCodec == "hevc") "HEVC" else "H.264"
        val kbps = settings.bitrateKbps.takeIf { it > 0 }
        return "$codec, ${kbps ?: DEFAULT_KBPS} kbit/s" + if (kbps == null) " (auto)" else ""
    }

    /** "Back Camera 1280x720 30 fps", as iOS shows the capture. */
    val pictureSummary: String get() {
        val size = camera.streamSize
        val lens = camera.lenses.firstOrNull { it.id == camera.cameraId }?.label ?: "Camera"
        return "$lens ${size.width}x${size.height} ${camera.settings.frameRate} fps"
    }

    fun batteryPercent(): Int? =
        (context.getSystemService(Context.BATTERY_SERVICE) as BatteryManager)
            .getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY).takeIf { it in 0..100 }

    fun clockSummary(): se.eyevinn.tallycam.control.ClockSummary? =
        clock?.let { c -> runCatching { c.statusJson() }.getOrNull()?.let(se.eyevinn.tallycam.control.ClockSummary::parse) }

    var adaptiveOn by mutableStateOf(settings.adaptiveBitrate)
        private set

    /** On by default, persisted, applied live (docs/PROTOCOL.md, `adaptive_bitrate`). */
    fun setAdaptiveBitrate(on: Boolean) {
        settings.adaptiveBitrate = on
        adaptiveOn = on
        stream.setAdaptive(on)
        publishState()
    }

    fun setSrtUrl(url: String) {
        if (url.isEmpty() || url.startsWith("srt://")) settings.srtUrl = url
        publishState()
    }

    /** Disconnects and forgets the hub, as "Disconnect and forget server" on iOS. */
    fun forgetServer() {
        newProduction()
        client.close()
        settings.controlUrl = ""
        settings.srtUrl = ""
        link = Link.NOT_SET_UP
        admitted = false
        tally = Tally.OFF
    }

    fun setCameraName(name: String) {
        settings.cameraName = name
        cameraNameSetting = settings.cameraName ?: ""
        displayName = CameraName.display(settings.cameraName, model, deviceId)
        // The hello carries the name.
        if (admitted && displayName != helloName) sendHello()
        publishState()
    }

    fun attachPreview(texture: SurfaceTexture) = camera.open(texture)

    /**
     * A tap on the preview, normalised 0..1: continuous autofocus there
     * (focus back to auto), and the metering point while exposure is auto.
     * Never changes the exposure owner or mode. Refused while focus is remote.
     */
    fun focusAt(x: Float, y: Float) {
        if (shading.view("focus")?.isRemote == true) {
            showBanner("Focus is under remote control")
            return
        }
        if (shading.machine.localAuto("focus") == se.eyevinn.tallycam.core.OwnershipChange.Changed) shading.changed()
        camera.focusAt(x, y)
    }

    /** Long press: focus at a point, then lock it there (focus local). */
    fun focusLockAt(x: Float, y: Float) {
        if (shading.view("focus")?.isRemote == true) {
            showBanner("Focus is under remote control")
            return
        }
        camera.focusAt(x, y)
        main.postDelayed({ localSet("focus", JSONObject().put("focus_mode", "locked")) }, 700)
    }

    /** The preview is gone (app in the background): the camera stays only while streaming. */
    fun detachPreview(released: () -> Unit) = camera.detachPreview(released)

    // MARK: Live

    fun toggleLive() = if (streamState == LiveStream.State.IDLE) goLive() else stopLive()

    fun goLive(): String? {
        // Asked again while a stop is still under way: start once it is done.
        if (stopping) {
            startAfterStop = true
            return null
        }
        // The stream's own state: the UI's follows a moment later.
        if (stream.state != LiveStream.State.IDLE) return null
        val id = deviceId ?: return "No device key"
        val url = settings.srtUrl.ifEmpty { return "No SRT URL: scan the hub's QR code" }
        val kbps = settings.bitrateKbps.takeIf { it > 0 } ?: DEFAULT_KBPS
        val mic = context.checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) ==
            android.content.pm.PackageManager.PERMISSION_GRANTED
        return try {
            // The foreground first: from the background Android 12+ refuses
            // it, and then nothing else has started.
            StreamService.start(context, "Live as $displayName", microphone = mic)
            val surface = stream.start(ProvisioningLink.resolveSrtUrl(url, id), camera.streamSize.width,
                                       camera.streamSize.height, camera.settings.frameRate, kbps,
                                       camera.timestampOffsetNs(), withAudio = mic,
                                       hevc = settings.videoCodec == "hevc", aac = settings.audioCodec == "aac",
                                       adaptive = settings.adaptiveBitrate)
            bitrateKbps = kbps
            streamMessage = null
            camera.setEncoderSurface(surface)
            clock?.setLive(true)
            null
        } catch (e: Exception) {
            Log.e(TAG, "Go live failed", e)
            StreamService.stop(context)
            streamMessage = e.message
            e.message ?: "could not start"
        }
    }

    /**
     * Stops the stream: the camera stops rendering into the encoder's
     * Surface first, then the encoder and SRT go (camera thread, then main).
     */
    fun stopLive(reason: String? = null) {
        startAfterStop = false
        if (stopping || (streamState == LiveStream.State.IDLE && stream.state == LiveStream.State.IDLE)) return
        stopping = true
        camera.setEncoderSurface(null) {
            main.post {
                stopping = false
                stream.stop()
                cameraControls.streamStopped()
                clock?.setLive(false)
                StreamService.stop(context)
                streamMessage = reason
                reason?.let { sendLog("warn", "Stream stopped: $it") }
                publishState()
                if (startAfterStop) {
                    startAfterStop = false
                    goLive()?.let { sendLog("error", "Go live failed: $it") }
                }
            }
        }
    }

    /** A stop is waiting for the camera to drop the encoder's Surface. */
    private var stopping = false
    private var startAfterStop = false

    override fun onStreamFailed(reason: String) = stopLive(reason)

    override fun onStreamState(state: LiveStream.State, message: String?) {
        streamState = state
        if (state != LiveStream.State.IDLE) streamMessage = message
        publishState()
    }

    // MARK: Control channel

    override fun onOpen() {
        link = Link.CONNECTED
        admitted = false
        challengeSeen = false
        sendHello()
        // A server without device keys may say nothing: admitted unless it
        // challenges soon.
        val connection = ++connectionSerial
        main.postDelayed({
            if (connection == connectionSerial && client.isOpen && !admitted && !challengeSeen) admit()
        }, 3000)
    }

    override fun onClosed(reason: String) {
        connectionSerial++
        link = if (settings.controlUrl.isEmpty()) Link.NOT_SET_UP else Link.CONNECTING
        admitted = false
        tally = Tally.OFF
        camera.onThumbnail = null
        statsIntervalNs = null
        shading.machine.setConnected(false)
        shading.changed(notify = false)
    }

    override fun onMessage(message: JSONObject) {
        val type = message.optString("type")
        when (type) {
            "challenge" -> {
                challengeSeen = true
                val k = key ?: return
                runCatching { k.authMessage(message.getString("server"), message.getString("nonce")) }
                    .onSuccess { client.send(JSONObject(it)) }
                    .onFailure { Log.e(TAG, "auth failed", it) }
                return
            }
            "message" -> {
                showBanner(message.optString("text"))
                return
            }
        }
        if (!admitted) admit()
        when (type) {
            "tally" -> tally = when (message.optString("state")) {
                "program" -> Tally.PROGRAM
                "preview" -> Tally.PREVIEW
                else -> Tally.OFF
            }
            "preview" -> setPreview(message.optBoolean("enabled"), message.optDouble("fps", 1.0))
            "set" -> applySet(message.opt("id"), message.optJSONObject("values") ?: JSONObject())
            "command" -> {
                val name = message.optString("name")
                val refused = shadingCommandGroup(name)?.let { shading.machine.checkRemote(it) }
                if (refused != null) message.opt("id")?.let { result(it, listOf("$name: $refused")) }
                else runCommand(message.opt("id"), name, message.optJSONObject("args"))
            }
            "own" -> {
                val own = ShadingMessages.own(message)
                val label = own.holderLabel
                val taken = shading.machine.own(own.groups, own.holderId, label)
                if (taken.fromLocal.isNotEmpty()) {
                    val names = taken.fromLocal.mapNotNull { shading.view(it)?.title }
                    showBanner("${names.joinToString(" and ")} taken over by ${label.ifEmpty { "the hub" }}")
                }
                shading.changed()
                message.opt("id")?.let { result(it, taken.skipped.map { g -> "$g: no such group on this camera" }) }
            }
            "release" -> {
                val released = shading.machine.release(ShadingMessages.groups(message))
                shading.changed()
                val errors = applyAuto(released)
                message.opt("id")?.let { result(it, errors) }
            }
            "production" -> {
                val production = ShadingMessages.production(message)
                cameraControls.setProduction(production.mainsHz, production.shutter)?.let { sendLog("warn", it) }
                publishState()
            }
            "stats" -> {
                val fps = message.optDouble("fps", 1.0).takeIf { it.isFinite() && it > 0 } ?: 1.0
                statsIntervalNs = if (message.optBoolean("enabled")) (1e9 / fps.coerceIn(0.2, 4.0)).toLong() else null
                camera.statsIntervalNs = (1e9 / maxOf(fps.coerceIn(0.2, 4.0), 2.0)).toLong()
            }
        }
    }


    private fun admit() {
        admitted = true
        client.resetBackoff()
        shading.machine.setConnected(true)
        shading.changed(notify = false)
        // The full state, then ownership (docs/SHADING.md).
        publishState()
        client.send(shading.machine.messageJson())
        sendStatus()
        pendingLog?.let {
            pendingLog = null
            sendLog("warn", it)
        }
    }

    private fun sendHello() {
        val k = key ?: return
        helloName = displayName
        val device = JSONObject()
            .put("id", k.deviceId)
            .put("name", displayName)
            .put("model", model)
            .put("platform", "android")
            .put("os_version", Build.VERSION.RELEASE)
            .put("app_version", BuildConfig.VERSION_NAME)
        val controls = mutableListOf(
            Controls.text("camera_name", "Camera name", "stream"),
            Controls.bool("live", "Live", "stream"),
            Controls.text("stream_url", "Stream URL", "stream"),
            Controls.range("srt_latency_ms", "SRT latency", "stream", SrtUrl.LATENCY_RANGE.first.toDouble(),
                           SrtUrl.LATENCY_RANGE.last.toDouble(), 10.0, "ms"),
            Controls.range("bitrate_kbps", "Video bitrate", "stream", 500.0, 20000.0, 100.0, "kbit/s"),
            Controls.bool("adaptive_bitrate", "Adaptive bitrate", "stream"),
            enumOf("video_codec", "Video codec", buildList {
                add("h264" to "H.264")
                if (VideoEncoder.hasEncoder(android.media.MediaFormat.MIMETYPE_VIDEO_HEVC)) add("hevc" to "HEVC")
            }).put("locked_while_live", true),
            enumOf("audio_codec", "Audio codec", listOf("opus" to "Opus", "aac" to "AAC")).put("locked_while_live", true),
        )
        controls += cameraControls.descriptors()
        client.send(JSONObject()
            .put("type", "hello")
            // 2: ownership of exposure, colour and focus, and production settings.
            .put("protocol", 2)
            .put("public_key", k.identity.publicKey)
            .put("device", device)
            .put("controls", Controls.array(controls))
            .put("commands", Controls.array(cameraControls.commands() + listOf(
                Controls.command("clock_log", "Clock log")
                    .put("args", JSONObject().put("lines", "1..4000").put("kinds", "comma-separated, e.g. tracking,selection")),
            ))))
    }

    private fun values(): JSONObject {
        val v = cameraControls.values()
        v.put("camera_name", displayName)
            .put("live", streamState != LiveStream.State.IDLE)
            .put("stream_url", settings.srtUrl)
            .put("srt_latency_ms", SrtUrl.latency(settings.srtUrl) ?: SrtUrl.DEFAULT_LATENCY_MS)
            .put("bitrate_kbps", settings.bitrateKbps.takeIf { it > 0 } ?: DEFAULT_KBPS)
            .put("adaptive_bitrate", settings.adaptiveBitrate)
            .put("video_codec", settings.videoCodec)
            .put("audio_codec", settings.audioCodec)
        return v
    }

    private fun enumOf(id: String, label: String, options: List<Pair<String, String>>) =
        JSONObject().put("id", id).put("label", label).put("group", "stream").put("kind", "enum")
            .put("options", org.json.JSONArray(options.map { JSONObject().put("value", it.first).put("label", it.second) }))

    /** `force` false: only when a value changed (the poll, 4 times a second). */
    private fun publishState(force: Boolean = true) {
        val v = values()
        val text = v.toString()
        if (!force && text == sentValues) return
        // Also offline: the values of owned groups survive a restart (D3).
        shading.note(v, shutterFree)
        if (!admitted) return
        sentValues = text
        client.send(JSONObject().put("type", "state").put("values", v))
    }

    private fun runCommand(id: Any?, name: String, args: JSONObject?) {
        when (name) {
            "focus_at" -> {
                val x = args?.optDouble("x", Double.NaN) ?: Double.NaN
                val y = args?.optDouble("y", Double.NaN) ?: Double.NaN
                if (x.isNaN() || y.isNaN()) {
                    id?.let { result(it, listOf("x and y (0..1) required")) }
                    return
                }
                camera.focusAt(x.toFloat(), y.toFloat())
                id?.let { result(it, emptyList()) }
            }
            "clock_log" -> {
                val lines = args?.optDouble("lines", 200.0)?.takeIf { it.isFinite() }?.coerceIn(1.0, 4000.0)?.toInt() ?: 200
                val kinds = args?.optString("kinds", "").orEmpty().split(',').map { it.trim() }.filter { it.isNotEmpty() }
                val c = clock
                val data = JSONObject()
                    .put("lines", org.json.JSONArray(c?.logLines(kinds, lines.toUInt()) ?: emptyList<String>()))
                    .put("reports", c?.reportsText() ?: "")
                id?.let { client.send(JSONObject().put("type", "result").put("id", it).put("ok", true).put("data", data)) }
            }
            "wb_one_shot" -> camera.whiteBalanceOnce {
                main.post {
                    id?.let { result(it, emptyList()) }
                    publishState()
                }
            }
            else -> id?.let { result(it, listOf("Unknown command $name")) }
        }
    }

    private fun applySet(id: Any?, values: JSONObject) {
        val errors = mutableListOf<String>()
        // Modes before the values that depend on them, as on iOS, so
        // {"exposure_mode": "manual", "iso": 400} works in one message, and
        // `live` last, so it starts with the URL and codecs of the same set.
        fun order(key: String) = when (key) {
            "live" -> Int.MAX_VALUE
            else -> MODE_FIRST.indexOf(key).let { i -> if (i < 0) MODE_FIRST.size else i }
        }
        val free = shutterFree
        for (key in values.keys().asSequence().sortedBy(::order)) {
            val value = values.get(key)
            // Not this server's group (docs/SHADING.md, rule 2).
            val refused = shadingControlGroup(key, free)?.let { shading.machine.checkRemote(it) }
            if (refused != null) {
                errors += "$key: $refused"
                continue
            }
            val error: String? = when (key) {
                "camera_name" -> if (value is String) { setCameraName(value); null } else "expected text"
                "live" -> when (value) {
                    true -> goLive()
                    false -> { stopLive(); null }
                    else -> "expected a bool"
                }
                "stream_url" -> when {
                    value !is String || !value.startsWith("srt://") -> "expected an srt:// URL"
                    else -> {
                        settings.srtUrl = value
                        if (streamState == LiveStream.State.IDLE) null else "applies the next time the stream starts"
                    }
                }
                "srt_latency_ms" -> {
                    val wanted = (value as? Number)?.toDouble()
                    if (wanted == null || !wanted.isFinite()) "expected a number" else {
                        val ms = wanted.coerceIn(SrtUrl.LATENCY_RANGE.first.toDouble(), SrtUrl.LATENCY_RANGE.last.toDouble()).toInt()
                        val url = SrtUrl.settingLatency(ms, settings.srtUrl)
                        when {
                            url == null -> "no SRT URL to set it in"
                            else -> {
                                settings.srtUrl = url
                                when {
                                    streamState != LiveStream.State.IDLE -> "applies the next time the stream starts"
                                    ms.toDouble() != wanted -> "clamped to $ms"
                                    else -> null
                                }
                            }
                        }
                    }
                }
                "adaptive_bitrate" -> if (value is Boolean) { setAdaptiveBitrate(value); null } else "expected a bool"
                "video_codec" -> when {
                    value != "h264" && !(value == "hevc" && VideoEncoder.hasEncoder(android.media.MediaFormat.MIMETYPE_VIDEO_HEVC)) ->
                        "expected h264 or hevc"
                    value == settings.videoCodec -> null
                    streamState != LiveStream.State.IDLE -> CameraControls.NOT_WHILE_LIVE
                    else -> { settings.videoCodec = value as String; null }
                }
                "audio_codec" -> when {
                    value != "opus" && value != "aac" -> "expected opus or aac"
                    value == settings.audioCodec -> null
                    streamState != LiveStream.State.IDLE -> CameraControls.NOT_WHILE_LIVE
                    else -> { settings.audioCodec = value as String; null }
                }
                "bitrate_kbps" -> {
                    val wanted = (value as? Number)?.toDouble()
                    if (wanted == null || !wanted.isFinite()) "expected a number" else {
                        val kbps = wanted.coerceIn(500.0, 20000.0).toInt()
                        settings.bitrateKbps = kbps
                        stream.setBitrate(kbps)
                        bitrateKbps = kbps
                        if (kbps.toDouble() == wanted) null else "clamped to $kbps"
                    }
                }
                else -> if (cameraControls.handles(key)) cameraControls.apply(key, value) else "Unknown control $key"
            }
            error?.let { errors += "$key: $it" }
        }
        id?.let { result(it, errors) }
        publishState()
    }

    private fun result(id: Any, errors: List<String>) {
        val r = JSONObject().put("type", "result").put("id", id).put("ok", errors.isEmpty())
        if (errors.isNotEmpty()) r.put("error", errors.joinToString("; "))
        client.send(r)
    }

    private fun sendLog(level: String, message: String) {
        if (admitted) client.send(JSONObject().put("type", "log").put("level", level).put("message", message))
    }

    private fun setPreview(enabled: Boolean, fps: Double) {
        if (!enabled) {
            camera.onThumbnail = null
            return
        }
        camera.thumbnailIntervalNs = (1e9 / fps.coerceIn(0.1, 10.0)).toLong()
        camera.onThumbnail = { jpeg, w, h ->
            val msg = JSONObject().put("type", "preview")
                .put("jpeg", Base64.encodeToString(jpeg, Base64.NO_WRAP)).put("width", w).put("height", h)
            main.post { if (admitted) client.send(msg) }
        }
    }

    // MARK: Shading (docs/SHADING.md)

    /** The production shutter is free: the shutter is then an exposure control. */
    private val shutterFree: Boolean get() = camera.settings.effectiveShutter == Shutter.Free

    /** "host:port" of a control URL, to tell a new server from the same one. */
    private fun server(url: String): String? = runCatching {
        android.net.Uri.parse(url.trim()).let { u -> u.host?.let { "$it:${u.port}" } }
    }.getOrNull()

    /** The groups this camera has: focus only with a focus control. */
    private fun shadingGroups() {
        val groups = Shading.GROUPS.filter { it != "focus" || camera.minFocusDiopters != null }
        if (groups != shading.groups.map { it.group }) shading.useGroups(groups)
    }

    /**
     * A new control server: its remote groups go back to auto, and the
     * phone's own production applies until it sends one.
     */
    private fun newProduction() {
        val released = shading.machine.newServer()
        shading.changed(notify = false)
        applyAuto(released)
        cameraControls.setProduction(null, null)
    }

    /** Puts groups into auto on the camera; returns the errors. */
    private fun applyAuto(groups: List<String>): List<String> {
        val values = Shading.autoValues(groups)
        val errors = mutableListOf<String>()
        for (key in values.keys().asSequence().sortedBy { MODE_FIRST.indexOf(it).let { i -> if (i < 0) MODE_FIRST.size else i } }) {
            if (!cameraControls.handles(key)) continue
            cameraControls.apply(key, values.get(key))?.let { errors += "$key: $it" }
        }
        publishState()
        return errors
    }

    /** After a relaunch, the groups that were local or remote get their values back (D3). */
    private fun restoreShading() {
        val values = shading.restoredValues()
        val errors = mutableListOf<String>()
        for (key in values.keys().asSequence().sortedBy { MODE_FIRST.indexOf(it).let { i -> if (i < 0) MODE_FIRST.size else i } }) {
            if (!cameraControls.handles(key)) continue
            cameraControls.apply(key, values.get(key))?.let { errors += "$key: $it" }
        }
        if (errors.isNotEmpty()) pendingLog = "Restoring exposure, colour and focus: ${errors.joinToString("; ")}"
    }

    /** Sent once the hub admits the phone. */
    private var pendingLog: String? = null

    private fun statsMeasured(stats: se.eyevinn.tallycam.core.ImageStatsResult) {
        shading.update(stats)
        val interval = statsIntervalNs ?: return
        val now = android.os.SystemClock.elapsedRealtimeNanos()
        if (!admitted || now - lastStatsSentNs < interval * 0.9) return
        lastStatsSentNs = now
        val measured = runCatching { JSONObject(stats.json) }.getOrNull() ?: return
        val v = cameraControls.values()
        val cameraJson = JSONObject()
        for (key in listOf("iso", "shutter", "wb_temperature", "wb_tint")) if (v.has(key)) cameraJson.put(key, v.get(key))
        val message = JSONObject().put("type", "image_stats").put("luma", measured.opt("luma")).put("camera", cameraJson)
        measured.optJSONObject("colour")?.let { message.put("colour", it) }
        client.send(message)
    }

    // MARK: Local controls (the camera operator)

    /** The operator locks or sets values in a group. Refused, with a banner, while the group is remote. */
    fun localSet(group: String, values: JSONObject) {
        when (val change = shading.machine.localChange(group)) {
            is se.eyevinn.tallycam.core.OwnershipChange.Refused -> {
                showBanner(change.reason.capitalizedFirst())
                return
            }
            se.eyevinn.tallycam.core.OwnershipChange.Changed -> shading.changed()
            se.eyevinn.tallycam.core.OwnershipChange.Unchanged -> Unit
        }
        val errors = mutableListOf<String>()
        for (key in values.keys().asSequence().sortedBy { MODE_FIRST.indexOf(it).let { i -> if (i < 0) MODE_FIRST.size else i } }) {
            cameraControls.apply(key, values.get(key))?.let { errors += it }
        }
        errors.firstOrNull()?.let { showBanner(it.capitalizedFirst()) }
        publishState()
    }

    /** The operator puts a group back into auto. */
    fun localAuto(group: String) {
        when (val change = shading.machine.localAuto(group)) {
            is se.eyevinn.tallycam.core.OwnershipChange.Refused -> showBanner(change.reason.capitalizedFirst())
            else -> {
                shading.changed()
                applyAuto(listOf(group))
            }
        }
    }

    /** The escape hatch (D2): a remote group taken local while the hub is away. The values stay. */
    fun takeLocal(group: String) {
        when (val change = shading.machine.takeLocal(group)) {
            is se.eyevinn.tallycam.core.OwnershipChange.Refused -> showBanner(change.reason.capitalizedFirst())
            else -> shading.changed()
        }
    }

    /** White balance once, from the operator's controls (after [localSet]). */
    fun localWhiteBalanceOnce() = camera.whiteBalanceOnce { main.post { publishState() } }

    /** A lens button: lens is shared by the operator and the hub (D5). */
    fun selectLens(key: String) {
        cameraControls.apply("lens", key)?.let { showBanner(it.capitalizedFirst()) }
        publishState()
    }

    /** The zoom rocker: doublings per second, 0 stops (and reports the zoom reached). */
    fun rampZoom(rate: Double) {
        camera.rampZoom(rate)
        if (rate == 0.0) main.postDelayed({ publishState() }, 100)
    }

    /** The camera's descriptors and values, for the operator's group controls. */
    fun cameraDescriptors(): List<JSONObject> = cameraControls.descriptors()

    fun cameraValues(): JSONObject = cameraControls.values()

    // MARK: Status

    private val statusTick = object : Runnable {
        override fun run() {
            lastStats = stream.stats()
            lastStats?.let { bitrateKbps = it.bitrateKbps.takeIf { k -> k > 0 } ?: bitrateKbps }
            sendStatus()
            main.postDelayed(this, 1000)
        }
    }

    /** Four times a second: values that changed (measured ISO, shutter, focus in auto modes). */
    private val stateTick = object : Runnable {
        override fun run() {
            publishState(force = false)
            main.postDelayed(this, 250)
        }
    }

    private fun sendStatus() {
        if (!admitted) return
        val battery = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        val level = battery?.let {
            val l = it.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
            val s = it.getIntExtra(BatteryManager.EXTRA_SCALE, -1)
            if (l >= 0 && s > 0) l.toDouble() / s else null
        }
        val charging = battery?.getIntExtra(BatteryManager.EXTRA_STATUS, -1).let {
            it == BatteryManager.BATTERY_STATUS_CHARGING || it == BatteryManager.BATTERY_STATUS_FULL
        }
        val streamJson = JSONObject()
            .put("state", when (streamState) {
                LiveStream.State.IDLE -> if (streamMessage != null) "error" else "idle"
                LiveStream.State.CONNECTING -> "connecting"
                LiveStream.State.LIVE -> "live"
            })
            .put("url", SrtUrl.redacted(settings.srtUrl))
        streamMessage?.let { streamJson.put("error", it) }
        lastStats?.let {
            streamJson.put("bitrate_kbps", it.bitrateKbps).put("rtt_ms", it.rttMs)
                .put("retransmits", it.retransmits).put("dropped", it.dropped)
                .put("late_video_frames", it.lateVideoFrames).put("late_audio_frames", it.lateAudioFrames)
            if (it.srtLatencyMs > 0) streamJson.put("srt_latency_ms", it.srtLatencyMs)
            if (it.targetKbps > 0) streamJson.put("target_kbps", it.targetKbps)
            it.link?.let { link -> streamJson.put("link", link) }
            it.captureToMuxMs?.let { (p50, max) ->
                streamJson.put("latency", JSONObject().put("capture_to_mux_ms",
                    JSONObject().put("p50", Math.round(p50 * 10) / 10.0).put("max", Math.round(max * 10) / 10.0)))
            }
        }
        val status = JSONObject().put("type", "status")
            .put("thermal", thermal())
            .put("stream", streamJson)
            .put("network", network())
        level?.let { status.put("battery", JSONObject().put("level", Math.round(it * 100) / 100.0).put("charging", charging)) }
        clock?.let { c -> runCatching { JSONObject(c.statusJson()) }.getOrNull()?.let { status.put("clock", it) } }
        camera.genlockStatus?.let { g ->
            val state = when (g[0].toInt()) {
                se.eyevinn.tallycam.core.Hot.GENLOCK_LOCKED -> "locked"
                se.eyevinn.tallycam.core.Hot.GENLOCK_ACQUIRING -> "acquiring"
                else -> "free_run"
            }
            status.put("genlock", JSONObject().put("state", state)
                .put("phase_error_ms", Math.round(g[1] / 1e3) / 1e3).put("phase_error_max_ms", Math.round(g[2] / 1e3) / 1e3)
                .put("frame_duration_us", Math.round(g[3] / 10) / 100.0).put("correction_ppm", Math.round(g[4] * 10) / 10.0)
                .put("calibration_us", 0))
        }
        client.send(status)
    }

    private fun thermal(): String {
        val pm = context.getSystemService(Context.POWER_SERVICE) as PowerManager
        return when (pm.currentThermalStatus) {
            PowerManager.THERMAL_STATUS_NONE -> "nominal"
            PowerManager.THERMAL_STATUS_LIGHT, PowerManager.THERMAL_STATUS_MODERATE -> "fair"
            PowerManager.THERMAL_STATUS_SEVERE -> "serious"
            else -> "critical"
        }
    }

    private fun network(): String {
        val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return "none"
        return when {
            caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> "wifi"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "cellular"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "wired"
            else -> "other"
        }
    }

    // MARK: Clock

    private fun startClock() {
        clock = runCatching {
            ClockService.start(ClockOptions(ClockMode.DISCIPLINE, null, null,
                                            context.filesDir.resolve("clock.drift").path))
        }.onFailure { Log.e(TAG, "Clock not started", it) }.getOrNull()
        val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        cm.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) = pathChanged(network, true)
            override fun onLost(network: Network) = pathChanged(network, false)
        })
    }

    private fun pathChanged(network: Network, up: Boolean) {
        // A new network: try the hub now instead of waiting out the backoff.
        if (up) main.post { if (!client.isOpen && settings.controlUrl.isNotEmpty()) client.connect(settings.controlUrl) }
        val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        val props = cm.getLinkProperties(network)
        val addresses = props?.linkAddresses?.map { it.address } ?: emptyList()
        clock?.pathChanged(NetworkPath(up, addresses.any { it is java.net.Inet4Address },
                                       addresses.any { it is java.net.Inet6Address },
                                       "${props?.interfaceName ?: "net"}:$network"))
    }

    fun showMessage(text: String) = showBanner(text)

    private fun showBanner(text: String) {
        banner = text
        main.removeCallbacks(clearBanner)
        main.postDelayed(clearBanner, 8000)
    }

    private val clearBanner = Runnable { banner = null }

    fun close() {
        stopLive()
        client.close()
        camera.close()
        clock?.stop()
        main.removeCallbacks(statusTick)
        main.removeCallbacks(stateTick)
    }

    companion object {
        private const val TAG = "TallyCam"
        const val DEFAULT_KBPS = 6000
        private val MODE_FIRST = listOf("lens", "resolution", "frame_rate", "exposure_mode", "wb_mode", "focus_mode", "genlock")
    }
}
