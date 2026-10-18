package se.eyevinn.tallycam.media

import android.media.MediaFormat
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.util.Log
import android.view.Surface
import se.eyevinn.tallycam.control.SrtUrl
import se.eyevinn.tallycam.core.Hot
import se.eyevinn.tallycam.core.SrtCaller
import se.eyevinn.tallycam.core.SrtEvent
import se.eyevinn.tallycam.core.SrtListener
import se.eyevinn.tallycam.core.srtCallerOptionsFromUrl
import java.nio.ByteBuffer
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.atomic.AtomicBoolean

/**
 * One live stream at a time: H.264 or HEVC from the camera's Surface and
 * Opus or AAC from the microphone, muxed by the core into MPEG-TS and queued
 * onto its SRT caller (the same path as the iOS app).
 *
 * Each start makes a [Session] with its own stream thread, muxer and
 * buffers: the encoders' callbacks and the muxer run on that thread only
 * (the core needs calls for one muxer serialized), and callbacks still
 * queued from a stopped session never reach the next one.
 *
 * Timestamps: the encoder's presentation times are the camera's sensor
 * timestamps; `timestampOffsetNs` turns them into the host clock
 * (CLOCK_BOOTTIME), 0 when the camera already stamps in it. Every frame
 * carries its TAI capture time in the timestamp SEI.
 */
class LiveStream(private val listener: Listener) {
    enum class State { IDLE, CONNECTING, LIVE }

    interface Listener {
        /** Main thread. `message`: why it is reconnecting. */
        fun onStreamState(state: State, message: String?)

        /**
         * Main thread: the stream cannot go on (the first connection failed,
         * or SRT gave up). Stop it the usual way: the camera's encoder
         * Surface first, then [stop].
         */
        fun onStreamFailed(reason: String)
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
        /** The encoder's bitrate now (below the operator's while adaptive bitrate holds it down). */
        val targetKbps: Int,
        /** What adaptive bitrate last saw: clear, congested or overloaded; null while it is off. */
        val link: String?,
    )

    private val main = Handler(Looper.getMainLooper())
    private var session: Session? = null
    private var lastBytes = 0L
    private var lastStatsNs = 0L

    var state = State.IDLE
        private set

    /**
     * Starts SRT, the muxer and the encoders. Returns the Surface the camera
     * must render into, or throws with a reason, having stopped whatever it
     * had started.
     */
    fun start(srtUrl: String, width: Int, height: Int, frameRate: Int, bitrateKbps: Int,
              timestampOffsetNs: Long, withAudio: Boolean, hevc: Boolean = false, aac: Boolean = false,
              adaptive: Boolean = true): Surface {
        check(state == State.IDLE && session == null) { "already live" }
        val s = Session(timestampOffsetNs)
        try {
            val surface = s.start(srtUrl, width, height, frameRate, bitrateKbps, withAudio, hevc, aac, adaptive)
            session = s
            lastBytes = 0
            lastStatsNs = Hot.hostNowNs()
            setState(State.CONNECTING, null)
            Log.i(TAG, "Streaming to ${SrtUrl.redacted(srtUrl)}")
            return surface
        } catch (e: Exception) {
            s.close()
            throw e
        }
    }

    /** Stops the stream. Call after the camera stopped rendering into the encoder Surface. */
    fun stop() {
        val s = session ?: return
        session = null
        s.close()
        setState(State.IDLE, null)
    }

    /** The operator's bitrate: with adaptive bitrate on, its ceiling. */
    fun setBitrate(kbps: Int) {
        session?.setBitrate(kbps)
    }

    /** Adaptive bitrate on or off while live; off returns to the operator's bitrate. */
    fun setAdaptive(on: Boolean) {
        session?.setAdaptive(on)
    }

    /** Since the last call. Main thread; the muxer's counters are read on the stream thread. */
    fun stats(): Stats? {
        val s = session ?: return null
        val srt = s.caller?.stats() ?: return null
        val now = Hot.hostNowNs()
        val bytes = srt.bytesSent.toLong()
        val kbps = if (now > lastStatsNs) ((bytes - lastBytes) * 8 * 1_000_000L / (now - lastStatsNs)).toInt() else 0
        lastBytes = bytes
        lastStatsNs = now
        val (late, latency) = s.takeStats()
        return Stats(kbps, srt.rttMs, srt.retransmittedPackets.toLong(), srt.droppedPackets.toLong(),
                     srt.negotiatedLatencyMs.toInt(), late.first, late.second, latency,
                     s.encoderKbps, if (s.adaptive) srt.link else null)
    }

    private fun setState(state: State, message: String?) {
        this.state = state
        main.post { listener.onStreamState(state, message) }
    }

    /** SRT events of one session, on the main thread. */
    private fun onSrtEvent(s: Session, event: SrtEvent) {
        if (session !== s) return
        when (event) {
            is SrtEvent.Connecting -> Unit
            is SrtEvent.Connected -> {
                s.everConnected = true
                setState(State.LIVE, null)
            }
            is SrtEvent.Disconnected ->
                // A first connection that fails stops the stream; a lost one
                // keeps encoding and connects again until SRT gives up
                // (docs/PROTOCOL.md, "Stream URL").
                if (s.everConnected && event.reconnecting) setState(State.CONNECTING, event.reason)
                else listener.onStreamFailed(event.reason)
        }
    }

    private inner class Session(private val timestampOffsetNs: Long) {
        private val thread = HandlerThread("stream", android.os.Process.THREAD_PRIORITY_URGENT_AUDIO).also { it.start() }
        private val handler = Handler(thread.looper)
        var caller: SrtCaller? = null
            private set
        /** The muxer; stream thread only after start. 0 once closed. */
        @Volatile private var mux = 0L
        private var video: VideoEncoder? = null
        private var audio: AudioEncoder? = null
        @Volatile var everConnected = false
        @Volatile private var closed = false
        private var frameCounter = 0
        /** A keyframe was asked for and has not come out yet. */
        private var keyframeRequested = false
        @Volatile var adaptive = false
            private set
        /** The operator's bitrate (the ceiling) and what the encoder runs now. */
        @Volatile private var ceilingKbps = 0
        @Volatile var encoderKbps = 0
            private set
        private var avcc: ByteBuffer = ByteBuffer.allocateDirect(256 * 1024)
        private val config: ByteBuffer = ByteBuffer.allocateDirect(1024)
        /** Stream thread only: the clock mapping of the frame being muxed. */
        private val mapping = LongArray(5)
        private val tsStats = LongArray(5)

        // Stats the stream thread collects for the main thread.
        private val statsLock = Any()
        private val captureToMux = LongArray(512)
        private var captureToMuxCount = 0
        private var lateVideo = 0L
        private var lateAudio = 0L

        /** Audio packets copied off the encoder thread: a fixed pool, reused (no allocation per packet). */
        private inner class Packet(val buffer: ByteBuffer) {
            var ptsNs = 0L
        }
        private val audioFree = ArrayBlockingQueue<Packet>(AUDIO_PACKETS)
        private val audioReady = ArrayBlockingQueue<Packet>(AUDIO_PACKETS)
        private val drainPosted = AtomicBoolean(false)
        private val drainAudio = Runnable {
            drainPosted.set(false)
            while (true) {
                val p = audioReady.poll() ?: break
                if (mux != 0L) Hot.tsAudio(mux, p.buffer, p.buffer.limit(), p.ptsNs, Hot.hostNowNs())
                audioFree.offer(p)
            }
        }

        fun start(srtUrl: String, width: Int, height: Int, frameRate: Int, bitrateKbps: Int,
                  withAudio: Boolean, hevc: Boolean, aac: Boolean, adaptive: Boolean): Surface {
            val options = srtCallerOptionsFromUrl(srtUrl).copy(reconnect = true)
            val session = this
            val caller = SrtCaller.start(options, object : SrtListener {
                override fun onEvent(event: SrtEvent) {
                    main.post { onSrtEvent(session, event) }
                }

                override fun onFrameSent(captureNs: Long, enqueuedNs: Long, sentNs: Long, last: Boolean) = Unit
            })
            this.caller = caller
            // The microphone first: the muxer only declares an audio stream
            // that will carry data.
            var audioCodec = Hot.AUDIO_NONE
            if (withAudio) {
                repeat(AUDIO_PACKETS) { audioFree.offer(Packet(ByteBuffer.allocateDirect(1500))) }
                val audio = AudioEncoder(aac, audioSink)
                if (audio.start()) {
                    this.audio = audio
                    audioCodec = if (aac) Hot.AUDIO_AAC else Hot.AUDIO_OPUS
                } else {
                    Log.w(TAG, "Streaming without audio")
                }
            }
            val mux = Hot.tsNew(caller.hotHandle().toLong(), if (hevc) Hot.VIDEO_HEVC else Hot.VIDEO_H264, audioCodec, 0)
            check(mux != 0L) { "no muxer" }
            this.mux = mux
            if (audioCodec != Hot.AUDIO_NONE) Hot.tsSetAudioFormat(mux, AudioEncoder.RATE, 1)
            val mime = if (hevc) MediaFormat.MIMETYPE_VIDEO_HEVC else MediaFormat.MIMETYPE_VIDEO_AVC
            val video = VideoEncoder(mime, width, height, frameRate, bitrateKbps, handler, videoSink)
            this.video = video
            video.start()
            ceilingKbps = bitrateKbps
            encoderKbps = bitrateKbps
            this.adaptive = adaptive
            if (adaptive) startAdaptive()
            return video.inputSurface
        }

        /**
         * Adaptive bitrate on the core's SRT thread (core/src/bitrate): its
         * reports set the encoder, on the stream thread, as on iOS.
         */
        private fun startAdaptive() {
            caller?.startAdaptiveBitrate(ceilingKbps.toUInt(), bitrateListener)
        }

        private val bitrateListener = object : se.eyevinn.tallycam.core.BitrateListener {
            override fun onBitrate(kbps: UInt) {
                handler.post {
                    // A report from before adaptive bitrate was turned off.
                    if (closed || !adaptive) return@post
                    encoderKbps = kbps.toInt()
                    video?.setBitrate(encoderKbps)
                }
            }
        }

        fun setBitrate(kbps: Int) {
            ceilingKbps = kbps
            if (adaptive) {
                // The controller answers through onBitrate.
                caller?.setAdaptiveCeiling(kbps.toUInt())
                return
            }
            val video = video ?: return
            handler.post {
                if (!closed) {
                    encoderKbps = kbps
                    video.setBitrate(kbps)
                }
            }
        }

        fun setAdaptive(on: Boolean) {
            if (on == adaptive || closed) return
            adaptive = on
            if (on) {
                startAdaptive()
            } else {
                caller?.stopAdaptiveBitrate()
                setBitrate(ceilingKbps)
            }
        }

        /** Main thread: the late counters (read on the stream thread at the last tick) and the latency window. */
        fun takeStats(): Pair<Pair<Long, Long>, Pair<Double, Double>?> {
            handler.post {
                if (mux != 0L) {
                    Hot.tsStats(mux, tsStats)
                    synchronized(statsLock) {
                        lateVideo = tsStats[0]
                        lateAudio = tsStats[1]
                    }
                }
            }
            return synchronized(statsLock) {
                val n = captureToMuxCount
                captureToMuxCount = 0
                val latency = if (n == 0) null else {
                    captureToMux.sort(0, n)
                    captureToMux[n / 2] / 1e6 to captureToMux[n - 1] / 1e6
                }
                (lateVideo to lateAudio) to latency
            }
        }

        /** Idempotent; also cleans up a start that threw half way. */
        fun close() {
            if (closed) return
            closed = true
            audio?.stop()
            audio = null
            // Encoder callbacks and the muxer run on the stream thread: tear
            // them down there, in order, after anything already queued.
            val video = video
            val caller = caller
            handler.post {
                video?.stop()
                val m = mux
                mux = 0
                Hot.tsFree(m)
                caller?.shutdown()
                caller?.close()
                thread.quitSafely()
            }
            this.video = null
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
                // tai, utc, est error, max error, flags (1 synced, 2 disciplined): no object per frame.
                Hot.clockMap(hostNs, mapping)
                val synced = mapping[4] and 1L != 0L
                val flags = Hot.SEI_FLAG_TAI or (if (synced) Hot.SEI_FLAG_SYNCED else 0)
                // The clock's maximum error when synced, else unknown (as on iOS, docs/PROTOCOL.md).
                val uncertainty = Hot.seiUncertaintyUs(if (synced) mapping[3] else -1)
                frameCounter++
                // `now` is the PCR's clock (PCR = now - PCR delay): the host clock now.
                val now = Hot.hostNowNs()
                // Send capture 0: no frame marks, so the SRT thread makes no
                // onFrameSent calls (two JNA callbacks a frame), which Android
                // does not use.
                Hot.tsVideo(mux, avcc, len, keyframe, hostNs, 0, mapping[0], flags, frameCounter, uncertainty, now)
                // The first keyframe came too late for its PCR (the encoder
                // warming up): ask for another now rather than wait a GOP.
                if (keyframe) keyframeRequested = false
                if (!keyframeRequested && Hot.tsWantsKeyframe(mux)) {
                    keyframeRequested = true
                    video?.requestKeyframe()
                }
                synchronized(statsLock) {
                    if (captureToMuxCount < captureToMux.size) captureToMux[captureToMuxCount++] = now - hostNs
                }
            }

            override fun onVideoError(reason: String) = fail(reason)
        }

        // Audio thread: copy into a pooled packet, then mux on the stream thread.
        private val audioSink = object : AudioEncoder.Sink {
            override fun onAudioPacket(packet: ByteBuffer, ptsNs: Long) {
                if (closed) return
                val p = audioFree.poll() ?: return
                if (packet.remaining() > p.buffer.capacity()) {
                    audioFree.offer(p)
                    return
                }
                p.buffer.clear()
                p.buffer.put(packet).flip()
                p.ptsNs = ptsNs
                audioReady.offer(p)
                // One drain per burst; Message objects come from Android's pool.
                if (!drainPosted.getAndSet(true)) handler.post(drainAudio)
            }

            override fun onAudioError(reason: String) = fail(reason)
        }

        /** An encoder failed: the stream stops, as when SRT gives up. */
        private fun fail(reason: String) {
            main.post { if (session === this@Session && !closed) listener.onStreamFailed(reason) }
        }
    }

    private companion object {
        const val TAG = "LiveStream"
        /** About 640 ms of 20 ms Opus packets (AAC's are 21 ms). */
        const val AUDIO_PACKETS = 32
    }
}
