package se.eyevinn.tallycam

import org.junit.Assert.assertEquals
import org.junit.Test
import se.eyevinn.tallycam.media.AnnexB
import java.nio.ByteBuffer

class AnnexBTest {
    private fun bytes(vararg b: Int) = ByteArray(b.size) { b[it].toByte() }

    @Test
    fun convertsThreeAndFourByteStartCodes() {
        // SPS (4-byte start code), PPS (3-byte), IDR slice (4-byte).
        val annexB = bytes(0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 4, 5, 6)
        val src = ByteBuffer.allocateDirect(annexB.size).put(annexB).flip() as ByteBuffer
        val dst = ByteBuffer.allocateDirect(AnnexB.capacityFor(annexB.size))
        val n = AnnexB.toLengthPrefixed(src, dst)
        val out = ByteArray(n).also { dst.get(it) }
        val expected = bytes(0, 0, 0, 3, 0x67, 1, 2, 0, 0, 0, 2, 0x68, 3, 0, 0, 0, 4, 0x65, 4, 5, 6)
        assertEquals(expected.toList(), out.toList())
    }

    @Test
    fun keepsPositionOffsetsAndFindsNoKeyframeInPSlices() {
        val annexB = bytes(9, 9, 0, 0, 1, 0x41, 7, 7)
        val src = ByteBuffer.allocateDirect(annexB.size).put(annexB).flip() as ByteBuffer
        src.position(2)
        val dst = ByteBuffer.allocateDirect(64)
        val n = AnnexB.toLengthPrefixed(src, dst)
        assertEquals(4 + 3, n)
        assertEquals(3, dst.getInt(0))
    }

    @Test
    fun emptyWithoutStartCode() {
        val src = ByteBuffer.allocateDirect(4).put(bytes(1, 2, 3, 4)).flip() as ByteBuffer
        assertEquals(0, AnnexB.toLengthPrefixed(src, ByteBuffer.allocateDirect(16)))
    }
}
