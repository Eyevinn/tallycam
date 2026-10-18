package se.eyevinn.tallycam

import org.junit.Assert.assertArrayEquals
import org.junit.Test
import se.eyevinn.tallycam.media.Yuv
import java.nio.ByteBuffer
import kotlin.random.Random

/** Yuv.toNv21's bulk paths against the per-pixel copy they replaced, on synthetic planes. */
class YuvNv21Test {
    private class Plane(val buffer: ByteBuffer, val rowStride: Int, val pixelStride: Int)

    /** The per-pixel copy toNv21 used to make (absolute gets, as the old code). */
    private fun reference(w: Int, h: Int, y: Plane, u: Plane, v: Plane): ByteArray {
        val out = ByteArray(w * h * 3 / 2)
        var o = 0
        for (row in 0 until h) for (col in 0 until w) out[o++] = y.buffer.get(row * y.rowStride + col * y.pixelStride)
        for (row in 0 until h / 2) for (col in 0 until w / 2) {
            out[o++] = v.buffer.get(row * v.rowStride + col * v.pixelStride)
            out[o++] = u.buffer.get(row * u.rowStride + col * u.pixelStride)
        }
        return out
    }

    private fun converted(w: Int, h: Int, y: Plane, u: Plane, v: Plane): ByteArray {
        val out = ByteArray(w * h * 3 / 2)
        Yuv.toNv21(w, h, y.buffer, y.rowStride, y.pixelStride, u.buffer, u.rowStride, u.pixelStride,
                   v.buffer, v.rowStride, v.pixelStride, out)
        return out
    }

    /** A plane as Image gives one: the last row only as long as its pixels need. */
    private fun plane(random: Random, width: Int, height: Int, rowStride: Int, pixelStride: Int): Plane {
        val bytes = ByteArray((height - 1) * rowStride + (width - 1) * pixelStride + 1).also(random::nextBytes)
        return Plane(ByteBuffer.wrap(bytes), rowStride, pixelStride)
    }

    /** Interleaved chroma in one buffer, the second plane one byte in (NV21 when V is first). */
    private fun interleaved(random: Random, w: Int, h: Int, rowStride: Int): Pair<Plane, Plane> {
        val whole = ByteBuffer.wrap(ByteArray((h / 2 - 1) * rowStride + w).also(random::nextBytes))
        val first = whole.duplicate().apply { limit(capacity() - 1) }.slice()
        val second = whole.duplicate().apply { position(1) }.slice()
        return Plane(first, rowStride, 2) to Plane(second, rowStride, 2)
    }

    private fun check(w: Int, h: Int, y: Plane, u: Plane, v: Plane) {
        val expected = reference(w, h, y, u, v)
        assertArrayEquals(expected, converted(w, h, y, u, v))
    }

    @Test
    fun nv21LayoutCopiesWholeRows() {
        val random = Random(1)
        for ((w, h, stride) in listOf(Triple(640, 360, 640), Triple(640, 360, 704), Triple(322, 182, 384))) {
            val (v, u) = interleaved(random, w, h, stride)
            val before = ByteArray(v.buffer.capacity()).also { v.buffer.duplicate().get(it) }
            check(w, h, plane(random, w, h, stride, 1), u, v)
            // The overlap test writes a byte of V and puts it back.
            val after = ByteArray(v.buffer.capacity()).also { v.buffer.duplicate().apply { rewind() }.get(it) }
            assertArrayEquals(before, after)
        }
    }

    @Test
    fun nv12LayoutIsNotTakenForNv21() {
        val random = Random(2)
        val (u, v) = interleaved(random, 640, 360, 640)
        check(640, 360, plane(random, 640, 360, 640, 1), u, v)
    }

    @Test
    fun separatePlanes() {
        val random = Random(3)
        // I420 with padded rows, and two separate planes of pixel stride 2.
        check(640, 360, plane(random, 640, 360, 672, 1), plane(random, 320, 180, 352, 1), plane(random, 320, 180, 352, 1))
        check(320, 240, plane(random, 320, 240, 320, 1), plane(random, 160, 120, 320, 2), plane(random, 160, 120, 320, 2))
        // A read-only V plane (no overlap test) laid out as NV21.
        val (v, u) = interleaved(random, 320, 240, 320)
        check(320, 240, plane(random, 320, 240, 320, 1), u, Plane(v.buffer.asReadOnlyBuffer(), v.rowStride, 2))
    }

    @Test
    fun lumaWithPixelStride() {
        val random = Random(4)
        check(64, 36, plane(random, 64, 36, 160, 2), plane(random, 32, 18, 32, 1), plane(random, 32, 18, 32, 1))
    }
}
