package com.termirust.mobile.controller

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.UUID

class ControllerWriterTest {
    private val identity = ReadOnlyAttachIdentity(
        "host-fingerprint",
        UUID.fromString("00000000-0000-0000-0000-000000000001"),
        7,
    )

    @Test
    fun writerLeaseRequiresExactAcquireCommand() {
        val reducer = WriterControlReducer(identity)
        val command = UUID.randomUUID()
        reducer.beginAcquire(command)
        assertThrows(IllegalArgumentException::class.java) {
            reducer.finishAcquire(UUID.randomUUID(), applied = true)
        }
        reducer.finishAcquire(command, applied = true)
        assertEquals(WriterLeaseState.Held, reducer.lease)
    }

    @Test
    fun inputQueueEnforcesChunkCountAndByteCaps() {
        val reducer = heldReducer()
        repeat(16) {
            reducer.enqueue(ByteArray(16 * 1_024), PendingInputKind.KEYBOARD)
        }
        assertEquals(256 * 1_024, reducer.queuedBytes)
        assertThrows(IllegalArgumentException::class.java) {
            reducer.enqueue(byteArrayOf(1), PendingInputKind.KEYBOARD)
        }
        assertEquals(16 * 1_024, reducer.removeFirstOrNull()?.bytes?.size)

        val chunkCount = heldReducer()
        repeat(64) { chunkCount.enqueue(byteArrayOf(1), PendingInputKind.KEYBOARD) }
        assertThrows(IllegalArgumentException::class.java) {
            chunkCount.enqueue(byteArrayOf(1), PendingInputKind.KEYBOARD)
        }
    }

    @Test
    fun multilineAndLargePasteRequireExplicitConfirmation() {
        val reducer = heldReducer()
        assertTrue(reducer.pasteRequiresConfirmation("one\ntwo".encodeToByteArray()))
        assertFalse(reducer.pasteRequiresConfirmation(ByteArray(4 * 1_024)))
        assertTrue(reducer.pasteRequiresConfirmation(ByteArray(4 * 1_024 + 1)))
        assertThrows(IllegalArgumentException::class.java) {
            reducer.enqueue("one\ntwo".encodeToByteArray(), PendingInputKind.PASTE)
        }
        reducer.enqueue("one\ntwo".encodeToByteArray(), PendingInputKind.PASTE, confirmed = true)
    }

    @Test
    fun backgroundLosesLeaseAndDropsQueuedInput() {
        val reducer = heldReducer()
        reducer.enqueue("never replay".encodeToByteArray(), PendingInputKind.KEYBOARD)
        reducer.setForeground(false)
        assertEquals(WriterLeaseState.Lost, reducer.lease)
        assertEquals(0, reducer.queuedBytes)
        assertEquals(null, reducer.removeFirstOrNull())
    }

    @Test
    fun codecsBindGenerationIdentityAndBounds() {
        val commandId = UUID.randomUUID()
        val payload = ControllerWriterWireCodec.encodeInput(
            commandId,
            sessionGeneration = 11,
            deadlineMillis = 30_000,
            identity = identity,
            bytes = "ls\n".encodeToByteArray(),
        )
        val root = Json.parseToJsonElement(payload.decodeToString()).jsonObject
        val command = root.getValue("command").jsonObject
        assertEquals(commandId.toString(), root.getValue("command_id").jsonPrimitive.content)
        assertEquals("11", root.getValue("session_generation").jsonPrimitive.content)
        assertEquals(identity.sessionId.toString(), command.getValue("session_id").jsonPrimitive.content)
        assertEquals("7", command.getValue("occupant_generation").jsonPrimitive.content)
        assertThrows(IllegalArgumentException::class.java) {
            ControllerWriterWireCodec.encodeInput(
                commandId,
                11,
                30_000,
                identity,
                ByteArray(ControllerWriterWireCodec.MAX_INPUT_CHUNK_BYTES + 1),
            )
        }
        assertThrows(IllegalArgumentException::class.java) {
            ControllerWriterWireCodec.encodeInput(
                commandId,
                11,
                30_000,
                identity,
                ByteArray(ControllerWriterWireCodec.MAX_INPUT_CHUNK_BYTES) { 0xff.toByte() },
            )
        }
    }

    @Test
    fun resizeRejectsViewportOutsideHostLimits() {
        assertThrows(IllegalArgumentException::class.java) {
            ControllerWriterWireCodec.encodeResize(
                UUID.randomUUID(),
                1,
                30_000,
                identity,
                TerminalViewport(401, 40),
            )
        }
    }

    private fun heldReducer(): WriterControlReducer = WriterControlReducer(identity).also {
        val command = UUID.randomUUID()
        it.beginAcquire(command)
        it.finishAcquire(command, applied = true)
    }
}
