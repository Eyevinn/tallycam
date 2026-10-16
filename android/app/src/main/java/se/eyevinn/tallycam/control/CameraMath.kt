package se.eyevinn.tallycam.control

import android.hardware.camera2.CameraMetadata

/** The pure parts of the camera controls, unit tested. */
object CameraMath {
    /** Shutter denominators (1/x s) the iOS app offers too. */
    val SHUTTERS = listOf(30, 48, 50, 60, 100, 120, 125, 180, 240, 250, 320, 500, 640, 1000, 2000, 4000, 8000)

    /** Those within the sensor's exposure range and at most one frame long. */
    fun shutterOptions(minNs: Long, maxNs: Long, frameRate: Int): List<Int> =
        SHUTTERS.filter { d ->
            val ns = 1_000_000_000L / d
            ns in minNs..maxNs && d >= frameRate
        }

    fun shutterNs(denominator: Int): Long = 1_000_000_000L / denominator

    /** The nearest shutter denominator for an exposure time. */
    fun shutterDenominator(ns: Long, options: List<Int> = SHUTTERS): Int =
        options.minByOrNull { kotlin.math.abs(1_000_000_000.0 / it - ns) } ?: (1_000_000_000L / ns.coerceAtLeast(1)).toInt()

    /** Nominal colour temperature of the AWB presets, Kelvin. */
    val PRESET_KELVIN = mapOf(
        CameraMetadata.CONTROL_AWB_MODE_INCANDESCENT to 2700,
        CameraMetadata.CONTROL_AWB_MODE_WARM_FLUORESCENT to 3000,
        CameraMetadata.CONTROL_AWB_MODE_TWILIGHT to 3700,
        CameraMetadata.CONTROL_AWB_MODE_FLUORESCENT to 4000,
        CameraMetadata.CONTROL_AWB_MODE_DAYLIGHT to 5500,
        CameraMetadata.CONTROL_AWB_MODE_CLOUDY_DAYLIGHT to 6500,
        CameraMetadata.CONTROL_AWB_MODE_SHADE to 7500,
    )

    /** The available preset nearest to `kelvin`, or null without presets. */
    fun nearestPreset(kelvin: Double, available: List<Int>): Int? =
        available.filter { it in PRESET_KELVIN }.minByOrNull { kotlin.math.abs(PRESET_KELVIN.getValue(it) - kelvin) }

    /** focus_position 0 (near) to 1 (far, infinity) and Camera2 diopters. */
    fun focusDiopters(position: Double, minFocusDiopters: Float): Float =
        (minFocusDiopters * (1 - position.coerceIn(0.0, 1.0))).toFloat()

    fun focusPosition(diopters: Float, minFocusDiopters: Float): Double =
        if (minFocusDiopters <= 0f) 1.0 else (1 - diopters / minFocusDiopters).toDouble().coerceIn(0.0, 1.0)
}
