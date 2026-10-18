package se.eyevinn.tallycam.media

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.ImageFormat
import android.graphics.Rect
import android.graphics.SurfaceTexture
import android.graphics.YuvImage
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CameraMetadata
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.CaptureResult
import android.hardware.camera2.TotalCaptureResult
import android.hardware.camera2.params.ColorSpaceTransform
import android.hardware.camera2.params.MeteringRectangle
import android.hardware.camera2.params.OutputConfiguration
import android.hardware.camera2.params.RggbChannelVector
import android.hardware.camera2.params.SessionConfiguration
import android.media.ImageReader
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.util.Log
import android.util.Range
import android.util.Rational
import android.util.Size
import android.view.Surface
import se.eyevinn.tallycam.core.Hot
import se.eyevinn.tallycam.control.Settings as Prefs
import java.io.ByteArrayOutputStream
import java.util.concurrent.Executor
import kotlin.math.roundToLong

/**
 * A back camera through Camera2: a preview Surface, a small YUV reader for
 * the operator's preview images, and, while live, the encoder's input
 * Surface. Changing the outputs or the size rebuilds the session (a short
 * gap in the preview).
 *
 * The capture settings ([Settings]) are applied with every repeating
 * request; [measured] is what the camera used for the latest frame (ISO,
 * shutter, focus distance), which auto modes report. Everything that
 * touches the device runs on the camera thread.
 */
class CameraCapture(context: Context) {
    /**
     * Two modes per group (docs/SHADING.md, D14). Manual holds the camera's
     * current values; where the camera cannot set a group's value, manual
     * locks Camera2's auto instead (AE lock, AWB lock).
     */
    enum class ExposureMode { AUTO, MANUAL }
    enum class WhiteBalanceMode { AUTO, MANUAL }
    enum class FocusMode { AUTO, MANUAL }

    /** The production's shutter (docs/SHADING.md, "Shutter"). */
    sealed interface Shutter {
        /** One mains period: 1/50 at 50 Hz, 1/60 at 60 Hz (free when mains is off). */
        data object Mains : Shutter
        /** No mains-lit light: the shutter is an exposure control. */
        data object Free : Shutter
        /** A fixed shutter, 1/denominator s. */
        data class Fixed(val denominator: Int) : Shutter
    }

    /**
     * What the operator set. Copied, changed and applied through [update].
     * With a manual sensor the auto exposure of Camera2 is never used: auto
     * is the core's auto ISO loop at the production shutter, writing [iso]
     * and [exposureNs], so manual starts from what auto had.
     */
    data class Settings(
        val exposureMode: ExposureMode = ExposureMode.AUTO,
        val iso: Int = 400,
        val exposureNs: Long = 1_000_000_000L / 60,
        val evSteps: Int = 0,
        val whiteBalanceMode: WhiteBalanceMode = WhiteBalanceMode.AUTO,
        /** Manual white balance (with [hasManualWhiteBalance]): K and tint. */
        val wbTemperatureK: Double = 5500.0,
        val wbTint: Double = 0.0,
        val focusMode: FocusMode = FocusMode.AUTO,
        /** Diopters (0 = infinity), for manual focus. */
        val focusDiopters: Float = 0f,
        val zoom: Float = 1f,
        val stabilization: Boolean = false,
        val frameRate: Int = 30,
        val torch: Boolean = false,
        /**
         * Soft genlock: the frame duration follows the core's loop onto the
         * TAI grid. Needs manual exposure: Camera2 honours SENSOR_FRAME_DURATION
         * only with the auto exposure off.
         */
        val genlock: Boolean = false,
        /** Where frames land after each grid point, ns (per-model calibration). */
        val genlockOffsetNs: Long = 0,
        /** Mains frequency, 50 or 60 Hz, 0 for no mains-lit light (`mains_hz`). */
        val mainsHz: Int = 50,
        val shutter: Shutter = Shutter.Mains,
        /** Normalised point (0..1) for focus and exposure, or null for the whole frame. */
        val meteringPoint: Pair<Float, Float>? = null,
    ) {
        /** What the shutter follows now: mains without mains is free. */
        val effectiveShutter: Shutter get() = if (shutter == Shutter.Mains && mainsHz == 0) Shutter.Free else shutter
    }

    /** From the latest capture result. */
    data class Measured(val iso: Int?, val exposureNs: Long?, val focusDiopters: Float?, val frameDurationNs: Long?)

    /** One back camera the app can switch to. */
    data class Lens(val id: String, val key: String, val label: String, val focalLength: Float)

    private val manager = context.getSystemService(Context.CAMERA_SERVICE) as CameraManager
    private val thread = HandlerThread("camera").also { it.start() }
    private val handler = Handler(thread.looper)
    private val executor = Executor { handler.post(it) }
    private var device: CameraDevice? = null
    private var session: CameraCaptureSession? = null
    private var texture: SurfaceTexture? = null
    private var previewSurface: Surface? = null
    private var encoderSurface: Surface? = null
    private var thumbnails: ImageReader? = null
    private var afTrigger = false

    /** Back cameras, widest first, labelled ultra wide / wide / tele by focal length. */
    val lenses: List<Lens> = backLenses()

    @Volatile var cameraId: String? = lenses.firstOrNull { it.key == "wide" }?.id ?: lenses.firstOrNull()?.id
        ?: manager.cameraIdList.firstOrNull()
        private set

    /** The active camera's characteristics, cached (a binder call each otherwise; read per frame). */
    @Volatile private var cachedCharacteristics: Pair<String, CameraCharacteristics>? = null

    val characteristics: CameraCharacteristics?
        get() {
            val id = cameraId ?: return null
            cachedCharacteristics?.let { (cached, c) -> if (cached == id) return c }
            return manager.getCameraCharacteristics(id).also { cachedCharacteristics = id to it }
        }

    /** Run once the session no longer uses the surfaces it dropped (camera thread). */
    private val afterConfigure = mutableListOf<() -> Unit>()

    /** Genlock on/off and its offset (a per-model calibration) are kept across launches. */
    private val prefs = Prefs(context)

    @Volatile var settings = Settings(genlock = prefs.genlock,
                                      genlockOffsetNs = (prefs.genlockOffsetUs * 1000).roundToLong())
        private set

    @Volatile var measured = Measured(null, null, null, null)
        private set

    /** Genlock loop (camera thread) and the duration it asked for. */
    private var genlockLoop = 0L
    private var genlockKey: Triple<Int, Long, Size>? = null
    private var genlockDurationNs = 0L
    private var lastGenlockOffsetNs = 0L

    /** state, phase error ns, max phase error ns, frame duration ns, correction ppm; null while off. */
    @Volatile var genlockStatus: DoubleArray? = null
        private set

    /** The resolution asked for (shorter side, e.g. 1080); the stream size follows it. */
    @Volatile var wantedHeight: Int = if (VideoEncoder.hasHardwareH264()) 1080 else 720
        private set

    /** Called on the camera thread when the controls or their ranges change (lens, size). */
    @Volatile var onControlsChanged: (() -> Unit)? = null

    /** At most every this many ns a JPEG goes to `onThumbnail`; 0 for none. */
    @Volatile var thumbnailIntervalNs = 0L

    /**
     * Width of the preview JPEGs, 16 to 4096 (the height keeps the aspect
     * ratio). The frames are scaled down to it, never up, so the session
     * is not rebuilt for it.
     */
    @Volatile var thumbnailWidth = 480
        set(value) { field = value.coerceIn(16, 4096) }

    /** Receives preview JPEGs (on the preview thread); null stops them. */
    @Volatile var onThumbnail: ((ByteArray, Int, Int) -> Unit)? = null
    private var lastThumbNs = 0L
    /** JPEG compression runs here, off the camera thread. */
    private val previewThread = HandlerThread("preview-jpeg").also { it.start() }
    private val previewHandler = Handler(previewThread.looper)
    /** Reused NV21 frame; busy while the preview thread compresses it. */
    private var nv21 = ByteArray(0)
    private val nv21Busy = java.util.concurrent.atomic.AtomicBoolean(false)
    /** Reused scaled NV21 frame (preview thread, while [nv21Busy]). */
    private var scaledNv21 = ByteArray(0)
    /** Every Nth result updates [measured] (read four times a second). */
    private var resultCount = 0
    /** Two status arrays, alternated, so a reader never sees one being written. */
    private val genlockArrays = arrayOf(DoubleArray(5), DoubleArray(5))
    private var genlockArrayIndex = 0
    private val genlockMapping = LongArray(5)

    @Volatile var onError: ((String) -> Unit)? = null

    /** Auto ISO (camera thread): the core's loop, its shutter setup, and the latest frame's sensor values. */
    private var autoIso = 0L
    private var autoIsoSetup: Triple<Shutter, Int, Int>? = null
    private var autoIsoBias = Double.NaN
    private var autoIsoOn = false
    private var lastAutoIsoNs = 0L
    private var frameIso = 0
    private var frameExposureNs = 0L
    /** After a commanded ISO and exposure: skip samples until the frames report them, or until this time (ns). */
    private var autoIsoWaitIso = 0
    private var autoIsoWaitExposureNs = 0L
    private var autoIsoWaitUntilNs = 0L
    private val autoIsoInput = DoubleArray(8)
    private val autoIsoOutput = DoubleArray(2)
    private val autoIsoState = DoubleArray(4)

    /** AUTO_ISO_LIMIT_* of the auto ISO loop; [onAutoIsoLimit] runs when it changes (camera thread). */
    @Volatile var autoIsoLimit = Hot.AUTO_ISO_LIMIT_NONE
        private set
    @Volatile var onAutoIsoLimit: ((Int, Int) -> Unit)? = null

    /**
     * Image statistics (docs/SHADING.md) at most every this many ns, from
     * the small YUV stream: a grid sampled on the camera thread, measured by
     * the core on the analysis thread, handed to [onStats] there.
     */
    @Volatile var statsIntervalNs = 500_000_000L
    @Volatile var onStats: ((se.eyevinn.tallycam.core.ImageStatsResult) -> Unit)? = null
    private var lastStatsNs = 0L
    private val analysisThread = HandlerThread("analysis").also { it.start() }
    private val analysisHandler = Handler(analysisThread.looper)
    private val statsGrid = StatsGrid()
    private val statsBusy = java.util.concurrent.atomic.AtomicBoolean(false)

    /** Focus peaking masks (analysis thread), about 9 a second while set; null: off. */
    @Volatile var onPeaking: ((android.graphics.Bitmap) -> Unit)? = null
    private val peaking = Peaking()
    private val peakingBusy = java.util.concurrent.atomic.AtomicBoolean(false)
    private var lastPeakingNs = 0L

    /** Zoom ramp (camera thread): doublings per second, 0 for none. */
    private var zoomRate = 0.0

    /**
     * White balance as Camera2's own AWB has it (auto, or locked), from the
     * latest result in that mode: its gains (for K and tint) and its colour
     * transform, which manual keeps (the core's is less saturated than the
     * HAL's on a Pixel). Reset with the lens.
     */
    @Volatile private var autoGains: RggbChannelVector? = null
    @Volatile private var autoTransform: ColorSpaceTransform? = null

    /** K and tint of [autoGains], rounded (step 50, 1), at most twice a second; null without a calibration. */
    @Volatile var measuredWhiteBalance: Pair<Double, Double>? = null
        private set
    private var lastWhiteBalanceNs = 0L
    private val wbGains = DoubleArray(4)
    private val wbTemperatureTint = DoubleArray(2)

    /** Manual white balance from the core (camera thread), for the key it was computed for. */
    private val wbOut = DoubleArray(13)
    private var wbCount = 0
    private var wbKey: Triple<DoubleArray, Double, Double>? = null

    /** Whether the last request set the manual gains (else manual fell back to AWB lock). */
    @Volatile private var manualGainsApplied = false

    // MARK: Capabilities of the active camera

    private fun capabilities(): IntArray =
        characteristics?.get(CameraCharacteristics.REQUEST_AVAILABLE_CAPABILITIES) ?: IntArray(0)

    val hasManualSensor: Boolean
        get() = CameraMetadata.REQUEST_AVAILABLE_CAPABILITIES_MANUAL_SENSOR in capabilities()

    /** Manual white balance: raw gains and a colour transform, which needs a colour calibration. */
    val hasManualWhiteBalance: Boolean
        get() = CameraMetadata.REQUEST_AVAILABLE_CAPABILITIES_MANUAL_POST_PROCESSING in capabilities() &&
            colorCalibration != null &&
            characteristics?.get(CameraCharacteristics.CONTROL_AWB_AVAILABLE_MODES)
                ?.contains(CameraMetadata.CONTROL_AWB_MODE_OFF) == true

    val hasAeLock: Boolean
        get() = characteristics?.get(CameraCharacteristics.CONTROL_AE_LOCK_AVAILABLE) == true

    val hasAwbLock: Boolean
        get() = characteristics?.get(CameraCharacteristics.CONTROL_AWB_LOCK_AVAILABLE) == true

    /** Exposure can be held: sensor values, else the AE lock. */
    val canHoldExposure: Boolean get() = hasManualSensor || hasAeLock

    /** White balance can be held: manual gains, else the AWB lock. */
    val canHoldWhiteBalance: Boolean get() = hasManualWhiteBalance || hasAwbLock

    /** The active camera's colour calibration ([ColorCalibration]), cached per camera; null: none. */
    @Volatile private var cachedCalibration: Pair<String, DoubleArray?>? = null

    val colorCalibration: DoubleArray?
        get() {
            val id = cameraId ?: return null
            cachedCalibration?.let { (cached, cal) -> if (cached == id) return cal }
            val c = characteristics ?: return null
            fun matrix(key: CameraCharacteristics.Key<ColorSpaceTransform>): DoubleArray? = c.get(key)?.let { t ->
                val r = arrayOfNulls<Rational>(9)
                t.copyElements(r, 0)
                DoubleArray(9) { r[it]?.toDouble() ?: 0.0 }
            }
            val cal = ColorCalibration.pack(
                c.get(CameraCharacteristics.SENSOR_REFERENCE_ILLUMINANT1),
                c.get(CameraCharacteristics.SENSOR_REFERENCE_ILLUMINANT2)?.toInt()?.and(0xff),
                matrix(CameraCharacteristics.SENSOR_COLOR_TRANSFORM1),
                matrix(CameraCharacteristics.SENSOR_COLOR_TRANSFORM2),
                matrix(CameraCharacteristics.SENSOR_CALIBRATION_TRANSFORM1),
                matrix(CameraCharacteristics.SENSOR_CALIBRATION_TRANSFORM2),
                matrix(CameraCharacteristics.SENSOR_FORWARD_MATRIX1),
                matrix(CameraCharacteristics.SENSOR_FORWARD_MATRIX2),
            )
            cachedCalibration = id to cal
            return cal
        }

    val hasTorch: Boolean
        get() = characteristics?.get(CameraCharacteristics.FLASH_INFO_AVAILABLE) == true

    val isoRange: Range<Int>?
        get() = characteristics?.get(CameraCharacteristics.SENSOR_INFO_SENSITIVITY_RANGE)

    val exposureRange: Range<Long>?
        get() = characteristics?.get(CameraCharacteristics.SENSOR_INFO_EXPOSURE_TIME_RANGE)

    /** Exposure compensation in steps, and the step in EV. */
    val evRange: Pair<Range<Int>, Float>?
        get() {
            val c = characteristics ?: return null
            val range = c.get(CameraCharacteristics.CONTROL_AE_COMPENSATION_RANGE) ?: return null
            val step = c.get(CameraCharacteristics.CONTROL_AE_COMPENSATION_STEP)?.toFloat() ?: return null
            return if (range.lower == range.upper) null else range to step
        }

    /** Closest focus in diopters; null for a fixed-focus lens. */
    val minFocusDiopters: Float?
        get() = characteristics?.get(CameraCharacteristics.LENS_INFO_MINIMUM_FOCUS_DISTANCE)?.takeIf { it > 0f }

    val zoomRange: Range<Float>?
        get() = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            characteristics?.get(CameraCharacteristics.CONTROL_ZOOM_RATIO_RANGE)?.takeIf { it.upper > it.lower }
        } else null

    val hasStabilization: Boolean
        get() = characteristics?.get(CameraCharacteristics.CONTROL_AVAILABLE_VIDEO_STABILIZATION_MODES)
            ?.contains(CameraMetadata.CONTROL_VIDEO_STABILIZATION_MODE_ON) == true

    val antibandingModes: List<Int>
        get() = characteristics?.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_ANTIBANDING_MODES)?.toList().orEmpty()

    /** Fixed frame rates (AE target ranges with equal ends), lowest first. */
    val frameRates: List<Int>
        get() = characteristics?.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES)
            ?.filter { it.lower == it.upper }?.map { it.upper }?.distinct()?.sorted().orEmpty()
            .ifEmpty { listOf(30) }

    private val sizes get() = characteristics?.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)

    /** 16:9 sizes the camera gives both an encoder and a preview. */
    private fun streamSizes(): List<Size> {
        val map = sizes ?: return emptyList()
        val encoder = map.getOutputSizes(android.media.MediaCodec::class.java)?.toList().orEmpty()
        val preview = map.getOutputSizes(SurfaceTexture::class.java)?.toList().orEmpty()
        return encoder.filter { it in preview && it.width * 9 == it.height * 16 }
    }

    /** 720, 1080, 2160 where the camera has them in 16:9. */
    val resolutions: List<Int>
        get() = streamSizes().map { it.height }.filter { it in listOf(720, 1080, 2160) }.distinct().sorted()

    /** The stream's size: the wanted 16:9 height, else the nearest smaller, else anything. */
    val streamSize: Size
        get() {
            val all = streamSizes()
            return all.filter { it.height <= wantedHeight }.maxByOrNull { it.height }
                ?: all.minByOrNull { it.height }
                ?: sizes?.getOutputSizes(android.media.MediaCodec::class.java)?.maxByOrNull { it.width * it.height }
                ?: Size(1280, 720)
        }

    /** The sensor's timestamps in CLOCK_BOOTTIME (else CLOCK_MONOTONIC). */
    val timestampsAreBootTime: Boolean
        get() = characteristics?.get(CameraCharacteristics.SENSOR_INFO_TIMESTAMP_SOURCE) ==
            CameraCharacteristics.SENSOR_INFO_TIMESTAMP_SOURCE_REALTIME

    /** Added to a sensor timestamp to get CLOCK_BOOTTIME ns. */
    fun timestampOffsetNs(): Long =
        if (timestampsAreBootTime) 0 else SystemClock.elapsedRealtimeNanos() - System.nanoTime()

    // MARK: Changing things

    /**
     * Changes the settings at once (so the next control in the same `set`
     * sees them) and applies them with the next request.
     */
    fun update(change: (Settings) -> Settings) {
        change(change = change)
        handler.post { repeat() }
    }

    /** [refit]: fit the shutter whatever changed (another lens has other ranges). */
    private fun change(refit: Boolean = false, change: (Settings) -> Settings) {
        val (old, new) = synchronized(this) {
            val before = settings
            val after = change(before)
            // Only when what the shutter follows changed: a zoom tick or a
            // focus must not undo the auto ISO loop's shorter shutter. Nor
            // must leaving auto: locked and manual start from what auto had
            // (1/100 at the lowest ISO); entering auto starts from the
            // production shutter again.
            val leftAutoOnly = after.exposureMode != before.exposureMode &&
                after.exposureMode != ExposureMode.AUTO &&
                fitKey(before.copy(exposureMode = after.exposureMode)) == fitKey(after)
            val fit = refit || (fitKey(after) != fitKey(before) && !leftAutoOnly)
            settings = if (fit) fitShutter(after) else after
            before to settings
        }
        if (new.genlock != old.genlock) prefs.genlock = new.genlock
        if (new.genlockOffsetNs != old.genlockOffsetNs) prefs.genlockOffsetUs = new.genlockOffsetNs / 1000.0
    }

    /** What [fitShutter] depends on: the production, the frame rate (and genlock's room) and the exposure mode. */
    private fun fitKey(s: Settings) = listOf(s.effectiveShutter, s.mainsHz, s.frameRate, s.genlock, s.exposureMode)

    /**
     * With a manual sensor, puts the exposure at the production shutter,
     * moving the ISO so the picture keeps its brightness; under a free
     * shutter only within the frame.
     */
    private fun fitShutter(s: Settings): Settings {
        if (!hasManualSensor) return s
        val target = productionExposureNs(s)
        if (target == null) {
            val longest = longestExposureNs(s).toLong()
            return if (s.exposureNs > longest) s.copy(exposureNs = longest) else s
        }
        if (kotlin.math.abs(target.toDouble() / s.exposureNs - 1) <= 0.005) return s
        val range = isoRange
        val iso = (s.iso.toDouble() * s.exposureNs / target).toInt()
        return s.copy(exposureNs = target, iso = range?.let { iso.coerceIn(it.lower, it.upper) } ?: iso)
    }

    private fun longestExposureNs(s: Settings): Double =
        Hot.longestExposureNs(1e9 / s.frameRate, (exposureRange?.upper ?: 1_000_000_000L).toDouble(), s.genlock)

    /**
     * The production's exposure for the frame rate, or null under a free
     * shutter. A shutter longer than the frame becomes the longest
     * flicker-free one that fits (1/100 for 1/50 at 60 fps), as on iOS.
     */
    fun productionExposureNs(s: Settings = settings): Long? {
        val wanted = when (val shutter = s.effectiveShutter) {
            Shutter.Free -> return null
            Shutter.Mains -> 1e9 / s.mainsHz
            is Shutter.Fixed -> 1e9 / shutter.denominator
        }
        val shortest = (exposureRange?.lower ?: 0L).toDouble()
        val longest = longestExposureNs(s)
        if (wanted <= longest) return maxOf(wanted, shortest).toLong()
        if (s.mainsHz > 0) {
            val pulse = 1e9 / (2 * s.mainsHz)
            val pulses = kotlin.math.floor(longest / pulse)
            if (pulses >= 1) return maxOf(pulses * pulse, shortest).toLong()
        }
        return maxOf(longest, shortest).toLong()
    }

    /** One autofocus (and exposure metering) at a point, then continuous focus there. */
    fun focusAt(x: Float, y: Float) {
        handler.post {
            change { it.copy(meteringPoint = x.coerceIn(0f, 1f) to y.coerceIn(0f, 1f), focusMode = FocusMode.AUTO) }
            afTrigger = true
            repeat()
        }
    }

    /** Lets auto white balance settle, then manual at what it measured (K and tint). */
    fun whiteBalanceOnce(done: () -> Unit) {
        handler.post {
            change { it.copy(whiteBalanceMode = WhiteBalanceMode.AUTO) }
            repeat()
            handler.postDelayed({
                val wb = currentWhiteBalance()
                change { s ->
                    wb?.let { (k, tint) -> s.copy(whiteBalanceMode = WhiteBalanceMode.MANUAL, wbTemperatureK = k, wbTint = tint) }
                        ?: s.copy(whiteBalanceMode = WhiteBalanceMode.MANUAL)
                }
                repeat()
                done()
            }, 700)
        }
    }

    /**
     * K and tint (rounded, step 50 and 1) of Camera2's own AWB now, from
     * the latest auto result, unthrottled; null without a calibration or a
     * result. Any thread (allocates).
     */
    fun currentWhiteBalance(): Pair<Double, Double>? {
        val cal = colorCalibration ?: return null
        val gains = autoGains ?: return null
        val out = DoubleArray(2)
        val input = doubleArrayOf(gains.red.toDouble(), gains.greenEven.toDouble(), gains.greenOdd.toDouble(),
                                  gains.blue.toDouble())
        if (!Hot.whiteBalanceTemperatureTint(cal, input, out)) return null
        return ColorCalibration.roundedKelvin(out[0]) to ColorCalibration.roundedTint(out[1])
    }

    /**
     * The white balance to report: the set K and tint while manual gains
     * apply, else what Camera2's AWB has (auto, or locked).
     */
    fun reportedWhiteBalance(): Pair<Double, Double>? {
        val s = settings
        if (s.whiteBalanceMode == WhiteBalanceMode.MANUAL && manualGainsApplied) {
            return ColorCalibration.roundedKelvin(s.wbTemperatureK) to ColorCalibration.roundedTint(s.wbTint)
        }
        return measuredWhiteBalance
    }

    /** Switches to another back camera (it reopens; settings that the new lens lacks fall back). */
    fun switchLens(id: String) {
        handler.post {
            if (id == cameraId) return@post
            session?.close()
            session = null
            device?.close()
            device = null
            cameraId = id
            // Another camera, another calibration and AWB.
            autoGains = null
            autoTransform = null
            measuredWhiteBalance = null
            lastWhiteBalanceNs = 0L
            wbKey = null
            change(refit = true) { it.copy(zoom = 1f, meteringPoint = null) }
            // Also without a preview (streaming in the background).
            if (texture != null || encoderSurface != null) openDevice()
            onControlsChanged?.invoke()
        }
    }

    /** The resolution for the next stream (the preview follows at once). */
    fun setResolution(height: Int) {
        handler.post {
            wantedHeight = height
            texture?.setDefaultBufferSize(streamSize.width, streamSize.height)
            configure()
            onControlsChanged?.invoke()
        }
    }

    /** Opens the camera with the preview; call again after a new texture. */
    fun open(texture: SurfaceTexture) {
        handler.post { openOnThread(texture) }
    }

    /**
     * The preview went away (app in the background): keep streaming without
     * it, or close the camera when not streaming. `released` runs once the
     * camera no longer renders into the texture, which may then be freed.
     */
    fun detachPreview(released: () -> Unit = {}) {
        handler.post {
            val surface = previewSurface
            texture = null
            previewSurface = null
            afterConfigure += {
                surface?.release()
                released()
            }
            if (encoderSurface != null) configure() else closeDevice()
        }
    }

    /**
     * The encoder's Surface, or null to go back to preview only. `done` runs
     * (camera thread) once the session no longer renders into the surface
     * it dropped, so the encoder may release it.
     */
    fun setEncoderSurface(surface: Surface?, done: () -> Unit = {}) {
        handler.post {
            encoderSurface = surface
            afterConfigure += done
            configure()
        }
    }

    fun close() {
        handler.post { closeDevice() }
    }

    private fun closeDevice() {
        session?.close()
        session = null
        device?.close()
        device = null
        // No more frames: the core's loops start over when the camera opens again.
        freeGenlock()
        freeAutoIso()
        runAfterConfigure()
    }

    private fun runAfterConfigure() {
        if (afterConfigure.isEmpty()) return
        val pending = afterConfigure.toList()
        afterConfigure.clear()
        pending.forEach { runCatching(it).onFailure { e -> Log.w(TAG, "after configure", e) } }
    }

    // MARK: Camera thread

    private fun openOnThread(texture: SurfaceTexture) {
        this.texture = texture
        texture.setDefaultBufferSize(streamSize.width, streamSize.height)
        previewSurface = Surface(texture)
        openDevice()
    }

    /** Opens the camera, or reconfigures it when open (with or without a preview). */
    @SuppressLint("MissingPermission")
    private fun openDevice() {
        val id = cameraId ?: run {
            onError?.invoke("No camera")
            return
        }
        if (device != null) {
            configure()
            return
        }
        try {
            manager.openCamera(id, executor, object : CameraDevice.StateCallback() {
                override fun onOpened(camera: CameraDevice) {
                    device = camera
                    configure()
                }

                override fun onDisconnected(camera: CameraDevice) {
                    // Another app took the camera, or it went away: say so,
                    // and while streaming try to get it back.
                    camera.close()
                    if (device === camera) device = null
                    session = null
                    runAfterConfigure()
                    if (encoderSurface != null) {
                        onError?.invoke("Camera taken by another app, retrying")
                        handler.postDelayed({ if (device == null && encoderSurface != null) openDevice() }, 1000)
                    }
                }

                override fun onError(camera: CameraDevice, error: Int) {
                    camera.close()
                    if (device === camera) device = null
                    session = null
                    runAfterConfigure()
                    onError?.invoke("Camera error $error")
                }
            })
        } catch (e: Exception) {
            onError?.invoke("Cannot open the camera: ${e.message}")
        }
    }

    private fun configure() {
        val device = device ?: return runAfterConfigure()
        session?.close()
        session = null
        val thumbSize = thumbSize()
        if (thumbnails?.width != thumbSize.width || thumbnails?.height != thumbSize.height) {
            thumbnails?.close()
            thumbnails = ImageReader.newInstance(thumbSize.width, thumbSize.height, ImageFormat.YUV_420_888, 2)
                .also { it.setOnImageAvailableListener(::onThumbnailImage, handler) }
        }
        val outputs = listOfNotNull(previewSurface, encoderSurface, thumbnails?.surface).map(::OutputConfiguration)
        try {
            device.createCaptureSession(SessionConfiguration(SessionConfiguration.SESSION_REGULAR, outputs, executor,
                object : CameraCaptureSession.StateCallback() {
                    override fun onConfigured(s: CameraCaptureSession) {
                        session = s
                        repeat()
                        // The previous session is closed: dropped surfaces are free.
                        runAfterConfigure()
                    }

                    override fun onConfigureFailed(s: CameraCaptureSession) {
                        onError?.invoke("Camera session failed")
                        runAfterConfigure()
                    }
                }))
        } catch (e: Exception) {
            Log.e(TAG, "Camera session failed (${outputs.size} outputs, stream $streamSize, preview images $thumbSize)", e)
            onError?.invoke("Camera session failed: ${e.message}")
            runAfterConfigure()
        }
    }

    private fun repeat() {
        val device = device ?: return
        val session = session ?: return
        val s = settings
        val builder = try {
            device.createCaptureRequest(
                if (encoderSurface != null) CameraDevice.TEMPLATE_RECORD else CameraDevice.TEMPLATE_PREVIEW
            )
        } catch (e: Exception) {
            return
        }
        listOfNotNull(previewSurface, encoderSurface).forEach(builder::addTarget)
        builder.apply(s)
        try {
            if (afTrigger) {
                afTrigger = false
                builder.set(CaptureRequest.CONTROL_AF_TRIGGER, CameraMetadata.CONTROL_AF_TRIGGER_START)
                session.capture(builder.build(), null, handler)
                builder.set(CaptureRequest.CONTROL_AF_TRIGGER, CameraMetadata.CONTROL_AF_TRIGGER_IDLE)
            }
            // The small YUV stream (auto ISO metering, image statistics,
            // previews) on about 10 frames a second, not every frame: a
            // repeating burst where one request in `every` also targets it.
            val yuv = thumbnails?.surface
            val every = (s.frameRate / 10).coerceAtLeast(1)
            if (yuv == null) {
                session.setRepeatingRequest(builder.build(), resultCallback, handler)
            } else if (every == 1) {
                builder.addTarget(yuv)
                session.setRepeatingRequest(builder.build(), resultCallback, handler)
            } else {
                val plain = builder.build()
                builder.addTarget(yuv)
                val sampled = builder.build()
                session.setRepeatingBurst(List(every) { if (it == 0) sampled else plain }, resultCallback, handler)
            }
        } catch (e: Exception) {
            Log.w(TAG, "Repeating request failed", e)
        }
    }

    private fun CaptureRequest.Builder.apply(s: Settings) {
        set(CaptureRequest.CONTROL_MODE, CameraMetadata.CONTROL_MODE_AUTO)
        set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, Range(s.frameRate, s.frameRate))
        antibandingMode(s.mainsHz).takeIf { it in antibandingModes }
            ?.let { set(CaptureRequest.CONTROL_AE_ANTIBANDING_MODE, it) }
        set(CaptureRequest.CONTROL_VIDEO_STABILIZATION_MODE,
            if (s.stabilization && hasStabilization) CameraMetadata.CONTROL_VIDEO_STABILIZATION_MODE_ON
            else CameraMetadata.CONTROL_VIDEO_STABILIZATION_MODE_OFF)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R && zoomRange != null) {
            set(CaptureRequest.CONTROL_ZOOM_RATIO, s.zoom)
        }
        val region = s.meteringPoint?.let { meteringRegion(it) }
        // Exposure: with a manual sensor always the sensor values (auto is
        // the core's auto ISO loop writing them), else Camera2's auto exposure.
        if (hasManualSensor) {
            set(CaptureRequest.CONTROL_AE_MODE, CameraMetadata.CONTROL_AE_MODE_OFF)
            set(CaptureRequest.SENSOR_SENSITIVITY, s.iso)
            set(CaptureRequest.SENSOR_EXPOSURE_TIME, s.exposureNs)
            val duration = if (s.genlock && genlockDurationNs > 0) genlockDurationNs else 1_000_000_000L / s.frameRate
            set(CaptureRequest.SENSOR_FRAME_DURATION, duration)
        } else when (s.exposureMode) {
            // Held: Camera2's auto exposure locked where it is.
            ExposureMode.MANUAL -> {
                set(CaptureRequest.CONTROL_AE_MODE, CameraMetadata.CONTROL_AE_MODE_ON)
                set(CaptureRequest.CONTROL_AE_LOCK, true)
            }
            ExposureMode.AUTO -> {
                set(CaptureRequest.CONTROL_AE_MODE, CameraMetadata.CONTROL_AE_MODE_ON)
                set(CaptureRequest.CONTROL_AE_LOCK, false)
                set(CaptureRequest.CONTROL_AE_EXPOSURE_COMPENSATION, s.evSteps)
                region?.let { set(CaptureRequest.CONTROL_AE_REGIONS, arrayOf(it)) }
            }
        }
        // White balance: manual is the core's gains for K and tint where the
        // camera takes them, else the AWB lock.
        val manualGains = s.whiteBalanceMode == WhiteBalanceMode.MANUAL && setManualWhiteBalance(s)
        manualGainsApplied = manualGains
        if (!manualGains) {
            set(CaptureRequest.CONTROL_AWB_MODE, CameraMetadata.CONTROL_AWB_MODE_AUTO)
            set(CaptureRequest.CONTROL_AWB_LOCK, s.whiteBalanceMode == WhiteBalanceMode.MANUAL)
        }
        // Focus: manual is the lens at a distance (A→M starts where auto focus was).
        when (s.focusMode) {
            FocusMode.AUTO -> {
                set(CaptureRequest.CONTROL_AF_MODE,
                    if (region != null) CameraMetadata.CONTROL_AF_MODE_AUTO else CameraMetadata.CONTROL_AF_MODE_CONTINUOUS_VIDEO)
                region?.let { set(CaptureRequest.CONTROL_AF_REGIONS, arrayOf(it)) }
            }
            FocusMode.MANUAL -> if (minFocusDiopters != null) {
                set(CaptureRequest.CONTROL_AF_MODE, CameraMetadata.CONTROL_AF_MODE_OFF)
                set(CaptureRequest.LENS_FOCUS_DISTANCE, s.focusDiopters)
            }
        }
        if (hasTorch) {
            set(CaptureRequest.FLASH_MODE, if (s.torch) CameraMetadata.FLASH_MODE_TORCH else CameraMetadata.FLASH_MODE_OFF)
        }
    }

    /**
     * Camera thread: AWB off, the core's gains for the settings' K and tint,
     * and the colour transform of the last auto result (the core's only
     * before there is one). False when it cannot (no manual white balance,
     * no gains, no transform): the caller locks the AWB instead.
     */
    private fun CaptureRequest.Builder.setManualWhiteBalance(s: Settings): Boolean {
        if (!hasManualWhiteBalance) return false
        val cal = colorCalibration ?: return false
        val key = Triple(cal, s.wbTemperatureK, s.wbTint)
        if (key != wbKey) {
            wbKey = key
            wbCount = Hot.whiteBalanceFor(cal, s.wbTemperatureK, s.wbTint, wbOut)
        }
        if (wbCount < 4) return false
        val transform = autoTransform
            ?: if (wbCount == 13) ColorSpaceTransform(ColorCalibration.transformElements(wbOut, 4)) else return false
        set(CaptureRequest.CONTROL_AWB_MODE, CameraMetadata.CONTROL_AWB_MODE_OFF)
        set(CaptureRequest.COLOR_CORRECTION_MODE, CameraMetadata.COLOR_CORRECTION_MODE_TRANSFORM_MATRIX)
        set(CaptureRequest.COLOR_CORRECTION_GAINS,
            RggbChannelVector(wbOut[0].toFloat(), wbOut[1].toFloat(), wbOut[2].toFloat(), wbOut[3].toFloat()))
        set(CaptureRequest.COLOR_CORRECTION_TRANSFORM, transform)
        return true
    }

    /**
     * Camera thread, every fourth result: a result of Camera2's own AWB
     * (auto or locked, not the manual gains) gives the gains and transform
     * manual starts from, and twice a second the K and tint auto reports.
     */
    private fun sampleWhiteBalance(result: TotalCaptureResult) {
        if (result.get(CaptureResult.CONTROL_AWB_MODE) != CameraMetadata.CONTROL_AWB_MODE_AUTO) return
        val gains = result.get(CaptureResult.COLOR_CORRECTION_GAINS) ?: return
        autoGains = gains
        result.get(CaptureResult.COLOR_CORRECTION_TRANSFORM)?.let { autoTransform = it }
        val now = SystemClock.elapsedRealtimeNanos()
        if (now - lastWhiteBalanceNs < 500_000_000L) return
        lastWhiteBalanceNs = now
        val cal = colorCalibration ?: return
        wbGains[0] = gains.red.toDouble()
        wbGains[1] = gains.greenEven.toDouble()
        wbGains[2] = gains.greenOdd.toDouble()
        wbGains[3] = gains.blue.toDouble()
        if (!Hot.whiteBalanceTemperatureTint(cal, wbGains, wbTemperatureTint)) return
        val wb = ColorCalibration.roundedKelvin(wbTemperatureTint[0]) to ColorCalibration.roundedTint(wbTemperatureTint[1])
        // Only when the rounded values change.
        if (wb != measuredWhiteBalance) measuredWhiteBalance = wb
    }

    /** A metering rectangle of a tenth of the active array around a normalised point. */
    private fun meteringRegion(point: Pair<Float, Float>): MeteringRectangle? {
        val array = characteristics?.get(CameraCharacteristics.SENSOR_INFO_ACTIVE_ARRAY_SIZE) ?: return null
        val w = array.width() / 10
        val h = array.height() / 10
        val cx = (array.left + point.first * array.width()).toInt()
        val cy = (array.top + point.second * array.height()).toInt()
        val left = (cx - w / 2).coerceIn(array.left, array.right - w)
        val top = (cy - h / 2).coerceIn(array.top, array.bottom - h)
        return MeteringRectangle(left, top, w, h, MeteringRectangle.METERING_WEIGHT_MAX)
    }

    private val resultCallback = object : CameraCaptureSession.CaptureCallback() {
        override fun onCaptureCompleted(s: CameraCaptureSession, request: CaptureRequest, result: TotalCaptureResult) {
            if (autoIsoOn) {
                frameIso = result.get(CaptureResult.SENSOR_SENSITIVITY) ?: frameIso
                frameExposureNs = result.get(CaptureResult.SENSOR_EXPOSURE_TIME) ?: frameExposureNs
            }
            if (resultCount++ % 4 == 0) {
                measured = Measured(
                    result.get(CaptureResult.SENSOR_SENSITIVITY),
                    result.get(CaptureResult.SENSOR_EXPOSURE_TIME),
                    result.get(CaptureResult.LENS_FOCUS_DISTANCE),
                    result.get(CaptureResult.SENSOR_FRAME_DURATION),
                )
                sampleWhiteBalance(result)
            }
            if (genlockLoop != 0L || settings.genlock) {
                result.get(CaptureResult.SENSOR_TIMESTAMP)?.let(::genlockFrame)
            }
        }
    }

    /** Camera thread: one frame's start of exposure, sensor clock. */
    private fun genlockFrame(sensorNs: Long) {
        val s = settings
        val active = s.genlock && hasManualSensor
        if (!active) {
            if (genlockLoop != 0L) {
                freeGenlock()
                repeat()
            }
            return
        }
        val minFrame = sizes?.getOutputMinFrameDuration(android.media.MediaCodec::class.java, streamSize) ?: 0L
        val key = Triple(s.frameRate, minFrame, streamSize)
        if (key != genlockKey) {
            if (genlockLoop != 0L) Hot.genlockFree(genlockLoop)
            genlockLoop = Hot.genlockNew(s.frameRate, 1, minFrame.toDouble())
            genlockKey = key
            genlockDurationNs = 0
            lastGenlockOffsetNs = Long.MIN_VALUE
        }
        if (genlockLoop == 0L) return
        if (s.genlockOffsetNs != lastGenlockOffsetNs) {
            Hot.genlockSetTargetOffset(genlockLoop, s.genlockOffsetNs)
            lastGenlockOffsetNs = s.genlockOffsetNs
        }
        Hot.clockMap(sensorNs + timestampOffsetNs(), genlockMapping)
        val duration = Hot.genlockOnFrame(genlockLoop, genlockMapping[0], genlockMapping[4] and 1L != 0L)
        genlockArrayIndex = 1 - genlockArrayIndex
        val status = genlockArrays[genlockArrayIndex]
        Hot.genlockStatus(genlockLoop, status)
        genlockStatus = status
        if (duration > 0) {
            genlockDurationNs = duration.toLong()
            repeat()
        }
    }

    /** Camera thread: frees the genlock loop, if any; the next frame makes a new one. */
    private fun freeGenlock() {
        if (genlockLoop == 0L) return
        Hot.genlockFree(genlockLoop)
        genlockLoop = 0
        genlockKey = null
        genlockDurationNs = 0
        genlockStatus = null
    }

    /** Camera thread: frees the auto ISO loop, if any; auto exposure makes a new one. */
    private fun freeAutoIso() {
        autoIsoOn = false
        if (autoIso == 0L) return
        Hot.autoIsoFree(autoIso)
        autoIso = 0
        autoIsoSetup = null
        // The new loop has no bias yet.
        autoIsoBias = Double.NaN
    }

    private fun thumbSize(): Size {
        val yuv = sizes?.getOutputSizes(ImageFormat.YUV_420_888)?.toList().orEmpty()
        // 16:9 like the picture, so masks and statistics cover what is streamed.
        val wide = yuv.filter { it.width * 9 == it.height * 16 }
        return yuv.firstOrNull { it.width == 640 && it.height == 360 }
            ?: wide.filter { it.width >= 320 }.minByOrNull { it.width * it.height }
            ?: yuv.filter { it.width >= 320 }.minByOrNull { it.width * it.height }
            ?: Size(640, 480)
    }

    /**
     * Camera thread: copies the frame into the reused NV21 buffer (bulk row
     * copies) when a preview image is due, and compresses it on the preview
     * thread. A frame that comes while the last one is still compressing is
     * skipped.
     */
    private fun onThumbnailImage(reader: ImageReader) {
        val image = reader.acquireLatestImage() ?: return
        try {
            val now = SystemClock.elapsedRealtimeNanos()
            stepAutoIso(image, now)
            sampleStats(image, now)
            samplePeaking(image, now)
            val interval = thumbnailIntervalNs
            val sink = onThumbnail
            if (sink == null || interval <= 0 || now - lastThumbNs < interval) return
            if (!nv21Busy.compareAndSet(false, true)) return
            lastThumbNs = now
            val w = image.width
            val h = image.height
            if (nv21.size != w * h * 3 / 2) nv21 = ByteArray(w * h * 3 / 2)
            Yuv.toNv21(image, nv21)
            val frame = nv21
            val (sw, sh) = Yuv.scaledSize(w, h, thumbnailWidth)
            previewHandler.post {
                try {
                    if (sw == w && sh == h) {
                        sink(Yuv.compress(frame, w, h), w, h)
                    } else {
                        if (scaledNv21.size != sw * sh * 3 / 2) scaledNv21 = ByteArray(sw * sh * 3 / 2)
                        Yuv.scale(frame, w, h, scaledNv21, sw, sh)
                        sink(Yuv.compress(scaledNv21, sw, sh), sw, sh)
                    }
                } finally {
                    nv21Busy.set(false)
                }
            }
        } finally {
            image.close()
        }
    }

    /**
     * Camera thread, 8 times a second: the metered exposure of this frame
     * to the core's auto ISO loop, and what it returns to the sensor.
     * Camera2 gives no metered offset with its auto exposure off, so the
     * metering is the picture's mean luma against a mid grey.
     */
    private fun stepAutoIso(image: android.media.Image, now: Long) {
        val s = settings
        val on = s.exposureMode == ExposureMode.AUTO && hasManualSensor
        if (!on) {
            freeAutoIso()
            return
        }
        if (autoIso == 0L) autoIso = Hot.autoIsoNew(0)
        if (!autoIsoOn) {
            autoIsoOn = true
            frameIso = 0
            // Set up again: the production may have changed while off.
            autoIsoSetup = null
        }
        val setup = Triple(s.effectiveShutter, s.mainsHz, s.frameRate)
        if (setup != autoIsoSetup) {
            autoIsoSetup = setup
            // A new production or frame rate: start the loop over, from the
            // values the settings now hold (fitShutter may have moved them).
            Hot.autoIsoReset(autoIso)
            awaitFrame(s.iso, s.exposureNs, now)
            when (setup.first) {
                Shutter.Mains -> {
                    Hot.autoIsoSetMains(autoIso, s.mainsHz)
                    Hot.autoIsoSetFreeShutter(autoIso, 0.0)
                }
                // The production chose this value on purpose: no rounding.
                is Shutter.Fixed -> {
                    Hot.autoIsoSetMains(autoIso, 0)
                    Hot.autoIsoSetFreeShutter(autoIso, 0.0)
                }
                // A 180 degree shutter (half the frame, 1/60 at 30 fps) as
                // the base, never an exposure the loop already shortened.
                Shutter.Free -> {
                    Hot.autoIsoSetMains(autoIso, 0)
                    Hot.autoIsoSetFreeShutter(autoIso, 0.5e9 / s.frameRate)
                }
            }
        }
        val bias = evRange?.let { (_, step) -> s.evSteps * step.toDouble() } ?: 0.0
        if (bias != autoIsoBias) {
            autoIsoBias = bias
            Hot.autoIsoSetBias(autoIso, bias)
        }
        if (now - lastAutoIsoNs < 95_000_000L || frameIso <= 0 || frameExposureNs <= 0) return
        // Frames still exposed with the values before the last command
        // would make the loop correct twice.
        if (autoIsoWaitUntilNs != 0L) {
            // Within what sensors round to: gain steps, and a line time
            // (tens of µs) that is more than 2 % of a short shutter.
            val arrived = kotlin.math.abs(frameIso - autoIsoWaitIso) <= maxOf(2, autoIsoWaitIso / 25) &&
                kotlin.math.abs(frameExposureNs - autoIsoWaitExposureNs) <= maxOf(autoIsoWaitExposureNs / 50, 50_000L)
            if (!arrived && now < autoIsoWaitUntilNs) return
            autoIsoWaitUntilNs = 0L
        }
        lastAutoIsoNs = now
        val isoRange = isoRange ?: return
        val exposureRange = exposureRange ?: return
        val mean = Luma.meteredMean(image, s.meteringPoint).coerceAtLeast(0.002)
        val input = autoIsoInput
        input[0] = Hot.hostNowNs().toDouble()
        // EV in light, from gamma-coded luma (about 2.2), against mid grey.
        input[1] = 2.2 * kotlin.math.log2(mean / Luma.TARGET)
        input[2] = frameIso.toDouble()
        input[3] = frameExposureNs.toDouble()
        input[4] = isoRange.lower.toDouble()
        input[5] = isoRange.upper.toDouble()
        input[6] = exposureRange.lower.toDouble()
        input[7] = longestExposureNs(s)
        val apply = Hot.autoIsoOnSample(autoIso, input, autoIsoOutput)
        Hot.autoIsoStatus(autoIso, autoIsoState)
        val limit = autoIsoState[2].toInt()
        if (limit != autoIsoLimit) {
            autoIsoLimit = limit
            onAutoIsoLimit?.invoke(limit, frameIso)
        }
        if (!apply) return
        val iso = autoIsoOutput[0].toInt().coerceIn(isoRange.lower, isoRange.upper)
        val exposure = autoIsoOutput[1].toLong()
        synchronized(this) {
            // Only while still in auto: the operator may have just gone manual.
            if (settings.exposureMode == ExposureMode.AUTO) settings = settings.copy(iso = iso, exposureNs = exposure)
        }
        awaitFrame(iso, exposure, now)
        repeat()
    }

    /** Camera thread: the next samples wait for frames with this ISO and exposure (at most [AUTO_ISO_WAIT_NS]). */
    private fun awaitFrame(iso: Int, exposureNs: Long, now: Long) {
        autoIsoWaitIso = iso
        autoIsoWaitExposureNs = exposureNs
        autoIsoWaitUntilNs = now + AUTO_ISO_WAIT_NS
    }

    private fun samplePeaking(image: android.media.Image, now: Long) {
        val sink = onPeaking ?: return
        if (now - lastPeakingNs < 100_000_000L || !peakingBusy.compareAndSet(false, true)) return
        lastPeakingNs = now
        if (!peaking.sample(image)) {
            peakingBusy.set(false)
            return
        }
        analysisHandler.post {
            try {
                val mask = peaking.mask()
                if (onPeaking != null) sink(mask)
            } finally {
                peakingBusy.set(false)
            }
        }
    }

    /**
     * A zoom move for the operator's rocker: `rate` doublings per second
     * (negative zooms out) until 0 or the end of the range (at most 10x).
     */
    fun rampZoom(rate: Double) {
        handler.post {
            val wasIdle = zoomRate == 0.0
            zoomRate = rate
            if (rate != 0.0 && wasIdle) handler.post(zoomStep)
        }
    }

    private val zoomStep = object : Runnable {
        override fun run() {
            val range = zoomRange ?: return run { zoomRate = 0.0 }
            val rate = zoomRate
            if (rate == 0.0) return
            val top = minOf(range.upper, 10f)
            val z = (settings.zoom * Math.pow(2.0, rate * ZOOM_STEP_MS / 1000.0).toFloat()).coerceIn(range.lower, top)
            change { it.copy(zoom = z) }
            repeat()
            if ((rate > 0 && z >= top) || (rate < 0 && z <= range.lower)) zoomRate = 0.0
            else handler.postDelayed(this, ZOOM_STEP_MS)
        }
    }

    /** Camera thread: a grid of this frame for the statistics, measured on the analysis thread. */
    private fun sampleStats(image: android.media.Image, now: Long) {
        val sink = onStats ?: return
        if (now - lastStatsNs < statsIntervalNs || !statsBusy.compareAndSet(false, true)) return
        lastStatsNs = now
        if (!statsGrid.sample(image)) {
            statsBusy.set(false)
            return
        }
        analysisHandler.post {
            try {
                val g = statsGrid
                // Camera2's YUV is full range (JFIF).
                se.eyevinn.tallycam.core.imageStatsCompute(g.y, g.cbcr, g.width.toUInt(), g.height.toUInt(), false)
                    ?.let(sink)
            } catch (e: Exception) {
                Log.w(TAG, "image statistics", e)
            } finally {
                statsBusy.set(false)
            }
        }
    }

    /**
     * Back cameras the app can switch between. The first back camera in the
     * list is the main one ("wide"); shorter focal lengths are "ultra_wide",
     * longer "tele". Many phones expose only one logical camera and reach the
     * other lenses through the zoom ratio instead.
     */
    private fun backLenses(): List<Lens> {
        val back = manager.cameraIdList.mapNotNull { id ->
            val c = manager.getCameraCharacteristics(id)
            if (c.get(CameraCharacteristics.LENS_FACING) != CameraCharacteristics.LENS_FACING_BACK) return@mapNotNull null
            val focal = c.get(CameraCharacteristics.LENS_INFO_AVAILABLE_FOCAL_LENGTHS)?.minOrNull() ?: return@mapNotNull null
            id to focal
        }
        val main = back.firstOrNull()?.second ?: return emptyList()
        return back.map { (id, focal) ->
            val key = when {
                focal < main * 0.9f -> "ultra_wide"
                focal > main * 1.1f -> "tele"
                else -> "wide"
            }
            Lens(id, key, LENS_LABELS.getValue(key), focal)
        }.distinctBy { it.key }.sortedBy { it.focalLength }
    }

    companion object {
        private const val TAG = "CameraCapture"
        private const val ZOOM_STEP_MS = 33L
        /** Longest wait for a commanded exposure to show in the frames (a few frames of pipeline). */
        private const val AUTO_ISO_WAIT_NS = 400_000_000L

        /** CONTROL_AE_ANTIBANDING_MODE_* for a mains frequency (0: off). */
        fun antibandingMode(hz: Int) = when (hz) {
            50 -> CameraMetadata.CONTROL_AE_ANTIBANDING_MODE_50HZ
            60 -> CameraMetadata.CONTROL_AE_ANTIBANDING_MODE_60HZ
            0 -> CameraMetadata.CONTROL_AE_ANTIBANDING_MODE_OFF
            else -> CameraMetadata.CONTROL_AE_ANTIBANDING_MODE_AUTO
        }
        private val LENS_LABELS = mapOf("ultra_wide" to "Ultra wide", "wide" to "Wide", "tele" to "Tele")
    }
}

/**
 * A camera's colour calibration for the core's white balance
 * (core/android/src/white_balance.rs): 56 doubles, the reference
 * illuminants (EXIF LightSource; 0 for none) then six row-major 3x3
 * matrices, colour transform 1 and 2, calibration transform 1 and 2,
 * forward matrix 1 and 2, zeros where the camera has none. Pure, unit tested.
 */
internal object ColorCalibration {
    const val LENGTH = 56

    /** The packed calibration, or null without a colour transform 1 (no calibration). */
    fun pack(illuminant1: Int?, illuminant2: Int?, colorTransform1: DoubleArray?, colorTransform2: DoubleArray?,
             calibration1: DoubleArray?, calibration2: DoubleArray?, forward1: DoubleArray?,
             forward2: DoubleArray?): DoubleArray? {
        if (colorTransform1 == null || colorTransform1.size < 9) return null
        val v = DoubleArray(LENGTH)
        v[0] = (illuminant1 ?: 0).toDouble()
        v[1] = (illuminant2 ?: 0).toDouble()
        listOf(colorTransform1, colorTransform2, calibration1, calibration2, forward1, forward2).forEachIndexed { i, m ->
            if (m != null && m.size >= 9) m.copyInto(v, 2 + 9 * i, 0, 9)
        }
        return v
    }

    /** ColorSpaceTransform's int[] (numerator, denominator pairs) for 9 doubles at `offset`. */
    fun transformElements(m: DoubleArray, offset: Int): IntArray = IntArray(18) {
        if (it % 2 == 1) DENOMINATOR else Math.round(m[offset + it / 2] * DENOMINATOR).toInt()
    }

    private const val DENOMINATOR = 10_000

    fun roundedKelvin(k: Double): Double = Math.round(k / 50) * 50.0

    fun roundedTint(tint: Double): Double = Math.round(tint).toDouble()
}

/** YUV_420_888 to NV21 and JPEG (YuvImage). */
internal object Yuv {
    /** Into `out` (w * h * 3 / 2 bytes): luma rows in bulk, chroma interleaved V U. */
    fun toNv21(image: android.media.Image, out: ByteArray) {
        val w = image.width
        val h = image.height
        val y = image.planes[0]
        val u = image.planes[1]
        val v = image.planes[2]
        val yBuf = y.buffer
        var o = 0
        if (y.pixelStride == 1) {
            for (row in 0 until h) {
                yBuf.position(row * y.rowStride)
                yBuf.get(out, o, w)
                o += w
            }
        } else {
            for (row in 0 until h) for (col in 0 until w) out[o++] = yBuf.get(row * y.rowStride + col * y.pixelStride)
        }
        val vBuf = v.buffer
        val uBuf = u.buffer
        for (row in 0 until h / 2) {
            val vBase = row * v.rowStride
            val uBase = row * u.rowStride
            for (col in 0 until w / 2) {
                out[o++] = vBuf.get(vBase + col * v.pixelStride)
                out[o++] = uBuf.get(uBase + col * u.pixelStride)
            }
        }
    }

    /**
     * The size a w x h frame scales to at `width`: even (NV21 has 2x2
     * chroma), the aspect ratio kept, never larger than the frame.
     */
    fun scaledSize(w: Int, h: Int, width: Int): Pair<Int, Int> {
        val dw = (width and 1.inv()).coerceIn(2, w)
        if (dw == w) return w to h
        val dh = (((dw.toLong() * h + w / 2) / w).toInt() and 1.inv()).coerceIn(2, h)
        return dw to dh
    }

    /** Scales an NV21 frame down (box filter, each output pixel the mean of the ones it covers). */
    fun scale(src: ByteArray, sw: Int, sh: Int, dst: ByteArray, dw: Int, dh: Int) {
        scalePlane(src, 0, sw, sh, dst, 0, dw, dh, 1)
        scalePlane(src, sw * sh, sw / 2, sh / 2, dst, dw * dh, dw / 2, dh / 2, 2)
    }

    /** One plane of `channels` interleaved bytes a pixel (2 for NV21's V U), at the offsets given. */
    private fun scalePlane(src: ByteArray, srcOff: Int, sw: Int, sh: Int,
                           dst: ByteArray, dstOff: Int, dw: Int, dh: Int, channels: Int) {
        var o = dstOff
        for (y in 0 until dh) {
            val y0 = y * sh / dh
            val y1 = maxOf(y0 + 1, (y + 1) * sh / dh)
            for (x in 0 until dw) {
                val x0 = x * sw / dw
                val x1 = maxOf(x0 + 1, (x + 1) * sw / dw)
                val n = (y1 - y0) * (x1 - x0)
                for (c in 0 until channels) {
                    var sum = 0
                    for (sy in y0 until y1) {
                        var i = srcOff + (sy * sw + x0) * channels + c
                        for (sx in x0 until x1) {
                            sum += src[i].toInt() and 0xff
                            i += channels
                        }
                    }
                    dst[o++] = ((sum + n / 2) / n).toByte()
                }
            }
        }
    }

    fun compress(nv21: ByteArray, w: Int, h: Int, quality: Int = 70): ByteArray {
        val out = ByteArrayOutputStream(w * h / 4)
        YuvImage(nv21, ImageFormat.NV21, w, h, null).compressToJpeg(Rect(0, 0, w, h), quality, out)
        return out.toByteArray()
    }
}
