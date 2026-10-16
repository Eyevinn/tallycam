package se.eyevinn.tallycam.media

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.os.Build
import android.os.Handler
import android.util.Log
import android.view.Surface
import java.nio.ByteBuffer

/**
 * Hardware H.264 from a Surface (the camera writes into it). Output
 * callbacks run on `handler`, the stream's thread, which also does the
 * muxing. Presentation times are the camera's sensor timestamps (µs).
 */
class VideoEncoder(
    /** MediaFormat.MIMETYPE_VIDEO_AVC or _HEVC. */
    private val mime: String,
    private val width: Int,
    private val height: Int,
    private val frameRate: Int,
    bitrateKbps: Int,
    private val handler: Handler,
    private val sink: Sink,
) {
    interface Sink {
        /** Annex B SPS and PPS, from position to limit. */
        fun onVideoFormat(parameterSets: ByteBuffer)

        /** One access unit, Annex B, from position to limit. */
        fun onVideoFrame(frame: ByteBuffer, ptsUs: Long, keyframe: Boolean)
    }

    private val codec = MediaCodec.createEncoderByType(mime)
    lateinit var inputSurface: Surface
        private set
    var bitrateKbps = bitrateKbps
        private set

    fun start() {
        val format = MediaFormat.createVideoFormat(mime, width, height).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, bitrateKbps * 1000)
            setInteger(MediaFormat.KEY_BITRATE_MODE, MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)
            setInteger(MediaFormat.KEY_FRAME_RATE, frameRate)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
            if (mime == MediaFormat.MIMETYPE_VIDEO_AVC) {
                // The level follows the size and rate (2160p and 60 fps need more than 4.1).
                setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.AVCProfileHigh)
            } else {
                setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.HEVCProfileMain)
            }
            // No B-frames: PTS order is decode order, as the muxer expects.
            setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
            // Real time, no faster: the codec neither buffers frames for
            // speed nor clocks up beyond the frame rate (power, heat).
            setInteger(MediaFormat.KEY_PRIORITY, 0)
            setInteger(MediaFormat.KEY_OPERATING_RATE, frameRate)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            }
        }
        codec.setCallback(object : MediaCodec.Callback() {
            override fun onInputBufferAvailable(codec: MediaCodec, index: Int) = Unit

            override fun onOutputBufferAvailable(codec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
                val buffer = runCatching { codec.getOutputBuffer(index) }.getOrNull()
                if (buffer != null && info.size > 0) {
                    buffer.position(info.offset).limit(info.offset + info.size)
                    if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                        sink.onVideoFormat(buffer)
                    } else {
                        sink.onVideoFrame(buffer, info.presentationTimeUs,
                                          info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0)
                    }
                }
                runCatching { codec.releaseOutputBuffer(index, false) }
            }

            override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) {
                Log.e(TAG, "Encoder error", e)
            }

            override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) = Unit
        }, handler)
        codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        inputSurface = codec.createInputSurface()
        codec.start()
        Log.i(TAG, "$mime ${width}x$height@$frameRate, $bitrateKbps kbit/s")
    }

    /** Reused: a keyframe request allocates nothing. */
    private val syncFrame = android.os.Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) }

    /** Makes the next frame a keyframe (stream thread). */
    fun requestKeyframe() {
        runCatching { codec.setParameters(syncFrame) }
    }

    /** Applied live. */
    fun setBitrate(kbps: Int) {
        bitrateKbps = kbps
        runCatching {
            codec.setParameters(android.os.Bundle().apply {
                putInt(MediaCodec.PARAMETER_KEY_VIDEO_BITRATE, kbps * 1000)
            })
        }
    }

    fun stop() {
        runCatching { codec.stop() }
        runCatching { codec.release() }
        runCatching { inputSurface.release() }
    }

    companion object {
        private const val TAG = "VideoEncoder"

        /** A hardware H.264 encoder exists (not only Google's software one). */
        fun hasHardwareH264(): Boolean = hasEncoder(MediaFormat.MIMETYPE_VIDEO_AVC, hardware = true)

        fun hasEncoder(mime: String, hardware: Boolean = false): Boolean =
            android.media.MediaCodecList(android.media.MediaCodecList.REGULAR_CODECS).codecInfos.any { info ->
                info.isEncoder && (!hardware || info.isHardwareAccelerated) &&
                    info.supportedTypes.any { it.equals(mime, ignoreCase = true) }
            }
    }
}
