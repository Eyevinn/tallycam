package se.eyevinn.tallycam

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Test
import se.eyevinn.tallycam.control.ShadingMessages
import se.eyevinn.tallycam.media.CameraCapture.Shutter

/** The server's `production` message, read leniently as the core reads it (docs/SHADING.md, "Shutter"). */
class ProductionMessageTest {
    private fun parse(json: String) = ShadingMessages.production(JSONObject(json))

    @Test
    fun parses() {
        assertEquals(ShadingMessages.Production(50, Shutter.Mains), parse("""{"type":"production","mains_hz":50,"shutter":"mains"}"""))
        assertEquals(ShadingMessages.Production(0, Shutter.Free), parse("""{"type":"production","mains_hz":0,"shutter":"free","x":1}"""))
        assertEquals(ShadingMessages.Production(60, Shutter.Fixed(100)), parse("""{"type":"production","mains_hz":60,"shutter":100}"""))
    }

    @Test
    fun lenient() {
        // An unknown shutter is mains; an unknown mains frequency is none.
        assertEquals(ShadingMessages.Production(null, Shutter.Mains), parse("""{"type":"production","mains_hz":55,"shutter":"odd"}"""))
        assertEquals(Shutter.Mains, ShadingMessages.shutter(100.5))
        assertEquals(Shutter.Mains, ShadingMessages.shutter(0))
    }
}
