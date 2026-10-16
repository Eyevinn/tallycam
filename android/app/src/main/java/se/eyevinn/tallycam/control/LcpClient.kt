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
    private var socket: WebSocket? = null
    private var url: String? = null
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

            override fun onMessage(webSocket: WebSocket, text: String) = main.post {
                if (gen == generation) {
                    runCatching { JSONObject(text) }.getOrNull()?.let(listener::onMessage)
                }
            }.let { }

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
