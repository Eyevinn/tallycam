package se.eyevinn.tallycam

import android.hardware.camera2.CameraMetadata
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import se.eyevinn.tallycam.control.CameraMath

class CameraMathTest {
    @Test
    fun shutterOptionsFitTheSensorAndTheFrame() {
        // 1/8000 to 1/30 s exposure range, 60 fps: nothing longer than a frame.
        val options = CameraMath.shutterOptions(125_000L, 33_333_334L, 60)
        assertEquals(60, options.first())
        assertEquals(8000, options.last())
        assertEquals(listOf(30, 48, 50, 60), CameraMath.shutterOptions(125_000L, 40_000_000L, 30).take(4))
        assertEquals(100, CameraMath.shutterDenominator(10_100_000L))
    }

    @Test
    fun whiteBalancePresetNearestToKelvin() {
        val presets = listOf(CameraMetadata.CONTROL_AWB_MODE_INCANDESCENT, CameraMetadata.CONTROL_AWB_MODE_DAYLIGHT,
                             CameraMetadata.CONTROL_AWB_MODE_CLOUDY_DAYLIGHT)
        assertEquals(CameraMetadata.CONTROL_AWB_MODE_DAYLIGHT, CameraMath.nearestPreset(5000.0, presets))
        assertEquals(CameraMetadata.CONTROL_AWB_MODE_INCANDESCENT, CameraMath.nearestPreset(2000.0, presets))
        assertNull(CameraMath.nearestPreset(5000.0, emptyList()))
    }

    @Test
    fun focusPositionAndDioptersRoundTrip() {
        assertEquals(10f, CameraMath.focusDiopters(0.0, 10f), 1e-6f)
        assertEquals(0f, CameraMath.focusDiopters(1.0, 10f), 1e-6f)
        assertEquals(0.75, CameraMath.focusPosition(CameraMath.focusDiopters(0.75, 8f), 8f), 1e-6)
    }
}
