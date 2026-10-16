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
import android.hardware.camera2.params.MeteringRectangle
import android.hardware.camera2.params.OutputConfiguration
import android.hardware.camera2.params.SessionConfiguration
import android.media.ImageReader
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.util.Log
import android.util.Range
import android.util.Size
import android.view.Surface
import se.eyevinn.tallycam.core.ClockMapping
import se.eyevinn.tallycam.core.Hot
import java.io.ByteArrayOutputStream
import java.util.concurrent.Executor

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
    enum class ExposureMode { AUTO, LOCKED, MANUAL }
    enum class WhiteBalanceMode { AUTO, LOCKED, MANUAL }
    enum class FocusMode { AUTO, LOCKED, MANUAL }

    /** What the operator set. Copied, changed and applied through [update]. */
    data class Settings(
        val exposureMode: ExposureMode = ExposureMode.AUTO,
        val iso: Int = 400,
        val exposureNs: Long = 1_000_000_000L / 60,
        val evSteps: Int = 0,
        val whiteBalanceMode: WhiteBalanceMode = WhiteBalanceMode.AUTO,
        /** An AWB preset (CONTROL_AWB_MODE_*), for manual white balance. */
        val awbPreset: Int = CameraMetadata.CONTROL_AWB_MODE_DAYLIGHT,
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
        /** CONTROL_AE_ANTIBANDING_MODE_*: the `mains_hz` control. */
        val antibanding: Int = CameraMetadata.CONTROL_AE_ANTIBANDING_MODE_AUTO,
        /** Normalised point (0..1) for focus and exposure, or null for the whole frame. */
        val meteringPoint: Pair<Float, Float>? = null,
    )

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

    val characteristics: CameraCharacteristics?
        get() = cameraId?.let { manager.getCameraCharacteristics(it) }

    @Volatile var settings = Settings()
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
    @Volatile var onThumbnail: ((ByteArray, Int, Int) -> Unit)? = null
    private var lastThumbNs = 0L

    @Volatile var onError: ((String) -> Unit)? = null

    // MARK: Capabilities of the active camera

    private fun capabilities(): IntArray =
        characteristics?.get(CameraCharacteristics.REQUEST_AVAILABLE_CAPABILITIES) ?: IntArray(0)

    val hasManualSensor: Boolean
        get() = CameraMetadata.REQUEST_AVAILABLE_CAPABILITIES_MANUAL_SENSOR in capabilities()

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

    val awbPresets: List<Int>
        get() = characteristics?.get(CameraCharacteristics.CONTROL_AWB_AVAILABLE_MODES)?.toList().orEmpty()
            .filter { it != CameraMetadata.CONTROL_AWB_MODE_AUTO && it != CameraMetadata.CONTROL_AWB_MODE_OFF }

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
        change(change)
        handler.post { repeat() }
    }

    private fun change(change: (Settings) -> Settings) {
        synchronized(this) { settings = change(settings) }
    }

    /** One autofocus (and exposure metering) at a point, then continuous focus there. */
    fun focusAt(x: Float, y: Float) {
        handler.post {
            change { it.copy(meteringPoint = x.coerceIn(0f, 1f) to y.coerceIn(0f, 1f), focusMode = FocusMode.AUTO) }
            afTrigger = true
            repeat()
        }
    }

    /** Lets auto white balance settle, then locks it. */
    fun whiteBalanceOnce(done: () -> Unit) {
        handler.post {
            change { it.copy(whiteBalanceMode = WhiteBalanceMode.AUTO) }
            repeat()
            handler.postDelayed({
                change { it.copy(whiteBalanceMode = WhiteBalanceMode.LOCKED) }
                repeat()
                done()
            }, 700)
        }
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
            change { it.copy(zoom = 1f, meteringPoint = null) }
            texture?.let { openOnThread(it) }
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

    /** The preview went away (app in the background): keep streaming without it. */
    fun detachPreview() {
        handler.post {
            texture = null
            previewSurface?.release()
            previewSurface = null
            if (encoderSurface != null) configure() else closeDevice()
        }
    }

    /** null: back to preview only. */
    fun setEncoderSurface(surface: Surface?) {
        handler.post {
            encoderSurface = surface
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
    }

    // MARK: Camera thread

    @SuppressLint("MissingPermission")
    private fun openOnThread(texture: SurfaceTexture) {
        this.texture = texture
        texture.setDefaultBufferSize(streamSize.width, streamSize.height)
        previewSurface = Surface(texture)
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
                    camera.close()
                    if (device === camera) device = null
                }

                override fun onError(camera: CameraDevice, error: Int) {
                    camera.close()
                    if (device === camera) device = null
                    onError?.invoke("Camera error $error")
                }
            })
        } catch (e: Exception) {
            onError?.invoke("Cannot open the camera: ${e.message}")
        }
    }

    private fun configure() {
        val device = device ?: return
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
                    }

                    override fun onConfigureFailed(s: CameraCaptureSession) {
                        onError?.invoke("Camera session failed")
                    }
                }))
        } catch (e: Exception) {
            Log.e(TAG, "Camera session failed (${outputs.size} outputs, stream $streamSize, preview images $thumbSize)", e)
            onError?.invoke("Camera session failed: ${e.message}")
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
        listOfNotNull(previewSurface, encoderSurface, thumbnails?.surface).forEach(builder::addTarget)
        builder.apply(s)
        try {
            if (afTrigger) {
                afTrigger = false
                builder.set(CaptureRequest.CONTROL_AF_TRIGGER, CameraMetadata.CONTROL_AF_TRIGGER_START)
                session.capture(builder.build(), null, handler)
                builder.set(CaptureRequest.CONTROL_AF_TRIGGER, CameraMetadata.CONTROL_AF_TRIGGER_IDLE)
            }
            session.setRepeatingRequest(builder.build(), resultCallback, handler)
        } catch (e: Exception) {
            Log.w(TAG, "Repeating request failed", e)
        }
    }

    private fun CaptureRequest.Builder.apply(s: Settings) {
        set(CaptureRequest.CONTROL_MODE, CameraMetadata.CONTROL_MODE_AUTO)
        set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, Range(s.frameRate, s.frameRate))
        if (s.antibanding in antibandingModes) set(CaptureRequest.CONTROL_AE_ANTIBANDING_MODE, s.antibanding)
        set(CaptureRequest.CONTROL_VIDEO_STABILIZATION_MODE,
            if (s.stabilization && hasStabilization) CameraMetadata.CONTROL_VIDEO_STABILIZATION_MODE_ON
            else CameraMetadata.CONTROL_VIDEO_STABILIZATION_MODE_OFF)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R && zoomRange != null) {
            set(CaptureRequest.CONTROL_ZOOM_RATIO, s.zoom)
        }
        val region = s.meteringPoint?.let { meteringRegion(it) }
        // Exposure.
        when (s.exposureMode) {
            ExposureMode.MANUAL -> if (hasManualSensor) {
                set(CaptureRequest.CONTROL_AE_MODE, CameraMetadata.CONTROL_AE_MODE_OFF)
                set(CaptureRequest.SENSOR_SENSITIVITY, s.iso)
                set(CaptureRequest.SENSOR_EXPOSURE_TIME, s.exposureNs)
                val duration = if (s.genlock && genlockDurationNs > 0) genlockDurationNs else 1_000_000_000L / s.frameRate
                set(CaptureRequest.SENSOR_FRAME_DURATION, duration)
            }
            ExposureMode.LOCKED -> {
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
        // White balance.
        when (s.whiteBalanceMode) {
            WhiteBalanceMode.AUTO -> {
                set(CaptureRequest.CONTROL_AWB_MODE, CameraMetadata.CONTROL_AWB_MODE_AUTO)
                set(CaptureRequest.CONTROL_AWB_LOCK, false)
            }
            WhiteBalanceMode.LOCKED -> {
                set(CaptureRequest.CONTROL_AWB_MODE, CameraMetadata.CONTROL_AWB_MODE_AUTO)
                set(CaptureRequest.CONTROL_AWB_LOCK, true)
            }
            WhiteBalanceMode.MANUAL -> set(CaptureRequest.CONTROL_AWB_MODE, s.awbPreset)
        }
        // Focus.
        when (s.focusMode) {
            FocusMode.AUTO -> {
                set(CaptureRequest.CONTROL_AF_MODE,
                    if (region != null) CameraMetadata.CONTROL_AF_MODE_AUTO else CameraMetadata.CONTROL_AF_MODE_CONTINUOUS_VIDEO)
                region?.let { set(CaptureRequest.CONTROL_AF_REGIONS, arrayOf(it)) }
            }
            FocusMode.LOCKED, FocusMode.MANUAL -> if (minFocusDiopters != null) {
                set(CaptureRequest.CONTROL_AF_MODE, CameraMetadata.CONTROL_AF_MODE_OFF)
                set(CaptureRequest.LENS_FOCUS_DISTANCE, s.focusDiopters)
            }
        }
        if (hasTorch) {
            set(CaptureRequest.FLASH_MODE, if (s.torch) CameraMetadata.FLASH_MODE_TORCH else CameraMetadata.FLASH_MODE_OFF)
        }
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
            measured = Measured(
                result.get(CaptureResult.SENSOR_SENSITIVITY),
                result.get(CaptureResult.SENSOR_EXPOSURE_TIME),
                result.get(CaptureResult.LENS_FOCUS_DISTANCE),
                result.get(CaptureResult.SENSOR_FRAME_DURATION),
            )
            result.get(CaptureResult.SENSOR_TIMESTAMP)?.let(::genlockFrame)
        }
    }

    /** Camera thread: one frame's start of exposure, sensor clock. */
    private fun genlockFrame(sensorNs: Long) {
        val s = settings
        val active = s.genlock && s.exposureMode == ExposureMode.MANUAL && hasManualSensor
        if (!active) {
            if (genlockLoop != 0L) {
                Hot.genlockFree(genlockLoop)
                genlockLoop = 0
                genlockKey = null
                genlockDurationNs = 0
                genlockStatus = null
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
        val mapping = ClockMapping.at(sensorNs + timestampOffsetNs())
        val duration = Hot.genlockOnFrame(genlockLoop, mapping.taiNs, mapping.synced)
        genlockStatus = DoubleArray(5).also { Hot.genlockStatus(genlockLoop, it) }
        if (duration > 0) {
            genlockDurationNs = duration.toLong()
            repeat()
        }
    }

    private fun thumbSize(): Size {
        val yuv = sizes?.getOutputSizes(ImageFormat.YUV_420_888)?.toList().orEmpty()
        return yuv.firstOrNull { it.width == 640 && it.height == 360 }
            ?: yuv.filter { it.width >= 320 }.minByOrNull { it.width * it.height }
            ?: Size(640, 480)
    }

    private fun onThumbnailImage(reader: ImageReader) {
        val image = reader.acquireLatestImage() ?: return
        try {
            val now = SystemClock.elapsedRealtimeNanos()
            val interval = thumbnailIntervalNs
            val sink = onThumbnail
            if (sink == null || interval <= 0 || now - lastThumbNs < interval) return
            lastThumbNs = now
            sink(Yuv.toJpeg(image), image.width, image.height)
        } finally {
            image.close()
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
        private val LENS_LABELS = mapOf("ultra_wide" to "Ultra wide", "wide" to "Wide", "tele" to "Tele")
    }
}

/** YUV_420_888 to JPEG through NV21 (YuvImage). */
internal object Yuv {
    fun toJpeg(image: android.media.Image, quality: Int = 70): ByteArray {
        val w = image.width
        val h = image.height
        val nv21 = ByteArray(w * h * 3 / 2)
        val y = image.planes[0]
        val u = image.planes[1]
        val v = image.planes[2]
        var o = 0
        for (row in 0 until h) {
            val buf = y.buffer
            val base = row * y.rowStride
            for (col in 0 until w) nv21[o++] = buf.get(base + col * y.pixelStride)
        }
        for (row in 0 until h / 2) {
            for (col in 0 until w / 2) {
                nv21[o++] = v.buffer.get(row * v.rowStride + col * v.pixelStride)
                nv21[o++] = u.buffer.get(row * u.rowStride + col * u.pixelStride)
            }
        }
        val out = ByteArrayOutputStream()
        YuvImage(nv21, ImageFormat.NV21, w, h, null).compressToJpeg(Rect(0, 0, w, h), quality, out)
        return out.toByteArray()
    }
}
