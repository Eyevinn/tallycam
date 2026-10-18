package se.eyevinn.tallycam

import org.junit.Assert.assertEquals
import org.junit.Test
import se.eyevinn.tallycam.control.Settings

class GenlockOffsetTest {
    @Test
    fun offsetKeepsWithinTheAdvertisedRange() {
        assertEquals(0.0, Settings.clampedGenlockOffsetUs(0.0), 0.0)
        assertEquals(-1234.5, Settings.clampedGenlockOffsetUs(-1234.5), 0.0)
        assertEquals(50000.0, Settings.clampedGenlockOffsetUs(50000.0), 0.0)
        assertEquals(50000.0, Settings.clampedGenlockOffsetUs(1e9), 0.0)
        assertEquals(-50000.0, Settings.clampedGenlockOffsetUs(-50000.1), 0.0)
    }

    @Test
    fun notFiniteIsNoOffset() {
        assertEquals(0.0, Settings.clampedGenlockOffsetUs(Double.NaN), 0.0)
        assertEquals(0.0, Settings.clampedGenlockOffsetUs(Double.POSITIVE_INFINITY), 0.0)
        assertEquals(0.0, Settings.clampedGenlockOffsetUs(Double.NEGATIVE_INFINITY), 0.0)
    }
}
