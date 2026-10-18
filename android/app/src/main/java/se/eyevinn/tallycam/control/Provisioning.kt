package se.eyevinn.tallycam.control

import android.net.Uri

/**
 * `tallycam://connect?control=<ws url>&srt=<srt url>` (docs/PROTOCOL.md,
 * "Deep link"), the same link the iOS app takes.
 */
data class ProvisioningLink(val controlUrl: String, val srtUrl: String?) {
    companion object {
        const val SCHEME = "tallycam"

        fun parse(uri: Uri): ProvisioningLink? =
            if (uri.scheme != SCHEME || uri.host != "connect") null
            else from(uri.getQueryParameter("control"), uri.getQueryParameter("srt"))

        /**
         * The decoded query values; null when unusable as a whole: no host,
         * or unencrypted `ws://` to anything but a local network host
         * ([isLocalHost]), where a man in the middle on the way could drive
         * the camera.
         */
        fun from(control: String?, srt: String?): ProvisioningLink? {
            if (control == null) return null
            val scheme = control.substringBefore("://", "").lowercase()
            if (scheme != "ws" && scheme != "wss") return null
            val host = host(control) ?: return null
            if (scheme == "ws" && !isLocalHost(host)) return null
            if (srt != null && !srt.startsWith("srt://")) return null
            return ProvisioningLink(control, srt?.ifEmpty { null })
        }

        /** The host of a `scheme://[user@]host[:port]/...` URL, lowercase, IPv6 without brackets; null without one. */
        fun host(url: String): String? {
            val rest = url.substringAfter("://", "")
            val authority = rest.substringBefore('/').substringBefore('?').substringBefore('#').substringAfterLast('@')
            val host = if (authority.startsWith("[")) authority.substringAfter('[').substringBefore(']')
                       else authority.substringBefore(':')
            return host.lowercase().ifEmpty { null }
        }

        /**
         * A host on the local network, where `ws://` is allowed: a private,
         * loopback or link-local address, `localhost`, a `.local` name or
         * one without a dot (resolved on the LAN, not in public DNS).
         */
        fun isLocalHost(host: String): Boolean {
            if (host == "localhost" || host.endsWith(".local") || ('.' !in host && ':' !in host)) return true
            val v4 = host.split('.').map { it.toIntOrNull() }
            if (v4.size == 4 && v4.all { it != null && it in 0..255 }) {
                val (a, b) = v4[0]!! to v4[1]!!
                return a == 10 || a == 127 || (a == 172 && b in 16..31) || (a == 192 && b == 168) ||
                    (a == 169 && b == 254) || (a == 100 && b in 64..127)
            }
            if (':' in host) {
                val first = host.substringBefore(':').ifEmpty { "0" }.toIntOrNull(16) ?: return false
                return host == "::1" || first and 0xfe00 == 0xfc00 || first and 0xffc0 == 0xfe80
            }
            return false
        }

        /** "cam-" and the first 8 alphanumerics of the device id, lowercased. */
        fun streamId(deviceId: String): String =
            "cam-" + deviceId.filter { it.isLetterOrDigit() }.take(8).lowercase()

        /** The SRT URL with `{device}` replaced by this phone's stream id. */
        fun resolveSrtUrl(url: String, deviceId: String): String = url.replace("{device}", streamId(deviceId))
    }
}
