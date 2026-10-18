package se.eyevinn.tallycam.control

import org.json.JSONArray
import org.json.JSONObject
import se.eyevinn.tallycam.media.CameraCapture
import se.eyevinn.tallycam.media.CameraCapture.ExposureMode
import se.eyevinn.tallycam.media.CameraCapture.FocusMode
import se.eyevinn.tallycam.media.CameraCapture.Shutter
import se.eyevinn.tallycam.media.CameraCapture.WhiteBalanceMode
import kotlin.math.roundToInt
import kotlin.math.roundToLong

/**
 * The camera controls (docs/PROTOCOL.md, "Controls in the iOS prototype"),
 * with the same ids and meanings as on iOS, offered only where this camera
 * has them: descriptors for `hello`, current values for `state` (measured
 * in auto modes), and `set`.
 */
class CameraControls(
    private val camera: CameraCapture,
    private val isLive: () -> Boolean,
    /** Persists the phone's own `mains_hz` (50, 60, 0). */
    private val saveMainsHz: (Int) -> Unit = {},
) {
    /**
     * The control server's production shutter (docs/SHADING.md, "Shutter"),
     * or null: none, the phone is its own production (one mains period, free
     * with mains off).
     */
    var serverShutter: Shutter? = null
        private set

    /**
     * The phone's own mains frequency (Settings, saved). A hub's production
     * replaces it while it applies, without being saved, until the phone
     * changes mains again; forgetting the hub returns to this one.
     */
    var ownMainsHz: Int = 50
        private set

    /** A production that came while live, applied when the stream stops. */
    private var pendingProduction: Pair<Int?, Shutter?>? = null

    /**
     * Mains is never locked by the hub; the last write wins (docs/SHADING.md,
     * "Mains on the phone"). The phone's choice under a hub production, kept
     * until the hub sends a different production.
     */
    private var mainsOverride: Int? = null

    /** The last production received (mains, shutter), to tell a new one from a repeat. */
    private var lastProduction: Pair<Int?, Shutter?>? = null

    private fun whenMode(control: String, vararg modes: String) = JSONObject().put(control,
        if (modes.size == 1) modes[0] else JSONArray(modes.toList()))

    private fun options(vararg pairs: Pair<Any, String>) =
        JSONArray(pairs.map { JSONObject().put("value", it.first).put("label", it.second) })

    private fun enumControl(id: String, label: String, group: String, options: JSONArray) =
        JSONObject().put("id", id).put("label", label).put("group", group).put("kind", "enum").put("options", options)

    fun descriptors(): List<JSONObject> {
        val c = camera
        val list = mutableListOf<JSONObject>()
        if (c.lenses.size > 1) {
            list += enumControl("lens", "Lens", "lens", options(*c.lenses.map { it.key to it.label }.toTypedArray()))
        }
        c.zoomRange?.let {
            list += Controls.range("zoom", "Zoom", "lens", it.lower.toDouble(), it.upper.toDouble(), 0.1, "x")
                .put("scale", "log")
        }
        // Not while live (docs/SHADING.md, D11).
        if (c.resolutions.size > 1) {
            list += enumControl("resolution", "Resolution", "image",
                                options(*c.resolutions.map { "${it}p" to "${it}p" }.toTypedArray()))
                .put("locked_while_live", true)
        }
        if (c.frameRates.size > 1) {
            list += enumControl("frame_rate", "Frame rate", "image",
                                options(*c.frameRates.map { it to "$it fps" }.toTypedArray()))
                .put("locked_while_live", true)
        }
        if (c.hasStabilization) {
            list += enumControl("stabilization", "Stabilization", "image", options("off" to "Off", "standard" to "Standard"))
        }
        if (c.hasTorch) list += Controls.bool("torch", "Torch", "image")
        // Exposure: auto is the core's auto ISO loop at the production
        // shutter where the sensor can be set, else Camera2's auto exposure.
        val modes = mutableListOf<Pair<Any, String>>("auto" to "Auto", "locked" to "Locked")
        if (c.hasManualSensor) modes += "manual" to "Manual"
        list += enumControl("exposure_mode", "Exposure", "exposure", options(*modes.toTypedArray()))
        if (c.hasManualSensor) {
            c.isoRange?.let {
                list += Controls.range("iso", "ISO", "exposure", it.lower.toDouble(), it.upper.toDouble(), 1.0, "")
                    .put("scale", "log").put("enabled_when", whenMode("exposure_mode", "manual"))
            }
            // Genlock needs Camera2's auto exposure off, which it is with a
            // manual sensor in every mode (docs/GENLOCK.md).
            list += Controls.bool("genlock", "Genlock", "image")
            list += Controls.range("genlock_offset_us", "Genlock offset", "image", -Settings.GENLOCK_OFFSET_MAX_US,
                                   Settings.GENLOCK_OFFSET_MAX_US, 10.0, "µs")
            shutters().takeIf { it.isNotEmpty() }?.let { shutters ->
                // A production setting: set only under a free shutter, in manual.
                val shutter = enumControl("shutter", "Shutter", "exposure", options(*shutters.map { it to "1/$it" }.toTypedArray()))
                    .put("enabled_when", whenMode("exposure_mode", "manual"))
                if (c.settings.effectiveShutter != Shutter.Free) shutter.put("readonly", true)
                list += shutter
            }
        }
        mainsOptions().takeIf { it.size > 1 }?.let { hz ->
            // A production setting: not while live; the phone and the hub both set it, the last write wins.
            list += enumControl("mains_hz", "Mains frequency", "exposure",
                                options(*hz.map { it to if (it == 0) "Off" else "$it Hz" }.toTypedArray()))
                .put("locked_while_live", true)
        }
        c.evRange?.let { (range, step) ->
            list += Controls.range("ev_bias", "Exposure bias", "exposure", (range.lower * step).toDouble(),
                                   (range.upper * step).toDouble(), step.toDouble(), "EV")
                .put("enabled_when", whenMode("exposure_mode", "auto"))
        }
        // White balance.
        val presets = c.awbPresets.filter { it in CameraMath.PRESET_KELVIN }
        val wbModes = mutableListOf<Pair<Any, String>>("auto" to "Auto", "locked" to "Locked")
        if (presets.isNotEmpty()) wbModes += "manual" to "Manual"
        list += enumControl("wb_mode", "White balance", "white_balance", options(*wbModes.toTypedArray()))
        if (presets.isNotEmpty()) {
            val kelvins = presets.map { CameraMath.PRESET_KELVIN.getValue(it) }
            list += Controls.range("wb_temperature", "Temperature", "white_balance", kelvins.min().toDouble(),
                                   kelvins.max().toDouble(), 100.0, "K")
                .put("enabled_when", whenMode("wb_mode", "manual"))
        }
        // Focus.
        if (c.minFocusDiopters != null) {
            list += enumControl("focus_mode", "Focus", "focus",
                                options("auto" to "Auto", "locked" to "Locked", "manual" to "Manual"))
            list += Controls.range("focus_position", "Focus position", "focus", 0.0, 1.0, 0.01, "")
                .put("enabled_when", whenMode("focus_mode", "manual"))
        }
        return list
    }

    fun commands(): List<JSONObject> {
        val list = mutableListOf<JSONObject>()
        list += Controls.command("focus_at", "Focus at point").put("args", JSONObject().put("x", "0..1").put("y", "0..1"))
        list += Controls.command("wb_one_shot", "White balance once")
        return list
    }

    private fun shutters(): List<Int> {
        val range = camera.exposureRange ?: return emptyList()
        return CameraMath.shutterOptions(range.lower, range.upper, camera.settings.frameRate)
    }

    fun values(): JSONObject {
        val c = camera
        val s = c.settings
        val m = c.measured
        val v = JSONObject()
        if (c.lenses.size > 1) c.lenses.firstOrNull { it.id == c.cameraId }?.let { v.put("lens", it.key) }
        if (c.zoomRange != null) v.put("zoom", (s.zoom * 100).roundToInt() / 100.0)
        if (c.resolutions.size > 1) v.put("resolution", "${c.streamSize.height}p")
        if (c.frameRates.size > 1) v.put("frame_rate", s.frameRate)
        if (c.hasStabilization) v.put("stabilization", if (s.stabilization) "standard" else "off")
        if (c.hasTorch) v.put("torch", s.torch)
        v.put("exposure_mode", s.exposureMode.name.lowercase())
        if (c.hasManualSensor) {
            v.put("genlock", s.genlock)
            v.put("genlock_offset_us", s.genlockOffsetNs / 1000.0)
            // The sensor's values in every mode (auto ISO writes them).
            v.put("iso", s.iso)
            v.put("shutter", CameraMath.shutterDenominator(s.exposureNs, shutters().ifEmpty { CameraMath.SHUTTERS }))
        }
        if (mainsOptions().size > 1) v.put("mains_hz", s.mainsHz)
        c.evRange?.let { (_, step) -> v.put("ev_bias", (s.evSteps * step * 100).roundToInt() / 100.0) }
        v.put("wb_mode", s.whiteBalanceMode.name.lowercase())
        if (s.whiteBalanceMode == WhiteBalanceMode.MANUAL) {
            CameraMath.PRESET_KELVIN[s.awbPreset]?.let { v.put("wb_temperature", it) }
        }
        c.minFocusDiopters?.let { min ->
            v.put("focus_mode", s.focusMode.name.lowercase())
            val diopters = if (s.focusMode == FocusMode.AUTO) m.focusDiopters else s.focusDiopters
            diopters?.let { v.put("focus_position", (CameraMath.focusPosition(it, min) * 100).roundToInt() / 100.0) }
        }
        return v
    }

    /** Applies one control; returns an error or warning, or null. */
    fun apply(id: String, value: Any): String? {
        val c = camera
        val s = c.settings
        fun number(): Double? = (value as? Number)?.toDouble()?.takeIf { it.isFinite() }
        return when (id) {
            "lens" -> {
                val lens = c.lenses.firstOrNull { it.key == value } ?: return "expected " + c.lenses.joinToString(" or ") { it.key }
                c.switchLens(lens.id)
                null
            }
            "zoom" -> {
                val range = c.zoomRange ?: return "not available on this lens"
                val zoom = number() ?: return "expected a number"
                val clamped = zoom.toFloat().coerceIn(range.lower, range.upper)
                c.update { it.copy(zoom = clamped) }
                if (clamped.toDouble() == zoom) null else "clamped to $clamped"
            }
            "resolution" -> {
                val height = (value as? String)?.removeSuffix("p")?.toIntOrNull()?.takeIf { it in c.resolutions }
                    ?: return "expected " + c.resolutions.joinToString(" or ") { "${it}p" }
                if (isLive()) return NOT_WHILE_LIVE
                c.setResolution(height)
                null
            }
            "frame_rate" -> {
                val fps = number()?.toInt()?.takeIf { it in c.frameRates } ?: return "expected " + c.frameRates.joinToString(" or ")
                // The rate it has is no change, live or not (as on iOS).
                if (fps == s.frameRate) return null
                if (isLive()) return NOT_WHILE_LIVE
                c.update { it.copy(frameRate = fps, exposureNs = it.exposureNs.coerceAtMost(1_000_000_000L / fps)) }
                null
            }
            "stabilization" -> when (value) {
                "off" -> { c.update { it.copy(stabilization = false) }; null }
                "standard" -> { c.update { it.copy(stabilization = true) }; null }
                else -> "expected off or standard"
            }
            "torch" -> if (value is Boolean) { c.update { it.copy(torch = value) }; null } else "expected a bool"
            "exposure_mode" -> {
                val mode = when (value) {
                    // `auto_iso` is what `auto` was called before protocol 2.
                    "auto", "auto_iso" -> ExposureMode.AUTO
                    "locked" -> ExposureMode.LOCKED
                    "manual" -> if (c.hasManualSensor) ExposureMode.MANUAL else return "manual exposure not supported"
                    else -> return "expected auto, locked or manual"
                }
                // Locked and manual start from what auto had: with a manual
                // sensor the settings already hold it. Auto starts with no bias.
                c.update { if (mode == ExposureMode.AUTO) it.copy(exposureMode = mode, evSteps = 0) else it.copy(exposureMode = mode) }
                null
            }
            "iso" -> {
                if (s.exposureMode != ExposureMode.MANUAL) return "requires exposure_mode manual"
                val range = c.isoRange ?: return "not available"
                val iso = number() ?: return "expected a number"
                val clamped = iso.roundToInt().coerceIn(range.lower, range.upper)
                c.update { it.copy(iso = clamped) }
                if (clamped.toDouble() == iso) null else "clamped to $clamped"
            }
            "shutter" -> {
                if (s.effectiveShutter != Shutter.Free) return "the shutter is a production setting"
                if (s.exposureMode != ExposureMode.MANUAL) return "requires exposure_mode manual"
                val d = number()?.toInt()?.takeIf { it in shutters() } ?: return "expected one of " + shutters().joinToString()
                c.update { it.copy(exposureNs = CameraMath.shutterNs(d)) }
                null
            }
            "genlock" -> {
                if (value !is Boolean) return "expected a bool"
                if (!c.hasManualSensor) return "needs manual exposure, which this camera lacks"
                c.update { it.copy(genlock = value) }
                null
            }
            "genlock_offset_us" -> {
                val us = number() ?: return "expected a number"
                val clamped = Settings.clampedGenlockOffsetUs(us)
                // Kept across launches by the camera (Settings).
                c.update { it.copy(genlockOffsetNs = (clamped * 1000).roundToLong()) }
                if (clamped == us) null else "clamped to $clamped"
            }
            "mains_hz" -> {
                val hz = number()?.takeIf { it == 50.0 || it == 60.0 || it == 0.0 }?.toInt()
                    ?: return "expected 50, 60 or 0 (off)"
                if (hz !in mainsOptions()) return "not supported by this camera"
                if (hz == s.mainsHz) return null
                if (isLive()) return NOT_WHILE_LIVE
                // Saved as the phone's own; under a hub production it holds
                // until the hub sends a different one.
                ownMainsHz = hz
                saveMainsHz(hz)
                if (serverShutter != null) mainsOverride = hz
                applyProduction(hz, serverShutter ?: Shutter.Mains)
                null
            }
            "ev_bias" -> {
                if (s.exposureMode != ExposureMode.AUTO) return "requires exposure_mode auto"
                val (range, step) = c.evRange ?: return "not available"
                val ev = number() ?: return "expected a number"
                val steps = (ev / step).roundToInt().coerceIn(range.lower, range.upper)
                c.update { it.copy(evSteps = steps) }
                if (kotlin.math.abs(steps * step - ev) < 1e-6) null else "rounded to ${steps * step}"
            }
            "wb_mode" -> {
                val mode = when (value) {
                    "auto" -> WhiteBalanceMode.AUTO
                    "locked" -> WhiteBalanceMode.LOCKED
                    "manual" -> if (c.awbPresets.any { it in CameraMath.PRESET_KELVIN }) WhiteBalanceMode.MANUAL
                                else return "manual white balance not supported"
                    else -> return "expected auto, locked or manual"
                }
                c.update { it.copy(whiteBalanceMode = mode) }
                null
            }
            "wb_temperature" -> {
                if (s.whiteBalanceMode != WhiteBalanceMode.MANUAL) return "requires wb_mode manual"
                val k = number() ?: return "expected a number"
                val preset = CameraMath.nearestPreset(k, c.awbPresets) ?: return "not available"
                c.update { it.copy(awbPreset = preset) }
                val applied = CameraMath.PRESET_KELVIN.getValue(preset)
                // Camera2 offers presets, not any temperature.
                if (applied.toDouble() == k) null else "nearest preset $applied K"
            }
            "focus_mode" -> {
                val min = c.minFocusDiopters ?: return "fixed focus"
                val mode = when (value) {
                    "auto" -> FocusMode.AUTO
                    "locked" -> FocusMode.LOCKED
                    "manual" -> FocusMode.MANUAL
                    else -> return "expected auto, locked or manual"
                }
                // Locked and manual start at the distance auto focus found.
                val d = c.measured.focusDiopters
                c.update {
                    if (mode != FocusMode.AUTO && it.focusMode == FocusMode.AUTO)
                        it.copy(focusMode = mode, focusDiopters = (d ?: it.focusDiopters).coerceIn(0f, min), meteringPoint = null)
                    else it.copy(focusMode = mode, meteringPoint = if (mode == FocusMode.AUTO) it.meteringPoint else null)
                }
                null
            }
            "focus_position" -> {
                if (s.focusMode != FocusMode.MANUAL) return "requires focus_mode manual"
                val min = c.minFocusDiopters ?: return "fixed focus"
                val p = number() ?: return "expected a number"
                c.update { it.copy(focusDiopters = CameraMath.focusDiopters(p, min)) }
                if (p in 0.0..1.0) null else "clamped to 0..1"
            }
            else -> "Unknown control $id"
        }
    }

    /**
     * The phone's own mains frequency (flicker from mains-powered light): 50,
     * 60, or 0 for none (outdoors). It applies while no hub production does;
     * the production shutter follows it, and without a manual sensor
     * Camera2's anti-banding.
     */
    fun setMainsHz(hz: Int) {
        if (hz != 50 && hz != 60 && hz != 0) return
        ownMainsHz = hz
        if (serverShutter == null) applyProduction(hz, Shutter.Mains)
    }

    /** With a manual sensor the app sets the shutter itself: every frequency works. */
    fun mainsOptions(): List<Int> =
        if (camera.hasManualSensor) listOf(50, 60, 0)
        else listOf(50, 60, 0).filter { CameraCapture.antibandingMode(it) in camera.antibandingModes }

    /**
     * The control server's production (null, null: none, back to the
     * phone's own). While live it is kept until the stream stops; returns a
     * warning then.
     */
    fun setProduction(mainsHz: Int?, shutter: Shutter?, stopped: Boolean = false): String? {
        val s = camera.settings
        // A different production (or none again) replaces the phone's own
        // choice; a repeat of the same one keeps it.
        val production = mainsHz to shutter
        if (production != lastProduction) {
            lastProduction = production
            mainsOverride = null
        }
        // No mains in the message: the phone's own.
        val hz = mainsOverride ?: mainsHz?.takeIf { it == 50 || it == 60 || it == 0 } ?: ownMainsHz
        if (isLive() && !stopped) {
            if (hz != s.mainsHz || shutter != serverShutter) {
                // Warn once per pending production, not on every repeat of it (as on iOS).
                val next = mainsHz to shutter
                val changed = next != pendingProduction
                pendingProduction = next
                return if (changed) "production shutter changed while live: kept until the stream stops" else null
            }
            pendingProduction = null
            return null
        }
        pendingProduction = null
        val wasServer = serverShutter != null
        serverShutter = shutter
        val changed = applyProduction(hz, shutter ?: Shutter.Mains)
        // The shutter's `readonly` and options follow whether a hub production applies.
        if (!changed && wasServer != (shutter != null)) camera.onControlsChanged?.invoke()
        return null
    }

    /**
     * Sets the camera's mains and shutter policy. When the effective shutter
     * or the mains changes, the descriptors are sent again (the shutter's
     * `readonly` and options follow them). Returns whether they changed.
     */
    private fun applyProduction(hz: Int, shutter: Shutter): Boolean {
        val before = camera.settings
        camera.update { it.copy(mainsHz = hz, shutter = shutter) }
        val after = camera.settings
        val changed = before.mainsHz != after.mainsHz || before.effectiveShutter != after.effectiveShutter
        if (changed) camera.onControlsChanged?.invoke()
        return changed
    }

    /** Ids this table handles. */
    fun handles(id: String) = id in IDS

    /** Called when the stream stops: a production that came while live. */
    fun streamStopped() {
        pendingProduction?.let { (hz, shutter) -> setProduction(hz, shutter, stopped = true) }
    }

    companion object {
        val IDS = setOf("lens", "zoom", "resolution", "frame_rate", "stabilization", "torch", "exposure_mode", "iso",
                        "shutter", "genlock", "genlock_offset_us", "mains_hz", "ev_bias", "wb_mode", "wb_temperature", "focus_mode", "focus_position")

        const val NOT_WHILE_LIVE = "not while live: stop the stream to change it"
    }
}
