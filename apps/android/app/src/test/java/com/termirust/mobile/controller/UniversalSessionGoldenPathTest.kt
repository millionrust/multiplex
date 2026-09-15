package com.termirust.mobile.controller

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test
import java.util.UUID

class UniversalSessionGoldenPathTest {
    @Test
    fun sharedIdentityCapabilitiesAndWriterLifecycle() {
        val fixture = fixture()
        assertEquals(1, fixture.schemaVersion)
        assertEquals(31, fixture.controller.capabilityBits)
        assertEquals(3, fixture.controller.revocationEpoch)
        assertEquals(
            setOf(
                "same_identity_and_capabilities",
                "single_writer",
                "background_releases_writer",
                "acknowledged_input_not_replayed",
                "revocation_stops_mutation",
            ),
            fixture.scenarios.toSet(),
        )

        val session = ControllerSessionSummary(
            id = fixture.session.sessionId,
            hostInstanceId = fixture.session.hostInstanceId,
            origin = fixture.session.origin,
            runtime = fixture.session.runtime,
            capabilities = fixture.session.capabilities,
            title = "Golden Session",
            lifecycle = "live",
            activity = "busy",
            occupantGeneration = fixture.session.occupantGeneration,
            lastOutputSequence = fixture.session.lastOutputSequence,
            hasWriter = false,
            unreadCount = 0,
        )
        session.validate()
        assertEquals(fixture.session.hostInstanceId, session.hostInstanceId)
        assertEquals(fixture.session.capabilities, session.capabilities)

        val identity = ReadOnlyAttachIdentity(
            hostId = "golden-host-fingerprint",
            sessionId = UUID.fromString(session.id),
            occupantGeneration = fixture.session.occupantGeneration,
            hostInstanceId = UUID.fromString(requireNotNull(session.hostInstanceId)),
        )
        val first = WriterControlReducer(identity)
        val second = WriterControlReducer(identity)
        first.beginAcquire(UUID.fromString(fixture.commands.firstWriter))
        first.finishAcquire(UUID.fromString(fixture.commands.firstWriter), applied = true)
        second.beginAcquire(UUID.fromString(fixture.commands.secondWriter))
        second.finishAcquire(UUID.fromString(fixture.commands.secondWriter), applied = false)
        assertEquals(WriterLeaseState.Held, first.lease)
        assertEquals(WriterLeaseState.Busy, second.lease)

        val input = fixture.inputBytes.map(Int::toByte).toByteArray()
        first.enqueue(
            input,
            PendingInputKind.KEYBOARD,
            commandId = UUID.fromString(fixture.commands.input),
        )
        assertEquals(input.toList(), first.removeFirstOrNull()?.bytes?.toList())
        assertNull(first.removeFirstOrNull())

        first.setForeground(false)
        assertEquals(WriterLeaseState.Lost, first.lease)
        assertEquals(0, first.queuedBytes)
        assertThrows(IllegalArgumentException::class.java) {
            first.enqueue(input, PendingInputKind.KEYBOARD)
        }

        val reconnected = WriterControlReducer(identity)
        assertNull(reconnected.removeFirstOrNull())
        reconnected.beginAcquire(UUID.fromString(fixture.commands.reconnectWriter))
        reconnected.finishAcquire(UUID.fromString(fixture.commands.reconnectWriter), applied = true)
        reconnected.markLeaseLost()
        assertThrows(IllegalArgumentException::class.java) {
            reconnected.enqueue(
                input,
                PendingInputKind.KEYBOARD,
                commandId = UUID.fromString(fixture.commands.deniedInput),
            )
        }
        TerminalLimits().validate(TerminalViewport(fixture.viewport.columns, fixture.viewport.rows))
    }

    private fun fixture(): UniversalSessionFixture {
        val bytes = requireNotNull(javaClass.classLoader?.getResourceAsStream("universal-session-v1.json"))
            .use { it.readBytes() }
        return Json.decodeFromString(bytes.decodeToString())
    }
}

@Serializable
private data class UniversalSessionFixture(
    @SerialName("schema_version") val schemaVersion: Int,
    val session: Session,
    val controller: Controller,
    val commands: Commands,
    @SerialName("input_bytes") val inputBytes: List<Int>,
    val viewport: Viewport,
    val scenarios: List<String>,
) {
    @Serializable
    data class Session(
        @SerialName("session_id") val sessionId: String,
        @SerialName("host_instance_id") val hostInstanceId: String,
        @SerialName("occupant_generation") val occupantGeneration: Long,
        @SerialName("session_generation") val sessionGeneration: Long,
        val origin: ControllerSessionOrigin,
        val runtime: String,
        val capabilities: List<ControllerSessionCapability>,
        @SerialName("last_output_sequence") val lastOutputSequence: Long,
    )

    @Serializable
    data class Commands(
        @SerialName("first_writer") val firstWriter: String,
        @SerialName("second_writer") val secondWriter: String,
        val input: String,
        val release: String,
        @SerialName("second_after_release") val secondAfterRelease: String,
        @SerialName("denied_input") val deniedInput: String,
        @SerialName("reconnect_writer") val reconnectWriter: String,
        val resize: String,
        val stop: String,
    )

    @Serializable
    data class Controller(
        @SerialName("device_id") val deviceId: String,
        @SerialName("identity_generation") val identityGeneration: Long,
        @SerialName("revocation_epoch") val revocationEpoch: Long,
        @SerialName("capability_bits") val capabilityBits: Int,
    )

    @Serializable
    data class Viewport(val columns: Int, val rows: Int)
}
