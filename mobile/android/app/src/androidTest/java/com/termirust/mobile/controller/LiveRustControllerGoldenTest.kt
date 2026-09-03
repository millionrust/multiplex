package com.termirust.mobile.controller

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.ByteArrayOutputStream
import java.io.DataOutputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class LiveRustControllerGoldenTest {
    @Test
    fun productionControllerCompletesLiveHostLifecycle() = runBlocking {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val config = instrumentation.context.assets.open("controller-live.json").use { input ->
            LiveControllerFixtureConfig.load(input.readBytes())
        }
        assumeTrue("Run scripts/test-mobile-android-controller-host.sh for the live fixture.", config != null)
        requireNotNull(config)

        val control = LiveControllerControlClient(config)
        val blobStore = ControllerSecureBlobStore(
            context,
            alias = "termirust-controller-n03-${config.fixtureId.take(12)}",
        )
        val connection = ControllerConnection(blobStore)
        var deviceSecretId: String? = null
        var terminal: LiveTerminalHarness? = null
        var stage = "begin_pairing"

        try {
            val challenge = withTimeout(STAGE_TIMEOUT_MILLIS) {
                connection.beginPairing(
                    offerText = config.offerText,
                    hostName = "Live Rust Host",
                    deviceName = "TermiRust Android Test",
                    deviceId = UUID.randomUUID(),
                )
            }
            assertEquals(control.waitForValue("sas"), challenge.sas)
            assertEquals("confirmed", control.command("confirm").value)

            stage = "finish_pairing"
            val initialHost = withTimeout(STAGE_TIMEOUT_MILLIS) { connection.finishPairing(true) }
            deviceSecretId = initialHost.deviceStaticKeyId
            assertNotNull(blobStore.load(initialHost.deviceStaticKeyId))
            assertEquals("paired", control.waitForValue("status", "paired"))

            stage = "initial_fleet"
            val initialSnapshot = withTimeout(STAGE_TIMEOUT_MILLIS) {
                connection.fetchSessions(initialHost)
            }
            assertEquals(1, initialSnapshot.sessions.size)
            assertEquals(config.sessionId, initialSnapshot.sessions.single().id)
            assertEquals(READ_ONLY_CAPABILITIES, initialSnapshot.capabilityBits)

            stage = "grant_input"
            assertEquals("granted", control.command("grant_input").value)
            val writableSnapshot = waitForCapabilities(connection, initialHost, ALL_CAPABILITIES)
            val writableHost = initialHost.copy(capabilityBits = writableSnapshot.capabilityBits)
            val session = writableSnapshot.sessions.single()
            val identity = ReadOnlyAttachIdentity(
                hostId = writableHost.id,
                sessionId = UUID.fromString(session.id),
                occupantGeneration = requireNotNull(session.occupantGeneration),
                hostInstanceId = session.hostInstanceId?.let(UUID::fromString),
            )

            stage = "interactive_attach"
            terminal = LiveTerminalHarness(
                connection,
                writableHost,
                identity,
                TerminalStreamCursor(identity),
            ).also { it.start() }
            terminal.awaitText("MOBILE-CONTROLLER-READY")

            stage = "writer_acquire"
            val acquireId = UUID.randomUUID()
            connection.requestWriter(writableHost, identity, acquireId)
            assertTrue(terminal.awaitCompletion(acquireId))

            stage = "input"
            val inputId = UUID.randomUUID()
            connection.sendInput(
                writableHost,
                identity,
                inputId,
                "N03-LIVE-MARKER\n".encodeToByteArray(),
            )
            assertTrue(terminal.awaitCompletion(inputId))
            terminal.awaitText("MOBILE-CONTROLLER-OUT:N03-LIVE-MARKER")
            assertEquals(1, terminal.occurrences("MOBILE-CONTROLLER-OUT:N03-LIVE-MARKER"))

            stage = "paste"
            val pasteId = UUID.randomUUID()
            connection.sendInput(
                writableHost,
                identity,
                pasteId,
                "N03-PASTE-ONE\nN03-PASTE-TWO\n".encodeToByteArray(),
            )
            assertTrue(terminal.awaitCompletion(pasteId))
            terminal.awaitText("MOBILE-CONTROLLER-OUT:N03-PASTE-TWO")
            assertEquals(1, terminal.occurrences("MOBILE-CONTROLLER-OUT:N03-PASTE-ONE"))
            assertEquals(1, terminal.occurrences("MOBILE-CONTROLLER-OUT:N03-PASTE-TWO"))

            stage = "resize"
            val resizeId = UUID.randomUUID()
            connection.sendResize(writableHost, identity, resizeId, TerminalViewport(118, 34))
            assertTrue(terminal.awaitCompletion(resizeId))

            stage = "background_disconnect"
            terminal.stop()
            val resumeCursor = TerminalStreamCursor(identity, terminal.latestSequence)

            stage = "foreground_reconnect"
            terminal = LiveTerminalHarness(
                connection,
                writableHost,
                identity,
                resumeCursor,
            ).also { it.start() }
            terminal.awaitAttached()
            val reacquireId = UUID.randomUUID()
            connection.requestWriter(writableHost, identity, reacquireId)
            assertTrue(terminal.awaitCompletion(reacquireId))

            stage = "revocation"
            assertEquals("revoked", control.command("revoke").value)
            val revokedInputId = UUID.randomUUID()
            val immediateFailure = runCatching {
                connection.sendInput(
                    writableHost,
                    identity,
                    revokedInputId,
                    "REVOKED-MUTATION-MUST-FAIL\n".encodeToByteArray(),
                )
            }.exceptionOrNull()
            assertTrue(immediateFailure != null || terminal.awaitFailure(revokedInputId))
            terminal.stop()
            terminal = null

            stage = "revoked_reauthentication"
            val authenticationFailure = runCatching {
                withTimeout(STAGE_TIMEOUT_MILLIS) { connection.fetchSessions(writableHost) }
            }.exceptionOrNull()
            assertNotNull("A revoked Android Controller authenticated again.", authenticationFailure)
        } catch (error: Throwable) {
            throw AssertionError("Live Rust Controller stage $stage failed", error)
        } finally {
            runCatching { terminal?.stop() }
            runCatching { connection.cancel() }
            connection.close()
            deviceSecretId?.let { runCatching { blobStore.delete(it) } }
            deviceSecretId?.let { assertNull(blobStore.load(it)) }
        }
    }

    private suspend fun waitForCapabilities(
        connection: ControllerConnection,
        host: PairedHostRecord,
        required: Int,
    ): ControllerFleetSnapshot = withTimeout(STAGE_TIMEOUT_MILLIS) {
        while (true) {
            val latest = connection.fetchSessions(host)
            if (latest.capabilityBits and required == required) return@withTimeout latest
            delay(25)
        }
        @Suppress("UNREACHABLE_CODE")
        error("capability refresh loop ended unexpectedly")
    }

    private companion object {
        const val STAGE_TIMEOUT_MILLIS = 20_000L
        const val READ_ONLY_CAPABILITIES = 0b11
        const val ALL_CAPABILITIES = 0b1_1111
    }
}

private class LiveTerminalHarness(
    private val connection: ControllerConnection,
    private val host: PairedHostRecord,
    private val identity: ReadOnlyAttachIdentity,
    private val cursor: TerminalStreamCursor,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val events = Channel<ReadOnlyWireEvent>(64)
    private val outcome = CompletableDeferred<Throwable?>()
    private val transcript = StringBuilder()
    private val completions = mutableMapOf<UUID, Boolean>()
    private val failures = mutableSetOf<UUID>()
    private var attached = false
    private var job: Job? = null
    var latestSequence: Long = cursor.outputSequence
        private set

    fun start() {
        check(job == null)
        job = scope.launch {
            val error = runCatching {
                connection.attachInteractive(
                    host,
                    cursor,
                    TerminalViewport(80, 24),
                ) { events.send(it) }
            }.exceptionOrNull()
            outcome.complete(error)
        }
    }

    suspend fun awaitAttached() = awaitCondition { attached }

    suspend fun awaitText(marker: String) = awaitCondition { transcript.contains(marker) }

    fun occurrences(marker: String): Int = transcript.windowed(marker.length)
        .count { it == marker }

    suspend fun awaitCompletion(commandId: UUID): Boolean {
        awaitCondition { commandId in completions }
        return completions[commandId] == true
    }

    suspend fun awaitFailure(commandId: UUID): Boolean = withTimeout(10_000) {
        while (true) {
            events.tryReceive().getOrNull()?.let(::consume)
            if (commandId in failures || completions[commandId] == false) return@withTimeout true
            if (outcome.isCompleted) return@withTimeout outcome.await() != null
            delay(20)
        }
        @Suppress("UNREACHABLE_CODE")
        false
    }

    suspend fun stop() {
        connection.cancel()
        withTimeout(5_000) { job?.join() }
        scope.cancel()
        events.close()
    }

    private suspend fun awaitCondition(condition: () -> Boolean) {
        withTimeout(15_000) {
            while (!condition()) {
                events.tryReceive().getOrNull()?.let(::consume)
                    ?: if (outcome.isCompleted) {
                        throw outcome.await() ?: IllegalStateException("terminal connection ended")
                    } else {
                        delay(10)
                    }
            }
        }
    }

    private fun consume(event: ReadOnlyWireEvent) {
        when (event) {
            is ReadOnlyWireEvent.Snapshot -> {
                latestSequence = maxOf(latestSequence, event.chunk.boundarySequence)
                append(event.chunk.bytes)
            }
            is ReadOnlyWireEvent.Attached -> {
                attached = true
                latestSequence = maxOf(latestSequence, event.replayThroughSequence)
            }
            is ReadOnlyWireEvent.Output -> {
                latestSequence = maxOf(latestSequence, event.frame.sequence)
                append(event.frame.bytes)
            }
            is ReadOnlyWireEvent.Completed -> completions[event.commandId] = event.applied
            is ReadOnlyWireEvent.HostError -> failures += event.commandId
        }
    }

    private fun append(bytes: ByteArray) {
        val value = bytes.decodeToString()
        if (transcript.length + value.length > MAX_TRANSCRIPT_CHARS) {
            val remove = transcript.length + value.length - MAX_TRANSCRIPT_CHARS
            transcript.delete(0, remove.coerceAtMost(transcript.length))
        }
        transcript.append(value.takeLast(MAX_TRANSCRIPT_CHARS))
    }

    private companion object {
        const val MAX_TRANSCRIPT_CHARS = 64 * 1_024
    }
}

@Serializable
private data class LiveControllerFixtureConfig(
    @SerialName("schema_version") val schemaVersion: Int,
    @SerialName("fixture_id") val fixtureId: String = "",
    @SerialName("offer_text") val offerText: String = "",
    @SerialName("control_address") val controlAddress: String = "",
    @SerialName("control_port") val controlPort: Int = 0,
    @SerialName("control_token") val controlToken: String = "",
    @SerialName("session_id") val sessionId: String = "",
) {
    companion object {
        private val json = Json { ignoreUnknownKeys = true }

        fun load(bytes: ByteArray): LiveControllerFixtureConfig? = runCatching {
            json.decodeFromString<LiveControllerFixtureConfig>(bytes.decodeToString())
        }.getOrNull()?.takeIf {
            it.schemaVersion == 1 && it.offerText.isNotBlank() &&
                it.controlAddress.isNotBlank() && it.controlPort in 1..65_535 &&
                it.controlToken.isNotBlank() && runCatching { UUID.fromString(it.fixtureId) }.isSuccess &&
                runCatching { UUID.fromString(it.sessionId) }.isSuccess
        }
    }
}

private class LiveControllerControlClient(private val config: LiveControllerFixtureConfig) {
    private val json = Json { ignoreUnknownKeys = false }

    suspend fun waitForValue(command: String, expected: String? = null): String {
        repeat(200) {
            val response = send(command)
            if (response.ok && response.value != null && (expected == null || response.value == expected)) {
                return response.value
            }
            delay(25)
        }
        throw IllegalStateException("fixture control command did not reach the expected state")
    }

    suspend fun command(command: String): LiveControllerControlResponse {
        val response = send(command)
        check(response.ok) { "fixture control command was rejected" }
        return response
    }

    private suspend fun send(command: String): LiveControllerControlResponse =
        kotlinx.coroutines.withContext(Dispatchers.IO) {
            Socket().use { socket ->
                socket.connect(InetSocketAddress(config.controlAddress, config.controlPort), 5_000)
                socket.soTimeout = 5_000
                socket.tcpNoDelay = true
                val request = json.encodeToString(
                    LiveControllerControlRequest(config.controlToken, command),
                ).encodeToByteArray() + byteArrayOf('\n'.code.toByte())
                DataOutputStream(socket.getOutputStream()).apply {
                    write(request)
                    flush()
                }
                val output = ByteArrayOutputStream()
                val buffer = ByteArray(512)
                while (output.size() <= MAX_RESPONSE_BYTES) {
                    val read = socket.getInputStream().read(buffer)
                    if (read < 0) break
                    output.write(buffer, 0, read)
                }
                check(output.size() in 1..MAX_RESPONSE_BYTES)
                json.decodeFromString<LiveControllerControlResponse>(output.toString(Charsets.UTF_8.name()))
            }
        }

    private companion object {
        const val MAX_RESPONSE_BYTES = 4 * 1_024
    }
}

@Serializable
private data class LiveControllerControlRequest(val token: String, val command: String)

@Serializable
private data class LiveControllerControlResponse(val ok: Boolean, val value: String? = null)
