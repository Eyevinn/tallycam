package se.eyevinn.tallycam.media

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.AudioTimestamp
import android.media.MediaCodec
import android.media.MediaFormat
import android.media.MediaRecorder
import android.util.Log
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Microphone (48 kHz mono) to Opus with MediaCodec, on its own thread.
 * Timestamps come from AudioRecord in CLOCK_BOOTTIME, the host clock of the
 * core; each packet's PTS is the capture time of its first sample minus the
 * encoder delay, as the muxer expects (docs/PROTOCOL.md, "MPEG-TS timing").
 */
class AudioEncoder(private val aac: Boolean, private val sink: Sink) {
    interface Sink {
        /** Called on the encoder thread; copy what you keep. */
        fun onAudioPacket(packet: ByteBuffer, ptsNs: Long)
    }

    @Volatile private var running = false
    private var thread: Thread? = null

    @SuppressLint("MissingPermission")
    fun start(): Boolean {
        val minBuffer = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        val record = try {
            AudioRecord(MediaRecorder.AudioSource.CAMCORDER, RATE, AudioFormat.CHANNEL_IN_MONO,
                        AudioFormat.ENCODING_PCM_16BIT, maxOf(minBuffer, CHUNK_BYTES * 8))
        } catch (e: Exception) {
            Log.e(TAG, "No microphone", e)
            return false
        }
        if (record.state != AudioRecord.STATE_INITIALIZED) {
            record.release()
            return false
        }
        val codec = try {
            val mime = if (aac) MediaFormat.MIMETYPE_AUDIO_AAC else MediaFormat.MIMETYPE_AUDIO_OPUS
            MediaCodec.createEncoderByType(mime).apply {
                configure(MediaFormat.createAudioFormat(mime, RATE, 1).apply {
                    setInteger(MediaFormat.KEY_BIT_RATE, if (aac) 128_000 else 80_000)
                    setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, CHUNK_BYTES * 4)
                    if (aac) setInteger(MediaFormat.KEY_AAC_PROFILE, android.media.MediaCodecInfo.CodecProfileLevel.AACObjectLC)
                    setInteger(MediaFormat.KEY_PRIORITY, 0)
                }, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                start()
            }
        } catch (e: Exception) {
            Log.e(TAG, "No Opus encoder", e)
            record.release()
            return false
        }
        running = true
        thread = Thread({ run(record, codec) }, "audio-encoder").apply {
            priority = Thread.MAX_PRIORITY
            start()
        }
        return true
    }

    fun stop() {
        running = false
        thread?.join(1000)
        thread = null
    }

    private fun run(record: AudioRecord, codec: MediaCodec) {
        val pcm = ByteBuffer.allocateDirect(CHUNK_BYTES).order(ByteOrder.nativeOrder())
        val info = MediaCodec.BufferInfo()
        val ts = AudioTimestamp()
        var framesRead = 0L
        var delayNs = if (aac) AAC_DELAY_NS else OPUS_DELAY_NS
        record.startRecording()
        try {
            while (running) {
                pcm.clear()
                val n = record.read(pcm, CHUNK_BYTES, AudioRecord.READ_BLOCKING)
                if (n <= 0) continue
                // Capture time of this chunk's first frame, from the latest
                // (frame position, boot time) pair the HAL reported.
                val captureNs = if (record.getTimestamp(ts, AudioTimestamp.TIMEBASE_BOOTTIME) == AudioRecord.SUCCESS) {
                    ts.nanoTime + (framesRead - ts.framePosition) * 1_000_000_000L / RATE
                } else {
                    android.os.SystemClock.elapsedRealtimeNanos() - n / 2 * 1_000_000_000L / RATE
                }
                framesRead += n / 2
                val input = codec.dequeueInputBuffer(10_000)
                if (input >= 0) {
                    val buf = codec.getInputBuffer(input)!!
                    buf.clear()
                    pcm.limit(n).position(0)
                    buf.put(pcm)
                    codec.queueInputBuffer(input, 0, n, captureNs / 1000, 0)
                }
                while (true) {
                    val out = codec.dequeueOutputBuffer(info, 0)
                    if (out == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                        delayNs = encoderDelayNs(codec.outputFormat) ?: delayNs
                        continue
                    }
                    if (out < 0) break
                    val buf = codec.getOutputBuffer(out)
                    if (buf != null && info.size > 0 && info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
                        buf.position(info.offset).limit(info.offset + info.size)
                        sink.onAudioPacket(buf, info.presentationTimeUs * 1000 - delayNs)
                    }
                    codec.releaseOutputBuffer(out, false)
                }
            }
        } catch (e: Exception) {
            Log.e(TAG, "Audio stopped", e)
        } finally {
            runCatching { record.stop() }
            record.release()
            runCatching { codec.stop() }
            codec.release()
        }
    }

    /**
     * The encoder delay: Opus's csd-1 (ns, little-endian), or an encoder's
     * "encoder-delay" (samples) where it reports one.
     */
    private fun encoderDelayNs(format: MediaFormat): Long? {
        if (format.containsKey(MediaFormat.KEY_ENCODER_DELAY)) {
            return format.getInteger(MediaFormat.KEY_ENCODER_DELAY) * 1_000_000_000L / RATE
        }
        if (aac) return null
        val csd = format.getByteBuffer("csd-1") ?: return null
        if (csd.remaining() < 8) return null
        return csd.duplicate().order(ByteOrder.LITTLE_ENDIAN).long
    }

    companion object {
        const val RATE = 48_000
        private const val TAG = "AudioEncoder"
        /** 10 ms of 16-bit mono. */
        private const val CHUNK_BYTES = RATE / 100 * 2
        /** Opus's usual pre-skip, 312 samples, until the encoder says. */
        private const val OPUS_DELAY_NS = 312L * 1_000_000_000L / RATE
        /**
         * AAC-LC priming, 2112 samples as docs/PROTOCOL.md ("MPEG-TS timing")
         * gives it, until the encoder reports its own. Not measured on Android
         * yet: check with sync-probe --av.
         */
        private const val AAC_DELAY_NS = 2112L * 1_000_000_000L / RATE
    }
}
