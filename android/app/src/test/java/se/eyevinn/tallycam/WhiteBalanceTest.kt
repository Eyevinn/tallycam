package se.eyevinn.tallycam

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import se.eyevinn.tallycam.control.CameraControls
import se.eyevinn.tallycam.media.ColorCalibration

class WhiteBalanceTest {
    private fun matrix(base: Double) = DoubleArray(9) { base + it }

    @Test
    fun calibrationPacksInTheCoresLayout() {
        // D65 (21) and Standard A (17), every matrix present.
        val v = ColorCalibration.pack(21, 17, matrix(100.0), matrix(200.0), matrix(300.0), matrix(400.0),
                                      matrix(500.0), matrix(600.0))!!
        assertEquals(ColorCalibration.LENGTH, v.size)
        assertEquals(21.0, v[0], 0.0)
        assertEquals(17.0, v[1], 0.0)
        for (m in 0 until 6) for (i in 0 until 9) assertEquals(100.0 * (m + 1) + i, v[2 + 9 * m + i], 0.0)
    }

    @Test
    fun missingKeysAreZeros() {
        // One reference illuminant, no calibration or forward matrices.
        val v = ColorCalibration.pack(21, null, matrix(1.0), null, null, null, null, null)!!
        assertEquals(0.0, v[1], 0.0)
        assertArrayEquals(matrix(1.0), v.copyOfRange(2, 11), 0.0)
        assertArrayEquals(DoubleArray(45), v.copyOfRange(11, 56), 0.0)
        // No colour transform 1: no calibration at all.
        assertNull(ColorCalibration.pack(21, 17, null, matrix(1.0), null, null, null, null))
    }

    @Test
    fun transformElementsAreRationals() {
        val m = doubleArrayOf(0.0, 0.0, 0.0, 0.0, 1.5, -0.25, 0.12345, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0)
        val e = ColorCalibration.transformElements(m, 4)
        assertEquals(18, e.size)
        assertEquals(15000, e[0])
        assertEquals(10000, e[1])
        assertEquals(-2500, e[2])
        assertEquals(1235, e[4])
        assertEquals(10000, e[8])
        for (i in 1 until 18 step 2) assertEquals(10000, e[i])
    }

    @Test
    fun reportedValuesAreRoundedToTheControlSteps() {
        assertEquals(5550.0, ColorCalibration.roundedKelvin(5530.0), 0.0)
        assertEquals(5500.0, ColorCalibration.roundedKelvin(5520.0), 0.0)
        assertEquals(-12.0, ColorCalibration.roundedTint(-11.6), 0.0)
        assertEquals(0.0, ColorCalibration.roundedTint(-0.2), 0.0)
    }

    @Test
    fun lockedIsManual() {
        assertEquals(false, CameraControls.manualMode("auto"))
        assertEquals(true, CameraControls.manualMode("manual"))
        // From older servers: manual at the current values.
        assertEquals(true, CameraControls.manualMode("locked"))
        assertNull(CameraControls.manualMode("auto_iso"))
        assertNull(CameraControls.manualMode(1))
        assertNull(CameraControls.manualMode(null))
    }
}
