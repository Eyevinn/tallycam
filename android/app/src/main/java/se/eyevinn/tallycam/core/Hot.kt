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

    /**
     * Like [tsVideo], on `len` bytes from absolute index `offset` of `data`
     * (a MediaCodec output buffer as it is: Annex B with a video format of
     * NAL length size 0).
     */
    @JvmStatic external fun tsVideoAt(
        mux: Long, data: ByteBuffer, offset: Int, len: Int, keyframe: Boolean, ptsNs: Long, sendCaptureNs: Long,
        seiCaptureNs: Long, seiFlags: Int, seiFrameCounter: Int, seiUncertaintyUs: Int, nowNs: Long,
    ): Int

    /** NAL units with 4-byte big-endian lengths. seiCaptureNs 0: no timestamp SEI. */
    @JvmStatic external fun tsVideo(
        mux: Long, data: ByteBuffer, len: Int, keyframe: Boolean, ptsNs: Long, sendCaptureNs: Long,
        seiCaptureNs: Long, seiFlags: Int, seiFrameCounter: Int, seiUncertaintyUs: Int, nowNs: Long,
    ): Int

    @JvmStatic external fun tsAudio(mux: Long, data: ByteBuffer, len: Int, ptsNs: Long, nowNs: Long): Int

    /** late video frames, late audio frames, started (0/1), start ns, PCR delay ns. */
    @JvmStatic external fun tsStats(mux: Long, out: LongArray)

    /** The stream waits for a keyframe in time for its PCR: ask the encoder for one. */
    @JvmStatic external fun tsWantsKeyframe(mux: Long): Boolean

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

    const val AUTO_ISO_LIMIT_NONE = 0
    const val AUTO_ISO_LIMIT_TOO_DARK = 1
    const val AUTO_ISO_LIMIT_TOO_BRIGHT = 2

    /** Auto ISO at a fixed shutter (core/src/exposure.rs); mains 50 or 60 rounds the shutter, 0 leaves it. */
    @JvmStatic external fun autoIsoNew(mainsHz: Int): Long

    @JvmStatic external fun autoIsoFree(a: Long)

    @JvmStatic external fun autoIsoReset(a: Long)

    @JvmStatic external fun autoIsoSetBias(a: Long, biasEv: Double)

    @JvmStatic external fun autoIsoSetMains(a: Long, mainsHz: Int)

    /** Free shutter: hold baseNs, shorten only at the lowest ISO; 0 turns it off. */
    @JvmStatic external fun autoIsoSetFreeShutter(a: Long, baseNs: Double)

    /**
     * input: t ns, metered offset EV, ISO, exposure ns, min ISO, max ISO,
     * min exposure ns, max exposure ns. True: out holds the ISO and exposure ns to apply.
     */
    @JvmStatic external fun autoIsoOnSample(a: Long, input: DoubleArray, out: DoubleArray): Boolean

    /** error EV, gain, limit (AUTO_ISO_LIMIT_*), settling (0/1). */
    @JvmStatic external fun autoIsoStatus(a: Long, out: DoubleArray)

    /** The longest exposure for a frame period: the format's maximum, the frame, room for genlock. */
    @JvmStatic external fun longestExposureNs(framePeriodNs: Double, formatMaxNs: Double, genlock: Boolean): Double

    /**
     * Manual white balance (core/src/white_balance.rs). `cal` is the
     * camera's colour calibration, 56 doubles (core/android/src/white_balance.rs,
     * [se.eyevinn.tallycam.media.ColorCalibration]). Fills `out` (13): the
     * gains R, Gr, Gb, B, then the 3x3 COLOR_CORRECTION_TRANSFORM row-major.
     * Returns 13, 4 (gains only) or 0 (none). Clamps to 2000-15000 K, tint ±150.
     */
    @JvmStatic external fun whiteBalanceFor(cal: DoubleArray, temperatureK: Double, tint: Double, out: DoubleArray): Int

    /** `gains` (4): R, Gr, Gb, B of an auto capture result. True: `out` (2) holds K and tint. */
    @JvmStatic external fun whiteBalanceTemperatureTint(cal: DoubleArray, gains: DoubleArray, out: DoubleArray): Boolean

    /** The greenest tint the core applies at `temperatureK` (+21 at 2000 K, the full 150 from about 3900 K). */
    @JvmStatic external fun whiteBalanceMaxTint(temperatureK: Double): Double
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
