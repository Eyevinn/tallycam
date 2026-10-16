package se.eyevinn.tallycam

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import se.eyevinn.tallycam.control.SrtUrl

class SrtUrlTest {
    @Test
    fun setsAndReadsLatencyKeepingThePlaceholder() {
        val url = "srt://192.0.2.5:9000?streamid={device}&latency=100"
        assertEquals(100, SrtUrl.latency(url))
        assertEquals("srt://192.0.2.5:9000?streamid={device}&latency=250", SrtUrl.settingLatency(250, url))
        assertEquals("srt://192.0.2.5:9000?latency=80", SrtUrl.settingLatency(80, "srt://192.0.2.5:9000"))
        assertNull(SrtUrl.settingLatency(80, "rtmp://x"))
        assertNull(SrtUrl.latency("srt://h:1"))
    }

    @Test
    fun redactsThePassphrase() {
        assertEquals("srt://h:1?passphrase=***&latency=1", SrtUrl.redacted("srt://h:1?passphrase=secret123&latency=1"))
    }
}
