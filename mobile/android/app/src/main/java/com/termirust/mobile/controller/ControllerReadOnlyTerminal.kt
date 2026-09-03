package com.termirust.mobile.controller

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.util.UUID

data class TerminalLimits(
    val maxColumns: Int = 400,
    val maxRows: Int = 200,
    val maxFrameBytes: Int = 1 * 1_024 * 1_024,
    val maxQueuedFrames: Int = 64,
    val maxQueuedFrameBytes: Int = 4 * 1_024 * 1_024,
    val maxScrollbackRows: Int = 50_000,
    val maxRetainedCells: Int = 1_000_000,
    val maxGraphemeBytes: Int = 16 * 1_024 * 1_024,
    val maxStyleBytes: Int = 8 * 1_024 * 1_024,
    val maxParserCarryBytes: Int = 4 * 1_024 * 1_024,
    val maxModelBytes: Int = 32 * 1_024 * 1_024,
) {
    fun validate() {
        val values = listOf(
            maxColumns, maxRows, maxFrameBytes, maxQueuedFrames, maxQueuedFrameBytes,
            maxScrollbackRows, maxRetainedCells, maxGraphemeBytes, maxStyleBytes,
            maxParserCarryBytes, maxModelBytes,
        )
        require(values.all { it > 0 })
        require(maxFrameBytes <= maxQueuedFrameBytes && maxQueuedFrameBytes <= maxModelBytes)
        require(maxGraphemeBytes <= maxModelBytes && maxStyleBytes <= maxModelBytes)
        require(maxParserCarryBytes <= maxModelBytes)
    }

    fun validate(viewport: TerminalViewport) {
        validate()
        require(viewport.columns in 1..maxColumns && viewport.rows in 1..maxRows)
    }
}

data class TerminalViewport(val columns: Int, val rows: Int)

data class ReadOnlyAttachIdentity(
    val hostId: String,
    val sessionId: UUID,
    val occupantGeneration: Long,
    val hostInstanceId: UUID? = null,
) {
    fun validate() {
        require(hostId.toByteArray().size in 1..256 && occupantGeneration > 0)
    }
}

data class TerminalStreamCursor(
    val identity: ReadOnlyAttachIdentity,
    var outputSequence: Long = 0,
)

data class TerminalOutputFrame(val sessionId: UUID, val sequence: Long, val bytes: ByteArray) {
    override fun equals(other: Any?): Boolean = other is TerminalOutputFrame &&
        sessionId == other.sessionId && sequence == other.sequence && bytes.contentEquals(other.bytes)

    override fun hashCode(): Int = 31 * (31 * sessionId.hashCode() + sequence.hashCode()) + bytes.contentHashCode()
}

data class TerminalSnapshotChunk(
    val sessionId: UUID,
    val boundarySequence: Long,
    val viewport: TerminalViewport,
    val chunkIndex: Int,
    val chunkCount: Int,
    val bytes: ByteArray,
) {
    override fun equals(other: Any?): Boolean = other is TerminalSnapshotChunk &&
        sessionId == other.sessionId && boundarySequence == other.boundarySequence &&
        viewport == other.viewport && chunkIndex == other.chunkIndex &&
        chunkCount == other.chunkCount && bytes.contentEquals(other.bytes)

    override fun hashCode(): Int = sessionId.hashCode()
}

sealed interface ReadOnlyWireEvent {
    data class Snapshot(val chunk: TerminalSnapshotChunk) : ReadOnlyWireEvent
    data class Attached(val replayThroughSequence: Long, val hasWriterLease: Boolean) : ReadOnlyWireEvent
    data class Output(val frame: TerminalOutputFrame) : ReadOnlyWireEvent
    data class Completed(val commandId: UUID, val applied: Boolean) : ReadOnlyWireEvent
    data class HostError(val commandId: UUID, val code: String, val completionUnknown: Boolean) : ReadOnlyWireEvent
}

sealed interface ReadOnlyAttachState {
    data object Detached : ReadOnlyAttachState
    data object Authenticating : ReadOnlyAttachState
    data object Snapshot : ReadOnlyAttachState
    data object Replaying : ReadOnlyAttachState
    data object Live : ReadOnlyAttachState
    data class Gap(val expected: Long, val received: Long) : ReadOnlyAttachState
    data object Exited : ReadOnlyAttachState
    data object Offline : ReadOnlyAttachState
    data class Failed(val reason: String) : ReadOnlyAttachState
}

enum class OutputDisposition { DELIVER, DUPLICATE, GAP }

object ControllerReadOnlyWireCodec {
    private const val MAX_OPENED_PAYLOAD_BYTES = 4 * 1_024 * 1_024
    private val json = Json { ignoreUnknownKeys = false; encodeDefaults = true; explicitNulls = true }

    fun encodeAttach(
        commandId: UUID,
        sessionGeneration: Long,
        deadlineMillis: Long,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewport,
        limits: TerminalLimits = TerminalLimits(),
    ): ByteArray {
        cursor.identity.validate()
        limits.validate(viewport)
        // Early desktop authorities used zero as their exact initial generation.
        require(sessionGeneration >= 0 && deadlineMillis > 0)
        return json.encodeToString(
            AttachEnvelope(
                commandId = commandId.toString(),
                sessionGeneration = sessionGeneration,
                deadlineMillis = deadlineMillis,
                command = AttachCommand(
                    sessionId = cursor.identity.sessionId.toString(),
                    occupantGeneration = cursor.identity.occupantGeneration,
                    fromSequence = cursor.outputSequence,
                    columns = viewport.columns,
                    rows = viewport.rows,
                ),
            ),
        ).encodeToByteArray()
    }

    fun decode(
        payload: ByteArray,
        commandId: UUID,
        identity: ReadOnlyAttachIdentity,
        limits: TerminalLimits = TerminalLimits(),
    ): ReadOnlyWireEvent {
        identity.validate()
        limits.validate()
        require(payload.size in 1..MAX_OPENED_PAYLOAD_BYTES)
        val text = payload.decodeToString()
        val kind = json.parseToJsonElement(text).jsonObject["kind"]?.jsonPrimitive?.content
            ?: throw IllegalArgumentException("missing event kind")
        return when (kind) {
            "snapshot" -> {
                val value = json.decodeFromString<SnapshotPayload>(text)
                require(value.commandId == commandId.toString() && value.sessionId == identity.sessionId.toString())
                require(value.boundarySequence >= 0 && value.chunkCount > 0 && value.chunkIndex in 0 until value.chunkCount)
                require(value.bytes.size <= limits.maxFrameBytes && value.bytes.all { it in 0..255 })
                require(value.bytes.isNotEmpty() || value.chunkCount == 1)
                val viewport = TerminalViewport(value.columns, value.rows).also(limits::validate)
                ReadOnlyWireEvent.Snapshot(
                    TerminalSnapshotChunk(
                        identity.sessionId,
                        value.boundarySequence,
                        viewport,
                        value.chunkIndex,
                        value.chunkCount,
                        value.bytes.map(Int::toByte).toByteArray(),
                    ),
                )
            }
            "attached" -> {
                val value = json.decodeFromString<AttachedPayload>(text)
                require(value.commandId == commandId.toString() && value.sessionId == identity.sessionId.toString())
                require(value.occupantGeneration == identity.occupantGeneration && value.replayThroughSequence >= 0)
                ReadOnlyWireEvent.Attached(value.replayThroughSequence, value.hasWriterLease)
            }
            "output" -> {
                val value = json.decodeFromString<OutputPayload>(text)
                require(value.sessionId == identity.sessionId.toString() && value.sequence > 0)
                require(value.bytes.size in 1..limits.maxFrameBytes && value.bytes.all { it in 0..255 })
                ReadOnlyWireEvent.Output(
                    TerminalOutputFrame(identity.sessionId, value.sequence, value.bytes.map(Int::toByte).toByteArray()),
                )
            }
            "error" -> {
                val value = json.decodeFromString<AttachErrorPayload>(text)
                require(value.code.toByteArray().size in 1..64)
                ReadOnlyWireEvent.HostError(UUID.fromString(value.commandId), value.code, value.completionUnknown)
            }
            "completed" -> {
                val value = json.decodeFromString<CompletedPayload>(text)
                ReadOnlyWireEvent.Completed(UUID.fromString(value.commandId), value.applied)
            }
            else -> throw IllegalArgumentException("unsupported event kind")
        }
    }
}

class BoundedTerminalFrameQueue(private val limits: TerminalLimits = TerminalLimits()) {
    private val frames = ArrayDeque<TerminalOutputFrame>()
    var queuedBytes: Int = 0
        private set
    val size: Int get() = frames.size

    init { limits.validate() }

    fun enqueue(frame: TerminalOutputFrame) {
        require(frame.bytes.size in 1..limits.maxFrameBytes)
        require(frames.size < limits.maxQueuedFrames)
        require(frame.bytes.size <= limits.maxQueuedFrameBytes - queuedBytes)
        frames.addLast(frame)
        queuedBytes += frame.bytes.size
    }

    fun removeFirstOrNull(): TerminalOutputFrame? = frames.removeFirstOrNull()?.also {
        queuedBytes -= it.bytes.size
    }

    fun clear() {
        frames.clear()
        queuedBytes = 0
    }
}

class ReadOnlyAttachReducer(
    identity: ReadOnlyAttachIdentity,
    fromSequence: Long = 0,
    private val limits: TerminalLimits = TerminalLimits(),
) {
    val cursor = TerminalStreamCursor(identity, fromSequence)
    var state: ReadOnlyAttachState = ReadOnlyAttachState.Detached
        private set
    var replayThroughSequence: Long? = null
        private set
    private val recentFrames = mutableMapOf<Long, ByteArray>()
    private val recentOrder = ArrayDeque<Long>()
    private var recentBytes = 0
    private var snapshotBoundary: Long? = null
    private var snapshotChunkCount: Int? = null
    private var nextSnapshotChunk = 0

    init {
        identity.validate()
        limits.validate()
        require(fromSequence >= 0)
    }

    fun beginAuthentication() {
        require(state == ReadOnlyAttachState.Detached || state == ReadOnlyAttachState.Offline)
        state = ReadOnlyAttachState.Authenticating
    }

    fun beginSnapshot() {
        require(state == ReadOnlyAttachState.Authenticating)
        snapshotBoundary = null
        snapshotChunkCount = null
        nextSnapshotChunk = 0
        state = ReadOnlyAttachState.Snapshot
    }

    fun observeSnapshot(chunk: TerminalSnapshotChunk): Boolean {
        require(state == ReadOnlyAttachState.Snapshot && chunk.sessionId == cursor.identity.sessionId)
        require(chunk.chunkCount > 0 && chunk.chunkIndex == nextSnapshotChunk)
        require(chunk.bytes.size <= limits.maxFrameBytes)
        limits.validate(chunk.viewport)
        if (snapshotBoundary == null) {
            require(chunk.boundarySequence >= cursor.outputSequence)
            snapshotBoundary = chunk.boundarySequence
            snapshotChunkCount = chunk.chunkCount
        } else {
            require(snapshotBoundary == chunk.boundarySequence && snapshotChunkCount == chunk.chunkCount)
        }
        nextSnapshotChunk += 1
        if (nextSnapshotChunk == chunk.chunkCount) {
            cursor.outputSequence = chunk.boundarySequence
            recentFrames.clear()
            recentOrder.clear()
            recentBytes = 0
            state = ReadOnlyAttachState.Replaying
            return true
        }
        return false
    }

    fun beginReplayWithoutSnapshot() {
        require(state == ReadOnlyAttachState.Authenticating)
        state = ReadOnlyAttachState.Replaying
    }

    fun bindReplayBarrier(boundary: Long) {
        require(state == ReadOnlyAttachState.Authenticating || state == ReadOnlyAttachState.Replaying)
        require(boundary >= cursor.outputSequence)
        replayThroughSequence = boundary
        state = if (boundary == cursor.outputSequence) ReadOnlyAttachState.Live else ReadOnlyAttachState.Replaying
    }

    fun observe(frame: TerminalOutputFrame): OutputDisposition {
        require(state == ReadOnlyAttachState.Replaying || state == ReadOnlyAttachState.Live)
        require(frame.sessionId == cursor.identity.sessionId && frame.bytes.isNotEmpty())
        require(frame.bytes.size <= limits.maxFrameBytes && frame.sequence > 0)
        if (frame.sequence <= cursor.outputSequence) {
            require(recentFrames[frame.sequence]?.contentEquals(frame.bytes) == true)
            return OutputDisposition.DUPLICATE
        }
        val expected = Math.addExact(cursor.outputSequence, 1)
        if (frame.sequence != expected) {
            state = ReadOnlyAttachState.Gap(expected, frame.sequence)
            return OutputDisposition.GAP
        }
        cursor.outputSequence = frame.sequence
        remember(frame)
        if (state == ReadOnlyAttachState.Replaying && replayThroughSequence == frame.sequence) {
            state = ReadOnlyAttachState.Live
        }
        return OutputDisposition.DELIVER
    }

    fun markOffline() { state = ReadOnlyAttachState.Offline }
    fun markExited() { state = ReadOnlyAttachState.Exited }
    fun fail(reason: String) { state = ReadOnlyAttachState.Failed(reason) }
    fun detach() {
        recentFrames.clear()
        recentOrder.clear()
        recentBytes = 0
        replayThroughSequence = null
        state = ReadOnlyAttachState.Detached
    }

    private fun remember(frame: TerminalOutputFrame) {
        recentFrames[frame.sequence] = frame.bytes.copyOf()
        recentOrder.addLast(frame.sequence)
        recentBytes += frame.bytes.size
        while (recentOrder.size > limits.maxQueuedFrames || recentBytes > limits.maxQueuedFrameBytes) {
            val sequence = recentOrder.removeFirst()
            recentFrames.remove(sequence)?.let { recentBytes -= it.size }
        }
    }
}

@Serializable private data class AttachEnvelope(
    val version: Int = 1,
    @SerialName("command_id") val commandId: String,
    @SerialName("session_generation") val sessionGeneration: Long,
    @SerialName("deadline_millis") val deadlineMillis: Long,
    val command: AttachCommand,
)

@Serializable private data class AttachCommand(
    val kind: String = "attach",
    @SerialName("session_id") val sessionId: String,
    @SerialName("occupant_generation") val occupantGeneration: Long,
    @SerialName("from_sequence") val fromSequence: Long,
    val columns: Int,
    val rows: Int,
)

@Serializable private data class SnapshotPayload(
    val kind: String,
    @SerialName("command_id") val commandId: String,
    @SerialName("session_id") val sessionId: String,
    @SerialName("boundary_sequence") val boundarySequence: Long,
    val columns: Int,
    val rows: Int,
    @SerialName("chunk_index") val chunkIndex: Int,
    @SerialName("chunk_count") val chunkCount: Int,
    val bytes: List<Int>,
)

@Serializable private data class AttachedPayload(
    val kind: String,
    @SerialName("command_id") val commandId: String,
    @SerialName("session_id") val sessionId: String,
    @SerialName("occupant_generation") val occupantGeneration: Long,
    @SerialName("replay_through_sequence") val replayThroughSequence: Long,
    @SerialName("has_writer_lease") val hasWriterLease: Boolean,
)

@Serializable private data class OutputPayload(
    val kind: String,
    @SerialName("session_id") val sessionId: String,
    val sequence: Long,
    val bytes: List<Int>,
)

@Serializable private data class AttachErrorPayload(
    val kind: String,
    @SerialName("command_id") val commandId: String,
    val code: String,
    @SerialName("completion_unknown") val completionUnknown: Boolean,
)

@Serializable private data class CompletedPayload(
    val kind: String,
    @SerialName("command_id") val commandId: String,
    val applied: Boolean,
)
