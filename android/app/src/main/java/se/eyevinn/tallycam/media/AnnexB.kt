package se.eyevinn.tallycam.media

import java.nio.ByteBuffer

/**
 * MediaCodec writes Annex B (NAL units after 00 00 01 or 00 00 00 01); the
 * core's muxer takes NAL units after a 4-byte big-endian length, as
 * VideoToolbox writes them. Runs once per video frame: no allocation (the
 * copies are bulk puts between direct buffers, a memcpy each).
 */
object AnnexB {
    /**
     * Writes the NAL units of `src` (position to limit) into `dst` from 0,
     * each after its length, and returns the bytes written. `dst` must have
     * room for `src.remaining()` plus 4 bytes per NAL unit; see [capacityFor].
     * `src`'s position and limit are restored; `dst`'s position is left at 0.
     */
    fun toLengthPrefixed(src: ByteBuffer, dst: ByteBuffer): Int {
        val start = src.position()
        val end = src.limit()
        var out = 0
        var code = nextStart(src, start, end)
        try {
            while (code >= 0) {
                val nalStart = afterCode(src, code)
                val next = nextStart(src, nalStart, end)
                // The NAL unit ends where the next start code begins.
                val nalEnd = if (next >= 0) next else end
                val len = nalEnd - nalStart
                if (len > 0) {
                    dst.putInt(out, len)
                    src.limit(nalEnd).position(nalStart)
                    dst.position(out + 4)
                    dst.put(src)
                    src.limit(end)
                    out += 4 + len
                }
                code = next
            }
        } finally {
            src.limit(end).position(start)
            dst.position(0)
        }
        return out
    }

    /** Room [toLengthPrefixed] may need for `annexBLength` bytes. */
    fun capacityFor(annexBLength: Int): Int = annexBLength + annexBLength / 3 + 16

    /**
     * Index of the next start code (its first zero, a 4-byte code's leading
     * zero included) at or after `from`, or -1.
     */
    private fun nextStart(buf: ByteBuffer, from: Int, end: Int): Int {
        var i = from
        while (i + 2 < end) {
            if (buf.get(i + 2).toInt() and 0xFF > 1) {
                // No start code can end at i + 2: skip ahead.
                i += 3
            } else if (buf.get(i) == ZERO && buf.get(i + 1) == ZERO && buf.get(i + 2) == ONE) {
                return if (i > from && buf.get(i - 1) == ZERO) i - 1 else i
            } else {
                i += 1
            }
        }
        return -1
    }

    /** The first byte after the start code at `code` (3 or 4 bytes long). */
    private fun afterCode(buf: ByteBuffer, code: Int): Int = if (buf.get(code + 2) == ONE) code + 3 else code + 4

    private const val ZERO: Byte = 0
    private const val ONE: Byte = 1
}
