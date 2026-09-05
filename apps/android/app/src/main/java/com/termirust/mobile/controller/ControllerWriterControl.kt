package com.termirust.mobile.controller

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import java.util.UUID

sealed interface WriterLeaseState {
    data object None : WriterLeaseState
    data class Requesting(val commandId: UUID) : WriterLeaseState
    data object Held : WriterLeaseState
    data object Busy : WriterLeaseState
    data object Lost : WriterLeaseState
}

enum class PendingInputKind { KEYBOARD, PASTE }

data class PendingInput(
    val commandId: UUID,
    val bytes: ByteArray,
    val kind: PendingInputKind,
) {
    override fun equals(other: Any?): Boolean = other is PendingInput &&
        commandId == other.commandId && kind == other.kind && bytes.contentEquals(other.bytes)
    override fun hashCode(): Int = commandId.hashCode()
}

object ControllerWriterWireCodec {
    const val MAX_INPUT_CHUNK_BYTES = 16 * 1_024
    private val json = Json { encodeDefaults = true; explicitNulls = true }

    fun encodeAcquire(commandId: UUID, sessionGeneration: Long, deadlineMillis: Long, identity: ReadOnlyAttachIdentity) =
        encodeLease("acquire_writer", commandId, sessionGeneration, deadlineMillis, identity)

    fun encodeRelease(commandId: UUID, sessionGeneration: Long, deadlineMillis: Long, identity: ReadOnlyAttachIdentity) =
        encodeLease("release_writer", commandId, sessionGeneration, deadlineMillis, identity)

    fun encodeInput(
        commandId: UUID,
        sessionGeneration: Long,
        deadlineMillis: Long,
        identity: ReadOnlyAttachIdentity,
        bytes: ByteArray,
    ): ByteArray {
        validateEnvelope(sessionGeneration, deadlineMillis, identity)
        require(bytes.size in 1..MAX_INPUT_CHUNK_BYTES)
        return encode(
            commandId,
            sessionGeneration,
            deadlineMillis,
            InputCommand(
                sessionId = identity.sessionId.toString(),
                occupantGeneration = identity.occupantGeneration,
                bytes = bytes.map { it.toInt() and 0xff },
            ),
        )
    }

    fun encodeResize(
        commandId: UUID,
        sessionGeneration: Long,
        deadlineMillis: Long,
        identity: ReadOnlyAttachIdentity,
        viewport: TerminalViewport,
        limits: TerminalLimits = TerminalLimits(),
    ): ByteArray {
        validateEnvelope(sessionGeneration, deadlineMillis, identity)
        limits.validate(viewport)
        return encode(
            commandId,
            sessionGeneration,
            deadlineMillis,
            ResizeCommand(
                sessionId = identity.sessionId.toString(),
                occupantGeneration = identity.occupantGeneration,
                columns = viewport.columns,
                rows = viewport.rows,
            ),
        )
    }

    private fun encodeLease(
        kind: String,
        commandId: UUID,
        sessionGeneration: Long,
        deadlineMillis: Long,
        identity: ReadOnlyAttachIdentity,
    ): ByteArray {
        validateEnvelope(sessionGeneration, deadlineMillis, identity)
        return encode(
            commandId,
            sessionGeneration,
            deadlineMillis,
            LeaseCommand(kind, identity.sessionId.toString(), identity.occupantGeneration),
        )
    }

    private inline fun <reified T> encode(
        commandId: UUID,
        sessionGeneration: Long,
        deadlineMillis: Long,
        command: T,
    ): ByteArray where T : WriterCommand {
        val payload = json.encodeToString(
            WriterEnvelope(
                commandId = commandId.toString(),
                sessionGeneration = sessionGeneration,
                deadlineMillis = deadlineMillis,
                command = command,
            ),
        ).encodeToByteArray()
        require(payload.size <= MAX_CONTROL_PAYLOAD_BYTES)
        return payload
    }

    private fun validateEnvelope(sessionGeneration: Long, deadlineMillis: Long, identity: ReadOnlyAttachIdentity) {
        identity.validate()
        // Early desktop authorities used zero as their exact initial generation.
        require(sessionGeneration >= 0 && deadlineMillis > 0)
    }

    private const val MAX_CONTROL_PAYLOAD_BYTES = 64 * 1_024
}

class WriterControlReducer(val identity: ReadOnlyAttachIdentity) {
    var lease: WriterLeaseState = WriterLeaseState.None
        private set
    var isForeground: Boolean = true
        private set
    var queuedBytes: Int = 0
        private set
    private val queue = ArrayDeque<PendingInput>()

    init { identity.validate() }

    fun beginAcquire(commandId: UUID) {
        require(isForeground && (lease == WriterLeaseState.None || lease == WriterLeaseState.Busy || lease == WriterLeaseState.Lost))
        lease = WriterLeaseState.Requesting(commandId)
    }

    fun finishAcquire(commandId: UUID, applied: Boolean) {
        require(lease == WriterLeaseState.Requesting(commandId))
        lease = if (applied) WriterLeaseState.Held else WriterLeaseState.Busy
    }

    fun markLeaseLost() {
        lease = WriterLeaseState.Lost
        clearPendingInput()
    }

    fun releaseLocally() {
        lease = WriterLeaseState.None
        clearPendingInput()
    }

    fun pasteRequiresConfirmation(bytes: ByteArray): Boolean =
        bytes.size > PASTE_CONFIRMATION_BYTES || bytes.any { it == '\n'.code.toByte() || it == '\r'.code.toByte() }

    fun enqueue(
        bytes: ByteArray,
        kind: PendingInputKind,
        confirmed: Boolean = false,
        commandId: UUID = UUID.randomUUID(),
    ) {
        require(isForeground && lease == WriterLeaseState.Held)
        require(bytes.size in 1..ControllerWriterWireCodec.MAX_INPUT_CHUNK_BYTES)
        require(kind != PendingInputKind.PASTE || confirmed || !pasteRequiresConfirmation(bytes))
        require(queue.size < MAX_QUEUED_CHUNKS)
        require(bytes.size <= MAX_QUEUED_BYTES - queuedBytes)
        queue.addLast(PendingInput(commandId, bytes.copyOf(), kind))
        queuedBytes += bytes.size
    }

    fun removeFirstOrNull(): PendingInput? = queue.removeFirstOrNull()?.also { queuedBytes -= it.bytes.size }

    fun setForeground(foreground: Boolean) {
        isForeground = foreground
        if (!foreground) markLeaseLost()
    }

    private fun clearPendingInput() {
        queue.clear()
        queuedBytes = 0
    }

    companion object {
        const val MAX_QUEUED_CHUNKS = 64
        const val MAX_QUEUED_BYTES = 256 * 1_024
        const val PASTE_CONFIRMATION_BYTES = 4 * 1_024
    }
}

private sealed interface WriterCommand

@Serializable private data class WriterEnvelope<T : WriterCommand>(
    val version: Int = 1,
    @SerialName("command_id") val commandId: String,
    @SerialName("session_generation") val sessionGeneration: Long,
    @SerialName("deadline_millis") val deadlineMillis: Long,
    val command: T,
)

@Serializable private data class LeaseCommand(
    val kind: String,
    @SerialName("session_id") val sessionId: String,
    @SerialName("occupant_generation") val occupantGeneration: Long,
) : WriterCommand

@Serializable private data class InputCommand(
    val kind: String = "input",
    @SerialName("session_id") val sessionId: String,
    @SerialName("occupant_generation") val occupantGeneration: Long,
    val bytes: List<Int>,
) : WriterCommand

@Serializable private data class ResizeCommand(
    val kind: String = "resize",
    @SerialName("session_id") val sessionId: String,
    @SerialName("occupant_generation") val occupantGeneration: Long,
    val columns: Int,
    val rows: Int,
) : WriterCommand
