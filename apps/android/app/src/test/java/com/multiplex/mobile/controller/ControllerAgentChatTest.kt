package com.multiplex.mobile.controller

import java.lang.reflect.Proxy
import java.util.Base64
import java.util.UUID
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ControllerAgentChatTest {
    @Test
    fun answersDecodeFromTheWire() {
        val id = UUID.randomUUID().toString()
        val answer = """
            {"kind":"agent_chat","command_id":"$id","session_id":"s",
             "chat":{"provider":"claude_code","title":"Fix the build","state":"working","revision":"r1",
             "unchanged":false,"earlier":true,"history_truncated":false,
             "items":[{"id":"u1","kind":"user","title":null,"text":"Why?","tool_state":null,"truncated":false},
                      {"id":"t1","kind":"tool","title":"Bash · cargo build","text":"error","tool_state":"failed","truncated":true},
                      {"id":"n1","kind":"something_new","title":null,"text":"x","tool_state":null,"truncated":false}]}}
        """.trimIndent()
        val chat = AgentChat.decodeAnswer(answer, id)!!
        assertEquals(AgentProvider.CLAUDE_CODE, chat.provider)
        assertEquals(AgentChatState.WORKING, chat.state)
        assertEquals(listOf(AgentChatItemKind.USER, AgentChatItemKind.TOOL, AgentChatItemKind.NOTICE), chat.items.map { it.kind })
        assertEquals(AgentToolState.FAILED, chat.items[1].toolState)
        assertNull(AgentChat.decodeAnswer("""{"kind":"agent_chat","command_id":"$id","chat":null}""", id))
        try {
            AgentChat.decodeAnswer("""{"kind":"error","code":"unsupported_command"}""", id)
            fail("an error answer throws")
        } catch (error: ControllerConnectionException.HostError) {
            assertEquals("unsupported_command", error.code)
        }
    }

    @Test
    fun codeFencesSplitFromProse() {
        assertEquals(
            listOf(
                ChatSegment.Prose("Here:"),
                ChatSegment.Code("rust", "let x = 1;"),
                ChatSegment.Prose("Done."),
                ChatSegment.Code(null, "unclosed"),
            ),
            ChatSegment.split("Here:\n```rust\nlet x = 1;\n```\nDone.\n```\nunclosed"),
        )
    }

    @Test
    fun newestPagesMergeBehindWhatWasReadBack() = runTest {
        val model = ControllerAgentChatModel(this, host(), GitTarget(UUID.randomUUID().toString(), 1), null)
        model.apply(chat("r1", listOf("a", "b", "c"), earlier = true))
        model.apply(chat("r2", listOf("b", "c", "d"), earlier = true))
        assertEquals(listOf("a", "b", "c", "d"), model.state.value.items.map { it.id })
        model.apply(chat("r2", emptyList(), earlier = false, unchanged = true, state = AgentChatState.READY))
        assertEquals(listOf("a", "b", "c", "d"), model.state.value.items.map { it.id })
        assertEquals(AgentChatState.READY, model.state.value.chat?.state)
        model.apply(chat("r3", listOf("x"), earlier = false))
        assertEquals(listOf("x"), model.state.value.items.map { it.id })
        model.apply(null)
        assertFalse(model.state.value.available)
    }

    @Test
    fun findsTheAgentReadsEarlierAndStopsWhereUnsupported() = runTest {
        val connection = FixtureChatConnection()
        val model = ControllerAgentChatModel(this, host(), GitTarget(UUID.randomUUID().toString(), 1), connection)
        model.refresh()
        assertTrue(model.state.value.available)
        assertEquals(listOf("m3", "m4"), model.state.value.items.map { it.id })
        model.loadEarlier()
        advanceUntilIdle()
        assertEquals(listOf("m1", "m2", "m3", "m4"), model.state.value.items.map { it.id })
        assertFalse(model.state.value.hasEarlier)
        assertEquals("m3", connection.requests.last().before)

        val old = ControllerAgentChatModel(this, host(), GitTarget(UUID.randomUUID().toString(), 1), null)
        old.start()
        advanceUntilIdle()
        assertFalse("a route without the chat is not asked again", old.state.value.available)
    }

    private fun chat(
        revision: String,
        ids: List<String>,
        earlier: Boolean,
        unchanged: Boolean = false,
        state: AgentChatState = AgentChatState.WORKING,
    ) = AgentChat(
        provider = AgentProvider.CODEX,
        state = state,
        revision = revision,
        unchanged = unchanged,
        items = ids.map { AgentChatItem(id = it, kind = AgentChatItemKind.ASSISTANT, text = it) },
        earlier = earlier,
    )

    private fun host() = PairedHostRecord(
        id = "host",
        displayName = "Build Mac",
        route = HostRoute("192.168.1.10", 22_222),
        hostStaticPublicKey = Base64.getEncoder().encodeToString(ByteArray(32) { 7 }),
        deviceStaticKeyId = "controller.device.host",
        deviceId = UUID.randomUUID().toString(),
        identityGeneration = 1,
        revocationEpoch = 1,
        sessionGeneration = 1,
        capabilityBits = 6,
        pairedAtMillis = 1,
    )

    private inner class FixtureChatConnection : ControllerConnecting by unsupported() {
        val requests = mutableListOf<AgentChatRequest>()

        override suspend fun agentChat(host: PairedHostRecord, target: GitTarget, request: AgentChatRequest): AgentChat? {
            requests += request
            return if (request.before == "m3") {
                chat("r1", listOf("m1", "m2"), earlier = false)
            } else {
                chat("r1", listOf("m3", "m4"), earlier = true)
            }
        }

        override suspend fun cancel() = Unit
    }
}

private fun unsupported(): ControllerConnecting =
    Proxy.newProxyInstance(
        ControllerConnecting::class.java.classLoader,
        arrayOf(ControllerConnecting::class.java),
    ) { _, method, _ -> throw UnsupportedOperationException(method.name) } as ControllerConnecting
