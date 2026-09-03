package com.termirust.mobile.controller

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.UUID

class ControllerReadOnlyTerminalTest {
    private val sessionId: UUID = UUID.fromString("00000000-0000-0000-0000-000000000001")
    private val identity = ReadOnlyAttachIdentity("host-fingerprint", sessionId, 7)

    @Test
    fun attachCodecBindsExactCursorGenerationAndViewport() {
        val commandId = UUID.fromString("10000000-0000-0000-0000-000000000001")
        val encoded = ControllerReadOnlyWireCodec.encodeAttach(
            commandId,
            sessionGeneration = 4,
            deadlineMillis = 30_000,
            cursor = TerminalStreamCursor(identity, 9),
            viewport = TerminalViewport(120, 40),
        )
        val root = Json.parseToJsonElement(encoded.decodeToString()).jsonObject
        val command = root.getValue("command").jsonObject
        assertEquals(commandId.toString(), root.getValue("command_id").jsonPrimitive.content)
        assertEquals("attach", command.getValue("kind").jsonPrimitive.content)
        assertEquals(sessionId.toString(), command.getValue("session_id").jsonPrimitive.content)
        assertEquals("7", command.getValue("occupant_generation").jsonPrimitive.content)
        assertEquals("9", command.getValue("from_sequence").jsonPrimitive.content)
        assertEquals("120", command.getValue("columns").jsonPrimitive.content)
    }

    @Test
    fun attachCodecAcceptsLegacyInitialSessionGeneration() {
        ControllerReadOnlyWireCodec.encodeAttach(
            UUID.randomUUID(),
            sessionGeneration = 0,
            deadlineMillis = 30_000,
            cursor = TerminalStreamCursor(identity, 0),
            viewport = TerminalViewport(120, 40),
        )
    }

    @Test
    fun codecRejectsUnknownAndCrossSessionOutput() {
        val commandId = UUID.randomUUID()
        val unknown = """{"kind":"output","session_id":"$sessionId","sequence":1,"bytes":[65],"extra":true}"""
        assertThrows(IllegalArgumentException::class.java) {
            ControllerReadOnlyWireCodec.decode(unknown.encodeToByteArray(), commandId, identity)
        }
        val foreign = """{"kind":"output","session_id":"00000000-0000-0000-0000-000000000099","sequence":1,"bytes":[65]}"""
        assertThrows(IllegalArgumentException::class.java) {
            ControllerReadOnlyWireCodec.decode(foreign.encodeToByteArray(), commandId, identity)
        }
    }

    @Test
    fun codecDecodesBoundOutputAndSnapshot() {
        val commandId = UUID.randomUUID()
        val output = """{"kind":"output","session_id":"$sessionId","sequence":10,"bytes":[65,66,67]}"""
        val outputEvent = ControllerReadOnlyWireCodec.decode(output.encodeToByteArray(), commandId, identity)
            as ReadOnlyWireEvent.Output
        assertEquals(10, outputEvent.frame.sequence)
        assertArrayEquals("ABC".encodeToByteArray(), outputEvent.frame.bytes)

        val snapshot = """{"kind":"snapshot","command_id":"$commandId","session_id":"$sessionId","boundary_sequence":9,"columns":120,"rows":40,"chunk_index":0,"chunk_count":1,"bytes":[27,91,50,74]}"""
        val snapshotEvent = ControllerReadOnlyWireCodec.decode(snapshot.encodeToByteArray(), commandId, identity)
            as ReadOnlyWireEvent.Snapshot
        assertEquals(9, snapshotEvent.chunk.boundarySequence)
        assertEquals(TerminalViewport(120, 40), snapshotEvent.chunk.viewport)
    }

    @Test
    fun queueRejectsSixtyFifthFrameAndAggregateOverflow() {
        val queue = BoundedTerminalFrameQueue()
        repeat(64) { queue.enqueue(frame((it + 1).toLong(), byteArrayOf(it.toByte()))) }
        assertEquals(64, queue.size)
        assertThrows(IllegalArgumentException::class.java) { queue.enqueue(frame(65, byteArrayOf(65))) }
        assertEquals(1L, queue.removeFirstOrNull()?.sequence)

        val small = BoundedTerminalFrameQueue(TerminalLimits(maxFrameBytes = 3, maxQueuedFrameBytes = 4))
        small.enqueue(frame(1, byteArrayOf(1, 2, 3)))
        assertThrows(IllegalArgumentException::class.java) { small.enqueue(frame(2, byteArrayOf(4, 5))) }
        assertEquals(3, small.queuedBytes)
    }

    @Test
    fun reducerDeliversOnceAndStopsAtGap() {
        val reducer = ReadOnlyAttachReducer(identity)
        reducer.beginAuthentication()
        reducer.beginReplayWithoutSnapshot()
        val first = frame(1, "one".encodeToByteArray())
        assertEquals(OutputDisposition.DELIVER, reducer.observe(first))
        assertEquals(OutputDisposition.DUPLICATE, reducer.observe(first))
        assertThrows(IllegalArgumentException::class.java) {
            reducer.observe(frame(1, "changed".encodeToByteArray()))
        }

        val gap = ReadOnlyAttachReducer(identity, fromSequence = 8)
        gap.beginAuthentication()
        gap.beginReplayWithoutSnapshot()
        assertEquals(OutputDisposition.GAP, gap.observe(frame(10, "ten".encodeToByteArray())))
        assertEquals(ReadOnlyAttachState.Gap(9, 10), gap.state)
        assertEquals(8, gap.cursor.outputSequence)
    }

    @Test
    fun snapshotChunksAreOrderedAndBindReplayBarrier() {
        val reducer = ReadOnlyAttachReducer(identity, fromSequence = 4)
        reducer.beginAuthentication()
        reducer.beginSnapshot()
        assertFalse(reducer.observeSnapshot(snapshot(0, 2, "first")))
        assertTrue(reducer.observeSnapshot(snapshot(1, 2, "second")))
        reducer.bindReplayBarrier(22)
        assertEquals(OutputDisposition.DELIVER, reducer.observe(frame(21, "next".encodeToByteArray())))
        assertEquals(ReadOnlyAttachState.Replaying, reducer.state)
        assertEquals(OutputDisposition.DELIVER, reducer.observe(frame(22, "last".encodeToByteArray())))
        assertEquals(ReadOnlyAttachState.Live, reducer.state)
    }

    @Test
    fun terminalParsesIncrementallyAndKeepsOscInert() {
        val terminal = BoundedControllerTerminal(TerminalViewport(20, 3))
        terminal.process("hello\nwor".encodeToByteArray())
        terminal.process("ld\u001b]52;c;secret\u0007!".encodeToByteArray())
        val screen = terminal.snapshot()
        assertEquals("hello", screen.lines[0])
        assertEquals("     world!", screen.lines[1])
        assertFalse(screen.lines.joinToString().contains("secret"))
    }

    @Test
    fun terminalEvictsCompleteScrollbackAtConfiguredLimit() {
        val limits = TerminalLimits(maxScrollbackRows = 1, maxRetainedCells = 20)
        val terminal = BoundedControllerTerminal(TerminalViewport(5, 2), limits)
        terminal.process("11111\n22222\n33333".encodeToByteArray())
        val screen = terminal.snapshot()
        assertTrue(screen.lines.size <= 3)
        assertTrue(screen.accountedBytes <= limits.maxModelBytes)
        assertEquals(TerminalTruncationReason.RETAINED_ROWS_LIMIT, screen.truncation)
    }

    private fun frame(sequence: Long, bytes: ByteArray) = TerminalOutputFrame(sessionId, sequence, bytes)
    private fun snapshot(index: Int, count: Int, value: String) = TerminalSnapshotChunk(
        sessionId,
        boundarySequence = 20,
        viewport = TerminalViewport(120, 40),
        chunkIndex = index,
        chunkCount = count,
        bytes = value.encodeToByteArray(),
    )
}
