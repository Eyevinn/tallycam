package se.eyevinn.tallycam.core

import java.nio.ByteBuffer

/**
 * The per-frame C ABI of the core, through JNI (core/android). Frames go in
 * direct [ByteBuffer]s: nothing is copied on the way in, nothing allocated.
 * The UniFFI bindings (ClockService, SrtCaller, device keys) load the same
 * library through JNA.
 */
object Hot {
    init {
        System.loadLibrary("tallycam_android")
    }

    const val VIDEO_H264: Byte = 0
    const val VIDEO_HEVC: Byte = 1
    const val AUDIO_NONE: Byte = 0
    const val AUDIO_OPUS: Byte = 1
    const val AUDIO_AAC: Byte = 2

    /** Not muxed yet (no format, or waiting for the first keyframe). */
    const val TS_SKIPPED = 0
    const val SRT_NOT_CONNECTED = -1
    const val SRT_QUEUE_FULL = -2

    const val SEI_FLAG_SYNCED = 0x01
    const val SEI_FLAG_TAI = 0x02

    /** CLOCK_BOOTTIME, ns: the clock of camera and audio timestamps. */
    @JvmStatic external fun hostNowNs(): Long

    /** tai_ns, utc_ns, est_error_ns, max_error_ns, flags (1 synced, 2 disciplined). */
    @JvmStatic external fun clockMap(hostNs: Long, out: LongArray)

    @JvmStatic external fun seiUncertaintyUs(ns: Long): Int

    @JvmStatic external fun tsNew(srtHandle: Long, videoCodec: Byte, audioCodec: Byte, pcrDelayNs: Long): Long

    @JvmStatic external fun tsFree(mux: Long)

    /** Annex B parameter sets, as MediaCodec's codec-config buffer. */
    @JvmStatic external fun tsSetVideoFormat(mux: Long, parameterSets: ByteBuffer, len: Int, nalLengthSize: Byte): Boolean

    @JvmStatic external fun tsSetAudioFormat(mux: Long, sampleRate: Int, channels: Int): Boolean

    /** NAL units with 4-byte big-endian lengths. seiCaptureNs 0: no timestamp SEI. */
    @JvmStatic external fun tsVideo(
        mux: Long, data: ByteBuffer, len: Int, keyframe: Boolean, ptsNs: Long, sendCaptureNs: Long,
        seiCaptureNs: Long, seiFlags: Int, seiFrameCounter: Int, seiUncertaintyUs: Int, nowNs: Long,
    ): Int

    @JvmStatic external fun tsAudio(mux: Long, data: ByteBuffer, len: Int, ptsNs: Long, nowNs: Long): Int

    /** late video frames, late audio frames, started (0/1), start ns, PCR delay ns. */
    @JvmStatic external fun tsStats(mux: Long, out: LongArray)

    const val GENLOCK_FREE_RUN = 0
    const val GENLOCK_ACQUIRING = 1
    const val GENLOCK_LOCKED = 2

    /** Soft genlock onto the TAI frame grid (core/src/genlock.rs); 0 for a zero rate. */
    @JvmStatic external fun genlockNew(fpsNum: Int, fpsDen: Int, minFrameDurationNs: Double): Long

    @JvmStatic external fun genlockFree(g: Long)

    /** Every frame's TAI capture time; returns the frame duration to apply (ns) once per window, else 0. */
    @JvmStatic external fun genlockOnFrame(g: Long, captureTaiNs: Long, synced: Boolean): Double

    /** state, phase error ns, max phase error ns, frame duration ns, correction ppm. */
    @JvmStatic external fun genlockStatus(g: Long, out: DoubleArray)

    /** Frames land offset ns after each grid point (per-model calibration). */
    @JvmStatic external fun genlockSetTargetOffset(g: Long, offsetNs: Long)
}

/** One mapped host instant (lc_clock_map). */
data class ClockMapping(
    val taiNs: Long,
    val utcNs: Long,
    val estErrorNs: Long,
    val maxErrorNs: Long,
    val synced: Boolean,
    val disciplined: Boolean,
) {
    companion object {
        private val scratch = ThreadLocal.withInitial { LongArray(5) }

        fun at(hostNs: Long): ClockMapping {
            val v = scratch.get()!!
            Hot.clockMap(hostNs, v)
            return ClockMapping(v[0], v[1], v[2], v[3], v[4] and 1L != 0L, v[4] and 2L != 0L)
        }
    }
}
