package se.eyevinn.tallycam

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test
import se.eyevinn.tallycam.media.Yuv

class YuvScaleTest {
    @Test
    fun scaledSizeKeepsTheAspectRatioAndNeverGrows() {
        assertEquals(480 to 270, Yuv.scaledSize(640, 360, 480))
        assertEquals(640 to 360, Yuv.scaledSize(640, 360, 640))
        assertEquals(640 to 360, Yuv.scaledSize(640, 360, 4096))
        // Even sizes only: NV21 has 2x2 chroma.
        assertEquals(320 to 180, Yuv.scaledSize(640, 360, 321))
        assertEquals(16 to 8, Yuv.scaledSize(640, 360, 16))
        assertEquals(240 to 180, Yuv.scaledSize(320, 240, 240))
    }

    @Test
    fun scaleAveragesTheCoveredPixels() {
        // 4x4 to 2x2: four 2x2 luma blocks, and 2x2 chroma (V U pairs) to one pair.
        val y = byteArrayOf(0, 2, 100, 100,
                            2, 4, 100, 100,
                            10, 10, (200).toByte(), (200).toByte(),
                            10, 10, (200).toByte(), (202).toByte())
        val vu = byteArrayOf(50, 60, 70, 80,
                             90, 100, 110, 120)
        val dst = ByteArray(2 * 2 * 3 / 2)
        Yuv.scale(y + vu, 4, 4, dst, 2, 2)
        // Luma: block means (rounded); chroma: one V U pair, the mean of four.
        assertArrayEquals(byteArrayOf(2, 100, 10, (201).toByte(), 80, 90), dst)
    }
}
