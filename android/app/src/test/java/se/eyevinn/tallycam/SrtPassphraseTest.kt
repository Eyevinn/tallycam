package se.eyevinn.tallycam

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** A redacted passphrase is filled in only for the same SRT target and streamid (security review). */
class SrtPassphraseTest {
    private val current = "srt://10.0.0.5:9000?streamid={device}&latency=120&passphrase=s3cret-pass"

    @Test
    fun keepsItForTheSameTarget() {
        assertEquals("srt://10.0.0.5:9000?streamid={device}&latency=200&passphrase=s3cret-pass",
                     SrtPassphrase.restore("srt://10.0.0.5:9000?streamid={device}&latency=200&passphrase=***", current))
    }

    @Test
    fun leavesAnUnredactedUrlAlone() {
        val url = "srt://other:9000?streamid=x&passphrase=new-pass-123"
        assertEquals(url, SrtPassphrase.restore(url, current))
        assertEquals("srt://other:9000", SrtPassphrase.restore("srt://other:9000", current))
    }

    @Test
    fun refusesAnotherTarget() {
        assertNull(SrtPassphrase.restore("srt://attacker:9000?streamid={device}&passphrase=***", current))
        assertNull(SrtPassphrase.restore("srt://10.0.0.5:9001?streamid={device}&passphrase=***", current))
        assertNull(SrtPassphrase.restore("srt://10.0.0.5:9000?streamid=other&passphrase=***", current))
    }

    @Test
    fun refusesWithoutAPassphraseToKeep() {
        assertNull(SrtPassphrase.restore("srt://10.0.0.5:9000?streamid={device}&passphrase=***",
                                         "srt://10.0.0.5:9000?streamid={device}"))
    }

    @Test
    fun fillsInEveryRedactedOneWithTheLast() {
        assertEquals("srt://10.0.0.5:9000?streamid={device}&passphrase=second-pass&passphrase=second-pass",
                     SrtPassphrase.restore("srt://10.0.0.5:9000?streamid={device}&passphrase=***&passphrase=***",
                                           "srt://10.0.0.5:9000?streamid={device}&passphrase=first-pass&passphrase=second-pass"))
    }
}
