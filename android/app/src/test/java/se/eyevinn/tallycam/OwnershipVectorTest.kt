package se.eyevinn.tallycam

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import se.eyevinn.tallycam.control.ShadingMessages
import se.eyevinn.tallycam.core.OwnershipChange
import se.eyevinn.tallycam.core.OwnershipMachine
import java.io.File

/**
 * The shared ownership vectors (core/tests/vectors/lcp_ownership.json), as
 * on iOS: the server messages through the app's parser, and the scenarios
 * through the core's state machine as the app calls it. The core is a host
 * build of core/android, loaded through JNA (see the Gradle test setup).
 */
class OwnershipVectorTest {
    private val vectors = JSONObject(File(System.getProperty("tallycam.vectors")!!, "lcp_ownership.json").readText())

    private fun strings(array: JSONArray?): List<String> = (0 until (array?.length() ?: 0)).map { array!!.getString(it) }

    /** JSON as plain maps and lists, to compare by value. */
    private fun plain(value: Any?): Any? = when (value) {
        is JSONObject -> value.keys().asSequence().associateWith { plain(value.get(it)) }
        is JSONArray -> (0 until value.length()).map { plain(value.get(it)) }
        is Number -> value.toDouble()
        JSONObject.NULL -> null
        else -> value
    }

    @Test
    fun serverMessagesParse() {
        val messages = vectors.getJSONArray("messages")
        var parsed = 0
        for (i in 0 until messages.length()) {
            val vector = messages.getJSONObject(i)
            if (vector.getString("direction") != "server_to_device") continue
            val name = vector.getString("name")
            val json = vector.getJSONObject("json")
            val canonical = vector.getJSONObject("canonical")
            when (json.getString("type")) {
                "own" -> {
                    val own = ShadingMessages.own(json)
                    val holder = canonical.getJSONObject("holder")
                    assertEquals(name, strings(canonical.getJSONArray("groups")), own.groups)
                    assertEquals(name, holder.getString("id"), own.holderId)
                    assertEquals(name, holder.getString("label"), own.holderLabel)
                }
                "release" -> assertEquals(name, strings(canonical.getJSONArray("groups")), ShadingMessages.groups(json))
                "production" -> {
                    val production = ShadingMessages.production(json)
                    assertEquals(name, canonical.getInt("mains_hz"), production.mainsHz)
                    assertEquals(name, ShadingMessages.shutter(canonical.get("shutter")), production.shutter)
                }
                else -> throw AssertionError("$name: unexpected ${json.getString("type")}")
            }
            parsed++
        }
        assertTrue("parsed $parsed", parsed >= 7)
    }

    @Test
    fun scenariosThroughTheCore() {
        val scenarios = vectors.getJSONArray("scenarios")
        assertTrue(scenarios.length() >= 10)
        for (s in 0 until scenarios.length()) {
            val scenario = scenarios.getJSONObject(s)
            val name = scenario.getString("name")
            val groups = strings(scenario.getJSONArray("groups"))
            var machine = OwnershipMachine(groups, null)
            val steps = scenario.getJSONArray("steps")
            for (i in 0 until steps.length()) {
                val step = steps.getJSONObject(i)
                val at = "$name step $i"
                val group = step.optString("group")
                var result: String? = null
                when (val action = step.getString("do")) {
                    "connect" -> machine.setConnected(true)
                    "disconnect" -> machine.setConnected(false)
                    "restart" -> machine = OwnershipMachine(groups, machine.saveJson())
                    "own" -> {
                        val holder = step.optJSONObject("holder")
                        val taken = machine.own(strings(step.optJSONArray("groups")), holder?.optString("id").orEmpty(),
                                                holder?.optString("label").orEmpty())
                        if (step.has("taken")) assertEquals(at, strings(step.getJSONArray("taken")), taken.taken)
                        if (step.has("from_local")) assertEquals(at, strings(step.getJSONArray("from_local")), taken.fromLocal)
                        if (step.has("skipped")) assertEquals(at, strings(step.getJSONArray("skipped")), taken.skipped)
                    }
                    "release" -> {
                        val released = machine.release(strings(step.optJSONArray("groups")))
                        if (step.has("released")) assertEquals(at, strings(step.getJSONArray("released")), released)
                    }
                    "new_server" -> {
                        val released = machine.newServer()
                        if (step.has("released")) assertEquals(at, strings(step.getJSONArray("released")), released)
                    }
                    "remote_set" -> result = if (machine.checkRemote(group) == null) "changed" else "refused"
                    "local_change" -> result = code(machine.localChange(group))
                    "local_auto" -> result = code(machine.localAuto(group))
                    "take_local" -> result = code(machine.takeLocal(group))
                    else -> throw AssertionError("$at: unknown action $action")
                }
                if (step.has("result")) {
                    // The FFI reports refusals as text; the vectors name them.
                    val want = step.getString("result")
                    assertEquals(at, if (want == "changed" || want == "unchanged") want else "refused", result)
                }
                step.optJSONObject("ownership")?.let { want ->
                    assertEquals(at, plain(want), plain(JSONObject(machine.messageJson()).getJSONObject("groups")))
                }
                step.optJSONObject("indicators")?.let { want ->
                    val view = machine.view()
                    for (g in want.keys()) {
                        assertEquals("$at $g", want.getString(g), view.firstOrNull { it.group == g }?.indicator)
                    }
                }
            }
        }
    }

    private fun code(change: OwnershipChange) = when (change) {
        OwnershipChange.Changed -> "changed"
        OwnershipChange.Unchanged -> "unchanged"
        is OwnershipChange.Refused -> "refused"
    }
}
