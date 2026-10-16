package se.eyevinn.tallycam

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import se.eyevinn.tallycam.control.CameraName
import se.eyevinn.tallycam.control.ProvisioningLink

class CameraNameTest {
    @Test
    fun defaultIsModelAndStartOfId() {
        assertEquals("Pixel 8 · 55dd", CameraName.display(null, "Pixel 8", "55DD2703ab"))
        assertEquals("Cam 2", CameraName.display("  Cam 2 ", "Pixel 8", "55dd"))
        assertEquals("Pixel 8", CameraName.display("", "Pixel 8", null))
        assertNull(CameraName.normalized(" \n"))
        assertEquals(CameraName.MAX_LENGTH, CameraName.normalized("x".repeat(100))!!.length)
    }

    @Test
    fun modelNames() {
        assertEquals("Pixel 8", CameraName.model("Google", "Pixel 8"))
        assertEquals("Samsung SM-S921B", CameraName.model("samsung", "SM-S921B"))
        assertEquals("sdk_gphone64_arm64", CameraName.model("Google", "sdk_gphone64_arm64").removePrefix("Google "))
    }

    @Test
    fun provisioningLink() {
        val link = ProvisioningLink.from("ws://192.0.2.5:8090/lcp/v0/device", "srt://192.0.2.5:9000?streamid={device}")!!
        assertEquals("srt://192.0.2.5:9000?streamid=cam-55dd2703",
                     ProvisioningLink.resolveSrtUrl(link.srtUrl!!, "55DD2703abcdef"))
        assertNull(ProvisioningLink.from("http://x", null))
        assertNull(ProvisioningLink.from("ws://x", "rtmp://y"))
    }
}
