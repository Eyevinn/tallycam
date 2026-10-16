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

        /** The decoded query values; null when unusable as a whole. */
        fun from(control: String?, srt: String?): ProvisioningLink? {
            if (control == null || !(control.startsWith("ws://") || control.startsWith("wss://"))) return null
            if (srt != null && !srt.startsWith("srt://")) return null
            return ProvisioningLink(control, srt?.ifEmpty { null })
        }

        /** "cam-" and the first 8 alphanumerics of the device id, lowercased. */
        fun streamId(deviceId: String): String =
            "cam-" + deviceId.filter { it.isLetterOrDigit() }.take(8).lowercase()

        /** The SRT URL with `{device}` replaced by this phone's stream id. */
        fun resolveSrtUrl(url: String, deviceId: String): String = url.replace("{device}", streamId(deviceId))
    }
}
