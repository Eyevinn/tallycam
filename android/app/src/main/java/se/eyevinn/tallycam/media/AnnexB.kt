package se.eyevinn.tallycam.media

import java.nio.ByteBuffer

/**
 * MediaCodec writes Annex B (NAL units after 00 00 01 or 00 00 00 01); the
 * core's muxer takes NAL units after a 4-byte big-endian length, as
 * VideoToolbox writes them.
 */
object AnnexB {
    /**
     * Writes the NAL units of `src` (position to limit) into `dst` from 0,
     * each after its length, and returns the bytes written. `dst` must have
     * room for `src.remaining()` plus 4 bytes per NAL unit; see [capacityFor].
     */
    fun toLengthPrefixed(src: ByteBuffer, dst: ByteBuffer): Int {
        val start = src.position()
        val end = src.limit()
        var out = 0
        var nal = nextStart(src, start, end) ?: return 0
        while (true) {
            val next = nextStart(src, nal.second, end)
            // The NAL unit ends where the next start code begins; trailing
            // zero bytes before a 4-byte start code belong to it.
            val nalEnd = next?.first ?: end
            val len = nalEnd - nal.second
            if (len > 0) {
                dst.putInt(out, len)
                // Bulk copy (a memcpy between direct buffers).
                val view = src.duplicate()
                view.limit(nalEnd).position(nal.second)
                dst.position(out + 4)
                dst.put(view)
                out += 4 + len
            }
            nal = next ?: break
        }
        dst.position(0)
        return out
    }

    /** Room [toLengthPrefixed] may need for `annexBLength` bytes. */
    fun capacityFor(annexBLength: Int): Int = annexBLength + annexBLength / 3 + 16

    /** (start of the start code, first byte after it) from `from`, or null. */
    private fun nextStart(buf: ByteBuffer, from: Int, end: Int): Pair<Int, Int>? {
        var i = from
        while (i + 2 < end) {
            if (buf.get(i) == 0.toByte() && buf.get(i + 1) == 0.toByte()) {
                val b2 = buf.get(i + 2)
                if (b2 == 1.toByte()) {
                    val codeStart = if (i > from && buf.get(i - 1) == 0.toByte()) i - 1 else i
                    return codeStart to i + 3
                }
                if (b2 != 0.toByte()) i += 2 else i += 1
            } else {
                i += 1
            }
        }
        return null
    }

    /** True if an H.264 access unit holds an IDR slice (nal_unit_type 5). */
    fun isH264Keyframe(lengthPrefixed: ByteBuffer, len: Int): Boolean {
        var i = 0
        while (i + 4 < len) {
            val n = lengthPrefixed.getInt(i)
            if (n <= 0) return false
            if (lengthPrefixed.get(i + 4).toInt() and 0x1F == 5) return true
            i += 4 + n
        }
        return false
    }
}
