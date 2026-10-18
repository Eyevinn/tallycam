package se.eyevinn.tallycam

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test
import se.eyevinn.tallycam.control.ProvisioningLink

class ProvisioningTest {
    @Test
    fun aControlUrlNeedsAHostAndWsOnlyOnTheLocalNetwork() {
        for (ok in listOf("wss://hub.example.com/lcp/v0/device?token=x", "ws://192.168.1.20:8090/lcp/v0/device",
                          "ws://10.0.0.5:8090/d", "ws://studio-hub:8090/d", "ws://hub.local/d", "ws://[fe80::1]:8090/d",
                          "WS://localhost:8090/d")) {
            assertNotNull(ok, ProvisioningLink.from(ok, null))
        }
        for (bad in listOf("ws://", "ws://:8090/d", "wss://", "ws://hub.example.com/d", "ws://8.8.8.8:8090/d",
                           "ws://[2001:db8::1]:8090/d", "http://hub.local/d")) {
            assertNull(bad, ProvisioningLink.from(bad, null))
        }
        assertNull(ProvisioningLink.from("wss://hub.example.com/d", "rtmp://x"))
    }

    @Test
    fun hostIsTheAuthorityWithoutUserOrPort() {
        assertEquals("hub.example.com", ProvisioningLink.host("wss://u@Hub.Example.com:8443/x?y"))
        assertEquals("fe80::1", ProvisioningLink.host("ws://[fe80::1]:8090/d"))
        assertNull(ProvisioningLink.host("ws://"))
    }
}
