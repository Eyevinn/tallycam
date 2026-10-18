package se.eyevinn.tallycam.control

import android.os.Handler
import android.os.Looper
import android.util.Log
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/**
 * The WebSocket to the device endpoint (`/lcp/v0/device`). The phone always
 * connects out; it reconnects with a backoff (1 s doubling to 30 s) and
 * keeps the link alive with WebSocket pings. Callbacks run on the main
 * thread.
 */
/** Longest message accepted from the hub, characters; the largest real ones (a clock log) are well under. */
private const val MAX_INCOMING_CHARS = 1 shl 20

/** Bytes queued on the socket above which [LcpClient.sendDroppable] drops (about 4 previews). */
private const val DROPPABLE_QUEUE_BYTES = 256L * 1024

class LcpClient(private val listener: Listener) {
    interface Listener {
        fun onOpen()
        fun onMessage(message: JSONObject)
        fun onClosed(reason: String)
    }

    private val main = Handler(Looper.getMainLooper())
    private val http = OkHttpClient.Builder()
        .pingInterval(5, TimeUnit.SECONDS)
        .readTimeout(0, TimeUnit.MILLISECONDS)
        .build()
    /** Set on main; read on the preview thread too ([sendDroppable]). */
    @Volatile private var socket: WebSocket? = null
    /** The control URL this client dials (an `auth` is signed for its host). */
    var url: String? = null
        private set
    private var backoffMs = 1000L
    private var generation = 0

    val isOpen: Boolean get() = socket != null

    fun connect(url: String) {
        close()
        this.url = url
        backoffMs = 1000L
        open()
    }

    fun close() {
        generation++
        url = null
        main.removeCallbacksAndMessages(null)
        socket?.close(1000, "closing")
        socket = null
    }

    /** False when not connected. */
    fun send(message: JSONObject): Boolean = socket?.send(message.toString()) ?: false

    /**
     * For messages a newer one replaces (previews): dropped, false, while
     * more than [DROPPABLE_QUEUE_BYTES] wait to go out. OkHttp closes the
     * socket once 16 MiB are queued, so a slow link would otherwise turn
     * into a reconnect loop.
     */
    fun sendDroppable(message: JSONObject): Boolean {
        val socket = socket ?: return false
        if (socket.queueSize() > DROPPABLE_QUEUE_BYTES) return false
        return socket.send(message.toString())
    }

    /**
     * Whether [sendDroppable] would send now: connected and not behind. Any
     * thread; lets a caller skip building a preview (JPEG, Base64) the link
     * would drop.
     */
    fun canSendDroppable(): Boolean = socket?.let { it.queueSize() <= DROPPABLE_QUEUE_BYTES } ?: false

    /** Admitted: the next reconnect starts from a short delay again. */
    fun resetBackoff() {
        backoffMs = 1000L
    }

    private fun open() {
        val url = url ?: return
        val gen = generation
        val request = try {
            Request.Builder().url(url).build()
        } catch (e: IllegalArgumentException) {
            listener.onClosed("bad control URL")
            return
        }
        http.newWebSocket(request, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) = main.post {
                if (gen != generation) {
                    webSocket.close(1000, null)
                    return@post
                }
                socket = webSocket
                listener.onOpen()
            }.let { }

            override fun onMessage(webSocket: WebSocket, text: String) {
                // A hub (or a man in the middle) could run the phone out of
                // memory with one huge frame: close rather than parse it.
                if (text.length > MAX_INCOMING_CHARS) {
                    Log.w("LcpClient", "Message of ${text.length} characters, closing")
                    webSocket.close(1009, "message too big")
                    return
                }
                deliver(text)
            }

            private fun deliver(text: String) = main.post {
                if (gen == generation) {
                    runCatching { JSONObject(text) }.getOrNull()?.let(listener::onMessage)
                }
            }.let { }

            // The server closed: answer its close frame, or OkHttp never
            // reaches onClosed.
            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                webSocket.close(1000, null)
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) = lost(gen, reason.ifEmpty { "closed" })

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) =
                lost(gen, t.message ?: t.javaClass.simpleName)
        })
    }

    private fun lost(gen: Int, reason: String) {
        main.post {
            if (gen != generation) return@post
            socket = null
            listener.onClosed(reason)
            val delay = backoffMs
            backoffMs = (backoffMs * 2).coerceAtMost(30_000L)
            Log.i("LcpClient", "Closed ($reason), reconnecting in $delay ms")
            main.postDelayed({ if (gen == generation) open() }, delay)
        }
    }
}
