package se.eyevinn.tallycam.media

import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.util.Log
import android.view.Surface
import se.eyevinn.tallycam.core.ClockMapping
import se.eyevinn.tallycam.core.Hot
import se.eyevinn.tallycam.core.SrtCaller
import se.eyevinn.tallycam.core.SrtEvent
import se.eyevinn.tallycam.core.SrtListener
import se.eyevinn.tallycam.core.srtCallerOptionsFromUrl
import java.nio.ByteBuffer
import java.util.concurrent.ArrayBlockingQueue

/**
 * One live stream: H.264 from the camera's Surface and Opus from the
 * microphone, muxed by the core into MPEG-TS and queued onto its SRT caller
 * (the same path as the iOS app). Muxing runs on one thread ("stream"); the
 * core needs calls for one muxer serialized.
 *
 * Timestamps: the encoder's presentation times are the camera's sensor
 * timestamps; `timestampOffsetNs` turns them into the host clock
 * (CLOCK_BOOTTIME), 0 when the camera already stamps in it. Every frame
 * carries its TAI capture time in the timestamp SEI.
 */
class LiveStream(private val listener: Listener) {
    enum class State { IDLE, CONNECTING, LIVE }

    interface Listener {
        /** Main thread. `message`: why it stopped or is reconnecting. */
        fun onStreamState(state: State, message: String?)
    }

    data class Stats(
        val bitrateKbps: Int,
        val rttMs: Double,
        val retransmits: Long,
        val dropped: Long,
        val srtLatencyMs: Int,
        val lateVideoFrames: Long,
        val lateAudioFrames: Long,
        /** Capture to muxed, ms over the last interval: p50 and max; null without frames. */
        val captureToMuxMs: Pair<Double, Double>?,
    )

    private val main = Handler(Looper.getMainLooper())
    private var thread: HandlerThread? = null
    private var handler: Handler? = null
    private var caller: SrtCaller? = null
    @Volatile private var mux = 0L
    private var video: VideoEncoder? = null
    private var audio: AudioEncoder? = null
    private var timestampOffsetNs = 0L
    private var frameCounter = 0
    private var avcc: ByteBuffer = ByteBuffer.allocateDirect(256 * 1024)
    private val config: ByteBuffer = ByteBuffer.allocateDirect(1024)
    /** Audio packets copied off the encoder thread, reused. */
    private val audioPool = ArrayBlockingQueue<ByteBuffer>(32)
    /** Capture-to-mux times since the last stats(), ns; stream thread writes, main reads under the lock. */
    private val captureToMux = LongArray(512)
    private var captureToMuxCount = 0
    private val captureToMuxLock = Any()
    private var lastBytes = 0L
    private var lastStatsNs = 0L

    var state = State.IDLE
        private set

    /**
     * Starts SRT, the muxer and the encoders. Returns the Surface the camera
     * must render into, or throws with a reason.
     */
    fun start(srtUrl: String, width: Int, height: Int, frameRate: Int, bitrateKbps: Int,
              timestampOffsetNs: Long, withAudio: Boolean, hevc: Boolean = false, aac: Boolean = false): Surface {
        check(state == State.IDLE) { "already live" }
        val options = srtCallerOptionsFromUrl(srtUrl).copy(reconnect = true)
        val thread = HandlerThread("stream", android.os.Process.THREAD_PRIORITY_URGENT_AUDIO).also { it.start() }
        val handler = Handler(thread.looper)
        this.thread = thread
        this.handler = handler
        this.timestampOffsetNs = timestampOffsetNs
        frameCounter = 0
        val caller = SrtCaller.start(options, srtListener)
        this.caller = caller
        mux = Hot.tsNew(caller.hotHandle().toLong(), if (hevc) Hot.VIDEO_HEVC else Hot.VIDEO_H264,
                        when {
                            !withAudio -> Hot.AUDIO_NONE
                            aac -> Hot.AUDIO_AAC
                            else -> Hot.AUDIO_OPUS
                        }, 0)
        check(mux != 0L) { "no muxer" }
        if (withAudio) {
            Hot.tsSetAudioFormat(mux, AudioEncoder.RATE, 1)
            repeat(32) { audioPool.offer(ByteBuffer.allocateDirect(1500)) }
            val audio = AudioEncoder(aac, audioSink)
            if (audio.start()) this.audio = audio else Log.w(TAG, "Streaming without audio")
        }
        val mime = if (hevc) android.media.MediaFormat.MIMETYPE_VIDEO_HEVC else android.media.MediaFormat.MIMETYPE_VIDEO_AVC
        val video = VideoEncoder(mime, width, height, frameRate, bitrateKbps, handler, videoSink)
        video.start()
        this.video = video
        lastBytes = 0
        lastStatsNs = Hot.hostNowNs()
        setState(State.CONNECTING, null)
        Log.i(TAG, "Streaming to ${se.eyevinn.tallycam.control.SrtUrl.redacted(srtUrl)}")
        return video.inputSurface
    }

    fun stop(reason: String? = null) {
        if (state == State.IDLE) return
        audio?.stop()
        audio = null
        val handler = handler
        val thread = thread
        val caller = caller
        val video = video
        val mux = mux
        this.mux = 0
        this.video = null
        this.caller = null
        this.handler = null
        this.thread = null
        // Encoder callbacks and the muxer run on the stream thread: tear
        // them down there, in order.
        handler?.post {
            video?.stop()
            Hot.tsFree(mux)
            caller?.shutdown()
            thread?.quitSafely()
        }
        audioPool.clear()
        setState(State.IDLE, reason)
    }

    fun setBitrate(kbps: Int) {
        val video = video ?: return
        handler?.post { video.setBitrate(kbps) }
    }

    /** Since the last call. Main thread. */
    fun stats(): Stats? {
        val caller = caller ?: return null
        val s = caller.stats()
        val now = Hot.hostNowNs()
        val bytes = s.bytesSent.toLong()
        val kbps = if (now > lastStatsNs) ((bytes - lastBytes) * 8 * 1_000_000L / (now - lastStatsNs)).toInt() else 0
        lastBytes = bytes
        lastStatsNs = now
        val ts = LongArray(5)
        if (mux != 0L) Hot.tsStats(mux, ts)
        val latency = synchronized(captureToMuxLock) {
            val n = captureToMuxCount
            captureToMuxCount = 0
            if (n == 0) null else captureToMux.copyOf(n).sorted().let { it[n / 2] / 1e6 to it[n - 1] / 1e6 }
        }
        return Stats(kbps, s.rttMs, s.retransmittedPackets.toLong(), s.droppedPackets.toLong(),
                     s.negotiatedLatencyMs.toInt(), ts[0], ts[1], latency)
    }

    private fun setState(state: State, message: String?) {
        this.state = state
        main.post { listener.onStreamState(state, message) }
    }

    private val srtListener = object : SrtListener {
        override fun onEvent(event: SrtEvent) {
            main.post {
                if (state == State.IDLE) return@post
                when (event) {
                    is SrtEvent.Connecting -> Unit
                    is SrtEvent.Connected -> setState(State.LIVE, null)
                    is SrtEvent.Disconnected ->
                        // A first connection that fails stops the stream; a
                        // lost one keeps encoding and connects again.
                        if (state == State.LIVE && event.reconnecting) setState(State.CONNECTING, event.reason)
                        else stop(event.reason)
                }
            }
        }

        override fun onFrameSent(captureNs: Long, enqueuedNs: Long, sentNs: Long, last: Boolean) = Unit
    }

    // Stream thread.
    private val videoSink = object : VideoEncoder.Sink {
        override fun onVideoFormat(parameterSets: ByteBuffer) {
            if (mux == 0L || parameterSets.remaining() > config.capacity()) return
            config.clear()
            config.put(parameterSets).flip()
            if (!Hot.tsSetVideoFormat(mux, config, config.limit(), 4)) {
                Log.w(TAG, "Encoder parameter sets not accepted")
            }
        }

        override fun onVideoFrame(frame: ByteBuffer, ptsUs: Long, keyframe: Boolean) {
            if (mux == 0L) return
            val needed = AnnexB.capacityFor(frame.remaining())
            if (avcc.capacity() < needed) avcc = ByteBuffer.allocateDirect(needed * 2)
            val len = AnnexB.toLengthPrefixed(frame, avcc)
            if (len == 0) return
            val hostNs = ptsUs * 1000 + timestampOffsetNs
            val m = ClockMapping.at(hostNs)
            val flags = Hot.SEI_FLAG_TAI or (if (m.synced) Hot.SEI_FLAG_SYNCED else 0)
            val uncertainty = Hot.seiUncertaintyUs(if (m.disciplined) m.estErrorNs else -1)
            frameCounter++
            // `now` is the PCR's clock (PCR = now - PCR delay): the host clock now.
            val now = Hot.hostNowNs()
            Hot.tsVideo(mux, avcc, len, keyframe, hostNs, hostNs, m.taiNs, flags, frameCounter, uncertainty, now)
            synchronized(captureToMuxLock) {
                if (captureToMuxCount < captureToMux.size) captureToMux[captureToMuxCount++] = now - hostNs
            }
        }
    }

    // Audio thread: copy, then mux on the stream thread.
    private val audioSink = object : AudioEncoder.Sink {
        override fun onAudioPacket(packet: ByteBuffer, ptsNs: Long) {
            val handler = handler ?: return
            val buf = audioPool.poll() ?: return
            if (packet.remaining() > buf.capacity()) {
                audioPool.offer(buf)
                return
            }
            buf.clear()
            buf.put(packet).flip()
            handler.post {
                if (mux != 0L) Hot.tsAudio(mux, buf, buf.limit(), ptsNs, Hot.hostNowNs())
                audioPool.offer(buf)
            }
        }
    }

    private companion object {
        const val TAG = "LiveStream"
    }
}
