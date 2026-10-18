package se.eyevinn.tallycam.control

import org.json.JSONArray
import org.json.JSONObject
import se.eyevinn.tallycam.core.Hot
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
        // Manual holds the current values (docs/SHADING.md, D14): the sensor
        // values, else the AE lock with the ISO read-only.
        val modes = mutableListOf<Pair<Any, String>>("auto" to "Auto")
        if (c.canHoldExposure) modes += "manual" to "Manual"
        list += enumControl("exposure_mode", "Exposure", "exposure", options(*modes.toTypedArray()))
        if (c.canHoldExposure) {
            c.isoRange?.let {
                val iso = Controls.range("iso", "ISO", "exposure", it.lower.toDouble(), it.upper.toDouble(), 1.0, "")
                    .put("scale", "log").put("enabled_when", whenMode("exposure_mode", "manual"))
                if (!c.hasManualSensor) iso.put("readonly", true).put("readonly_reason", NO_MANUAL_EXPOSURE)
                list += iso
            }
        }
        if (c.hasManualSensor) {
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
        // White balance: manual is the core's gains for a temperature and
        // tint, else the AWB lock with both read-only. K and tint need the
        // camera's colour calibration, to set them or to report them.
        val wbModes = mutableListOf<Pair<Any, String>>("auto" to "Auto")
        if (c.canHoldWhiteBalance) wbModes += "manual" to "Manual"
        list += enumControl("wb_mode", "White balance", "white_balance", options(*wbModes.toTypedArray()))
        if (c.canHoldWhiteBalance && c.colorCalibration != null) {
            val temperature = Controls.range("wb_temperature", "Temperature", "white_balance", MIN_KELVIN, MAX_KELVIN,
                                             50.0, "K")
                .put("enabled_when", whenMode("wb_mode", "manual"))
            val tint = Controls.range("wb_tint", "Tint", "white_balance", -MAX_TINT, MAX_TINT, 1.0, "")
                .put("enabled_when", whenMode("wb_mode", "manual"))
            if (!c.hasManualWhiteBalance) {
                listOf(temperature, tint).forEach { it.put("readonly", true).put("readonly_reason", NO_MANUAL_WHITE_BALANCE) }
            }
            list += temperature
            list += tint
        }
        // Focus.
        if (c.minFocusDiopters != null) {
            list += enumControl("focus_mode", "Focus", "focus", options("auto" to "Auto", "manual" to "Manual"))
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
        // Without a manual sensor, the ISO Camera2's auto exposure uses (or held).
        if (!c.hasManualSensor && c.canHoldExposure && c.isoRange != null) m.iso?.let { v.put("iso", it) }
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
        // In both modes: the set values in manual, measured otherwise.
        c.reportedWhiteBalance()?.let { (k, tint) -> v.put("wb_temperature", k).put("wb_tint", tint) }
        c.minFocusDiopters?.let { min ->
            v.put("focus_mode", s.focusMode.name.lowercase())
            val diopters = if (s.focusMode == FocusMode.AUTO) m.focusDiopters ?: s.focusDiopters else s.focusDiopters
            v.put("focus_position", (CameraMath.focusPosition(diopters, min) * 100).roundToInt() / 100.0)
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
                // `auto_iso` is what `auto` was called before protocol 2.
                val manual = manualMode(if (value == "auto_iso") "auto" else value) ?: return EXPECTED_MODE
                if (manual && !c.canHoldExposure) return "manual exposure not supported"
                val mode = if (manual) ExposureMode.MANUAL else ExposureMode.AUTO
                // Manual starts from what auto had: with a manual sensor the
                // settings already hold it (the auto ISO loop's last values),
                // else the AE lock holds it. Auto starts with no bias.
                c.update { if (mode == ExposureMode.AUTO) it.copy(exposureMode = mode, evSteps = 0) else it.copy(exposureMode = mode) }
                null
            }
            "iso" -> {
                if (!c.hasManualSensor) return NO_MANUAL_EXPOSURE
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
                if (isLive()) return if (hz == s.mainsHz) null else NOT_WHILE_LIVE
                // From the hub, for this camera: an override for this
                // session only, never saved; a new production clears it.
                mainsOverride = hz
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
                val manual = manualMode(value) ?: return EXPECTED_MODE
                if (manual && !c.canHoldWhiteBalance) return "manual white balance not supported"
                // A→M starts from what auto white balance has, as K and tint.
                val wb = if (manual && s.whiteBalanceMode == WhiteBalanceMode.AUTO) c.currentWhiteBalance() else null
                c.update {
                    when {
                        !manual -> it.copy(whiteBalanceMode = WhiteBalanceMode.AUTO)
                        wb != null -> it.copy(whiteBalanceMode = WhiteBalanceMode.MANUAL, wbTemperatureK = wb.first, wbTint = wb.second)
                        else -> it.copy(whiteBalanceMode = WhiteBalanceMode.MANUAL)
                    }
                }
                null
            }
            "wb_temperature", "wb_tint" -> {
                if (!c.hasManualWhiteBalance) return NO_MANUAL_WHITE_BALANCE
                if (s.whiteBalanceMode != WhiteBalanceMode.MANUAL) return "requires wb_mode manual"
                val x = number() ?: return "expected a number"
                // Green tints are limited at low temperatures (the core's
                // max_tint): what is kept, and reported, is what applies.
                val clamped = if (id == "wb_temperature") x.coerceIn(MIN_KELVIN, MAX_KELVIN)
                              else x.coerceIn(-MAX_TINT, Hot.whiteBalanceMaxTint(s.wbTemperatureK))
                c.update {
                    if (id == "wb_temperature") {
                        it.copy(wbTemperatureK = clamped, wbTint = it.wbTint.coerceAtMost(Hot.whiteBalanceMaxTint(clamped)))
                    } else it.copy(wbTint = clamped)
                }
                if (clamped == x) null else "clamped to ${clamped.roundToInt()}"
            }
            "focus_mode" -> {
                val min = c.minFocusDiopters ?: return "fixed focus"
                val mode = if (manualMode(value) ?: return EXPECTED_MODE) FocusMode.MANUAL else FocusMode.AUTO
                // Manual starts at the distance auto focus found.
                val d = c.measured.focusDiopters
                c.update {
                    if (mode == FocusMode.MANUAL && it.focusMode == FocusMode.AUTO)
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
        if (serverShutter == null && mainsOverride == null) applyProduction(hz, Shutter.Mains)
    }

    /**
     * The operator's mains choice on the phone (Settings): saved as the
     * phone's own, and the override of the hub's production until the hub
     * sends a different one, also when it equals what applies. Null when it
     * applied, else why not.
     */
    fun applyOwnMains(hz: Int): String? {
        if (hz != 50 && hz != 60 && hz != 0) return "expected 50, 60 or 0 (off)"
        if (hz !in mainsOptions()) return "not supported by this camera"
        if (isLive() && hz != camera.settings.mainsHz) return NOT_WHILE_LIVE
        ownMainsHz = hz
        saveMainsHz(hz)
        mainsOverride = hz
        if (!isLive()) applyProduction(hz, serverShutter ?: Shutter.Mains)
        return null
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
                        "shutter", "genlock", "genlock_offset_us", "mains_hz", "ev_bias", "wb_mode", "wb_temperature",
                        "wb_tint", "focus_mode", "focus_position")

        const val NOT_WHILE_LIVE = "not while live: stop the stream to change it"
        const val NO_MANUAL_EXPOSURE = "no manual exposure on this camera"
        const val NO_MANUAL_WHITE_BALANCE = "no manual white balance on this camera"
        private const val EXPECTED_MODE = "expected auto or manual"

        /** As iOS and the core (AVFoundation's gains run out above 10000 K). */
        const val MIN_KELVIN = 2000.0
        const val MAX_KELVIN = 10000.0
        const val MAX_TINT = 150.0

        /**
         * A group's mode from a `set`: true for manual, false for auto, null
         * for neither. `locked`, from older servers, is manual at the current
         * values (docs/PROTOCOL.md, "Modes").
         */
        fun manualMode(value: Any?): Boolean? = when (value) {
            "auto" -> false
            "manual", "locked" -> true
            else -> null
        }
    }
}
