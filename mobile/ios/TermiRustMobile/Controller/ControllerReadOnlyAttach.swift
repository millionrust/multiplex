import Foundation

enum ReadOnlyAttachFailure: Error, Equatable, Sendable {
    case invalidIdentity
    case invalidLimits
    case invalidTransition
    case invalidViewport
    case emptyFrame
    case frameTooLarge
    case queueFull
    case queueBytesExceeded
    case sessionMismatch
    case conflictingDuplicate
    case sequenceOverflow
    case snapshotOrder
}

enum ControllerReadOnlyWireError: Error, Equatable, Sendable {
    case malformedResponse
    case hostError(String)
}

enum ReadOnlyAttachState: Equatable, Sendable {
    case detached
    case authenticating
    case snapshot
    case replaying
    case live
    case gap(expected: UInt64, received: UInt64)
    case exited
    case offline
    case failed(ReadOnlyAttachFailure)
}

struct ReadOnlyAttachIdentity: Equatable, Hashable, Sendable {
    let hostID: String
    let hostInstanceID: UUID?
    let sessionID: UUID
    let occupantGeneration: UInt64

    init(
        hostID: String,
        hostInstanceID: UUID? = nil,
        sessionID: UUID,
        occupantGeneration: UInt64
    ) {
        self.hostID = hostID
        self.hostInstanceID = hostInstanceID
        self.sessionID = sessionID
        self.occupantGeneration = occupantGeneration
    }

    func validate() throws {
        guard !hostID.isEmpty,
              hostID.utf8.count <= 256,
              occupantGeneration > 0 else {
            throw ReadOnlyAttachFailure.invalidIdentity
        }
    }
}

struct TerminalStreamCursor: Equatable, Sendable {
    let identity: ReadOnlyAttachIdentity
    fileprivate(set) var outputSequence: UInt64
}

struct TerminalViewportState: Equatable, Sendable {
    let columns: Int
    let rows: Int
}

struct TerminalLimits: Equatable, Sendable {
    static let controllerDefault = Self()

    let maxColumns: Int
    let maxRows: Int
    let maxFrameBytes: Int
    let maxQueuedFrames: Int
    let maxQueuedFrameBytes: Int
    let maxScrollbackRows: Int
    let maxRetainedCells: Int
    let maxGraphemeBytes: Int
    let maxStyleBytes: Int
    let maxParserCarryBytes: Int
    let maxModelBytes: Int

    init(
        maxColumns: Int = 400,
        maxRows: Int = 200,
        maxFrameBytes: Int = 1 * 1_024 * 1_024,
        maxQueuedFrames: Int = 64,
        maxQueuedFrameBytes: Int = 4 * 1_024 * 1_024,
        maxScrollbackRows: Int = 50_000,
        maxRetainedCells: Int = 1_000_000,
        maxGraphemeBytes: Int = 16 * 1_024 * 1_024,
        maxStyleBytes: Int = 8 * 1_024 * 1_024,
        maxParserCarryBytes: Int = 4 * 1_024 * 1_024,
        maxModelBytes: Int = 32 * 1_024 * 1_024
    ) {
        self.maxColumns = maxColumns
        self.maxRows = maxRows
        self.maxFrameBytes = maxFrameBytes
        self.maxQueuedFrames = maxQueuedFrames
        self.maxQueuedFrameBytes = maxQueuedFrameBytes
        self.maxScrollbackRows = maxScrollbackRows
        self.maxRetainedCells = maxRetainedCells
        self.maxGraphemeBytes = maxGraphemeBytes
        self.maxStyleBytes = maxStyleBytes
        self.maxParserCarryBytes = maxParserCarryBytes
        self.maxModelBytes = maxModelBytes
    }

    func validate() throws {
        let values = [
            maxColumns, maxRows, maxFrameBytes, maxQueuedFrames, maxQueuedFrameBytes,
            maxScrollbackRows, maxRetainedCells, maxGraphemeBytes, maxStyleBytes,
            maxParserCarryBytes, maxModelBytes,
        ]
        guard values.allSatisfy({ $0 > 0 }),
              maxFrameBytes <= maxQueuedFrameBytes,
              maxQueuedFrameBytes <= maxModelBytes,
              maxGraphemeBytes <= maxModelBytes,
              maxStyleBytes <= maxModelBytes,
              maxParserCarryBytes <= maxModelBytes else {
            throw ReadOnlyAttachFailure.invalidLimits
        }
    }

    func validate(viewport: TerminalViewportState) throws {
        try validate()
        guard viewport.columns > 0,
              viewport.rows > 0,
              viewport.columns <= maxColumns,
              viewport.rows <= maxRows else {
            throw ReadOnlyAttachFailure.invalidViewport
        }
    }
}

struct TerminalOutputFrame: Equatable, Sendable {
    let sessionID: UUID
    let sequence: UInt64
    let bytes: Data
}

struct TerminalSnapshotChunk: Equatable, Sendable {
    let sessionID: UUID
    let boundarySequence: UInt64
    let viewport: TerminalViewportState
    let chunkIndex: UInt32
    let chunkCount: UInt32
    let bytes: Data
}

enum ReadOnlyOutputDisposition: Equatable, Sendable {
    case deliver
    case duplicate
    case gap(expected: UInt64, received: UInt64)
}

enum ControllerReadOnlyWireEvent: Equatable, Sendable {
    case snapshot(TerminalSnapshotChunk)
    case attached(replayThroughSequence: UInt64, hasWriterLease: Bool)
    case output(TerminalOutputFrame)
    case completed(commandID: UUID, applied: Bool)
    case error(commandID: UUID, code: String, completionUnknown: Bool)
}

private struct ReadOnlyAttachCommandEnvelope: Encodable {
    let version = 1
    let commandId: UUID
    let sessionGeneration: UInt64
    let deadlineMillis: UInt64
    let command: ReadOnlyAttachCommand

    private enum CodingKeys: String, CodingKey {
        case version
        case commandId = "command_id"
        case sessionGeneration = "session_generation"
        case deadlineMillis = "deadline_millis"
        case command
    }
}

private struct ReadOnlyAttachCommand: Encodable {
    let kind = "attach"
    let sessionId: UUID
    let occupantGeneration: UInt64
    let fromSequence: UInt64
    let columns: UInt32
    let rows: UInt32

    private enum CodingKeys: String, CodingKey {
        case kind
        case sessionId = "session_id"
        case occupantGeneration = "occupant_generation"
        case fromSequence = "from_sequence"
        case columns
        case rows
    }
}

private struct ReadOnlyAttachedPayload: Decodable {
    let kind: String
    let commandId: UUID
    let sessionId: UUID
    let occupantGeneration: UInt64
    let replayThroughSequence: UInt64
    let hasWriterLease: Bool

    private enum CodingKeys: String, CodingKey {
        case kind
        case commandId = "command_id"
        case sessionId = "session_id"
        case occupantGeneration = "occupant_generation"
        case replayThroughSequence = "replay_through_sequence"
        case hasWriterLease = "has_writer_lease"
    }
}

private struct ReadOnlyOutputPayload: Decodable {
    let kind: String
    let sessionId: UUID
    let sequence: UInt64
    let bytes: [UInt8]

    private enum CodingKeys: String, CodingKey {
        case kind
        case sessionId = "session_id"
        case sequence
        case bytes
    }
}

private struct ReadOnlySnapshotPayload: Decodable {
    let kind: String
    let commandId: UUID
    let sessionId: UUID
    let boundarySequence: UInt64
    let columns: UInt32
    let rows: UInt32
    let chunkIndex: UInt32
    let chunkCount: UInt32
    let bytes: [UInt8]

    private enum CodingKeys: String, CodingKey {
        case kind
        case commandId = "command_id"
        case sessionId = "session_id"
        case boundarySequence = "boundary_sequence"
        case columns
        case rows
        case chunkIndex = "chunk_index"
        case chunkCount = "chunk_count"
        case bytes
    }
}

private struct ReadOnlyErrorPayload: Decodable {
    let kind: String
    let commandId: UUID
    let code: String
    let completionUnknown: Bool

    private enum CodingKeys: String, CodingKey {
        case kind
        case commandId = "command_id"
        case code
        case completionUnknown = "completion_unknown"
    }
}

private struct ReadOnlyCompletedPayload: Decodable {
    let kind: String
    let commandId: UUID
    let applied: Bool

    private enum CodingKeys: String, CodingKey {
        case kind
        case commandId = "command_id"
        case applied
    }
}

enum ControllerReadOnlyWireCodec {
    private static let maxOpenedPayloadBytes = 4 * 1_024 * 1_024

    static func encodeAttach(
        commandID: UUID,
        sessionGeneration: UInt64,
        deadlineMillis: UInt64,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewportState,
        limits: TerminalLimits = .controllerDefault
    ) throws -> Data {
        try cursor.identity.validate()
        try limits.validate(viewport: viewport)
        // Early desktop authorities used zero as their exact initial generation.
        guard deadlineMillis > 0 else {
            throw ControllerReadOnlyWireError.malformedResponse
        }
        return try JSONEncoder().encode(ReadOnlyAttachCommandEnvelope(
            commandId: commandID,
            sessionGeneration: sessionGeneration,
            deadlineMillis: deadlineMillis,
            command: ReadOnlyAttachCommand(
                sessionId: cursor.identity.sessionID,
                occupantGeneration: cursor.identity.occupantGeneration,
                fromSequence: cursor.outputSequence,
                columns: UInt32(viewport.columns),
                rows: UInt32(viewport.rows)
            )
        ))
    }

    static func decode(
        _ payload: Data,
        commandID: UUID,
        identity: ReadOnlyAttachIdentity,
        limits: TerminalLimits = .controllerDefault
    ) throws -> ControllerReadOnlyWireEvent {
        try identity.validate()
        try limits.validate()
        guard !payload.isEmpty,
              payload.count <= maxOpenedPayloadBytes,
              let object = try JSONSerialization.jsonObject(with: payload) as? [String: Any],
              let kind = object["kind"] as? String else {
            throw ControllerReadOnlyWireError.malformedResponse
        }

        switch kind {
        case "snapshot":
            guard Set(object.keys) == [
                "kind", "command_id", "session_id", "boundary_sequence", "columns", "rows",
                "chunk_index", "chunk_count", "bytes",
            ] else {
                throw ControllerReadOnlyWireError.malformedResponse
            }
            let snapshot = try JSONDecoder().decode(ReadOnlySnapshotPayload.self, from: payload)
            let viewport = TerminalViewportState(
                columns: Int(snapshot.columns),
                rows: Int(snapshot.rows)
            )
            guard snapshot.kind == "snapshot",
                  snapshot.commandId == commandID,
                  snapshot.sessionId == identity.sessionID,
                  snapshot.chunkCount > 0,
                  snapshot.chunkIndex < snapshot.chunkCount,
                  snapshot.bytes.count <= limits.maxFrameBytes,
                  !snapshot.bytes.isEmpty || snapshot.chunkCount == 1 else {
                throw ControllerReadOnlyWireError.malformedResponse
            }
            try limits.validate(viewport: viewport)
            return .snapshot(TerminalSnapshotChunk(
                sessionID: snapshot.sessionId,
                boundarySequence: snapshot.boundarySequence,
                viewport: viewport,
                chunkIndex: snapshot.chunkIndex,
                chunkCount: snapshot.chunkCount,
                bytes: Data(snapshot.bytes)
            ))
        case "attached":
            guard Set(object.keys) == [
                "kind", "command_id", "session_id", "occupant_generation",
                "replay_through_sequence", "has_writer_lease",
            ] else {
                throw ControllerReadOnlyWireError.malformedResponse
            }
            let attached = try JSONDecoder().decode(ReadOnlyAttachedPayload.self, from: payload)
            guard attached.kind == "attached",
                  attached.commandId == commandID,
                  attached.sessionId == identity.sessionID,
                  attached.occupantGeneration == identity.occupantGeneration else {
                throw ControllerReadOnlyWireError.malformedResponse
            }
            return .attached(
                replayThroughSequence: attached.replayThroughSequence,
                hasWriterLease: attached.hasWriterLease
            )
        case "output":
            guard Set(object.keys) == ["kind", "session_id", "sequence", "bytes"] else {
                throw ControllerReadOnlyWireError.malformedResponse
            }
            let output = try JSONDecoder().decode(ReadOnlyOutputPayload.self, from: payload)
            guard output.kind == "output",
                  output.sessionId == identity.sessionID,
                  output.sequence > 0,
                  !output.bytes.isEmpty,
                  output.bytes.count <= limits.maxFrameBytes else {
                throw ControllerReadOnlyWireError.malformedResponse
            }
            return .output(TerminalOutputFrame(
                sessionID: output.sessionId,
                sequence: output.sequence,
                bytes: Data(output.bytes)
            ))
        case "error":
            guard Set(object.keys) == ["kind", "command_id", "code", "completion_unknown"] else {
                throw ControllerReadOnlyWireError.malformedResponse
            }
            let error = try JSONDecoder().decode(ReadOnlyErrorPayload.self, from: payload)
            guard error.kind == "error",
                  !error.code.isEmpty,
                  error.code.utf8.count <= 64 else {
                throw ControllerReadOnlyWireError.malformedResponse
            }
            return .error(
                commandID: error.commandId,
                code: error.code,
                completionUnknown: error.completionUnknown
            )
        case "completed":
            guard Set(object.keys) == ["kind", "command_id", "applied"] else {
                throw ControllerReadOnlyWireError.malformedResponse
            }
            let completed = try JSONDecoder().decode(ReadOnlyCompletedPayload.self, from: payload)
            guard completed.kind == "completed" else {
                throw ControllerReadOnlyWireError.malformedResponse
            }
            return .completed(commandID: completed.commandId, applied: completed.applied)
        default:
            throw ControllerReadOnlyWireError.malformedResponse
        }
    }
}

struct BoundedTerminalFrameQueue: Sendable {
    private let limits: TerminalLimits
    private var frames: [TerminalOutputFrame] = []
    private(set) var queuedBytes = 0

    init(limits: TerminalLimits = .controllerDefault) throws {
        try limits.validate()
        self.limits = limits
    }

    var count: Int { frames.count }
    var isEmpty: Bool { frames.isEmpty }

    mutating func enqueue(_ frame: TerminalOutputFrame) throws {
        guard !frame.bytes.isEmpty else { throw ReadOnlyAttachFailure.emptyFrame }
        guard frame.bytes.count <= limits.maxFrameBytes else {
            throw ReadOnlyAttachFailure.frameTooLarge
        }
        guard frames.count < limits.maxQueuedFrames else {
            throw ReadOnlyAttachFailure.queueFull
        }
        guard frame.bytes.count <= limits.maxQueuedFrameBytes - queuedBytes else {
            throw ReadOnlyAttachFailure.queueBytesExceeded
        }
        frames.append(frame)
        queuedBytes += frame.bytes.count
    }

    mutating func dequeue() -> TerminalOutputFrame? {
        guard !frames.isEmpty else { return nil }
        let frame = frames.removeFirst()
        queuedBytes -= frame.bytes.count
        return frame
    }

    mutating func removeAll() {
        frames.removeAll(keepingCapacity: true)
        queuedBytes = 0
    }
}

struct ReadOnlyAttachReducer: Sendable {
    private let limits: TerminalLimits
    private var recentFrames: [UInt64: Data] = [:]
    private var recentOrder: [UInt64] = []
    private var recentBytes = 0
    private var snapshotBoundary: UInt64?
    private var snapshotChunkCount: UInt32?
    private var nextSnapshotChunk: UInt32 = 0

    private(set) var state: ReadOnlyAttachState = .detached
    private(set) var cursor: TerminalStreamCursor
    private(set) var replayThroughSequence: UInt64?

    init(
        identity: ReadOnlyAttachIdentity,
        fromSequence: UInt64 = 0,
        limits: TerminalLimits = .controllerDefault
    ) throws {
        try identity.validate()
        try limits.validate()
        self.limits = limits
        self.cursor = TerminalStreamCursor(identity: identity, outputSequence: fromSequence)
    }

    mutating func beginAuthentication() throws {
        guard state == .detached || state == .offline else {
            throw fail(.invalidTransition)
        }
        state = .authenticating
    }

    mutating func beginSnapshot() throws {
        guard state == .authenticating else { throw fail(.invalidTransition) }
        snapshotBoundary = nil
        snapshotChunkCount = nil
        nextSnapshotChunk = 0
        state = .snapshot
    }

    mutating func observeSnapshot(_ chunk: TerminalSnapshotChunk) throws -> Bool {
        guard state == .snapshot,
              chunk.sessionID == cursor.identity.sessionID,
              chunk.chunkCount > 0,
              chunk.chunkIndex == nextSnapshotChunk,
              chunk.chunkIndex < chunk.chunkCount,
              chunk.bytes.count <= limits.maxFrameBytes,
              !chunk.bytes.isEmpty || chunk.chunkCount == 1 else {
            throw fail(.snapshotOrder)
        }
        try limits.validate(viewport: chunk.viewport)
        if let boundary = snapshotBoundary {
            guard boundary == chunk.boundarySequence,
                  snapshotChunkCount == chunk.chunkCount else {
                throw fail(.snapshotOrder)
            }
        } else {
            guard chunk.boundarySequence >= cursor.outputSequence else {
                throw fail(.snapshotOrder)
            }
            snapshotBoundary = chunk.boundarySequence
            snapshotChunkCount = chunk.chunkCount
        }
        nextSnapshotChunk += 1
        let complete = nextSnapshotChunk == chunk.chunkCount
        if complete {
            try finishSnapshot(boundary: chunk.boundarySequence)
        }
        return complete
    }

    mutating func finishSnapshot(boundary: UInt64) throws {
        guard state == .snapshot, boundary >= cursor.outputSequence else {
            throw fail(.invalidTransition)
        }
        cursor.outputSequence = boundary
        recentFrames.removeAll(keepingCapacity: true)
        recentOrder.removeAll(keepingCapacity: true)
        recentBytes = 0
        snapshotBoundary = nil
        snapshotChunkCount = nil
        nextSnapshotChunk = 0
        state = .replaying
    }

    mutating func beginReplayWithoutSnapshot() throws {
        guard state == .authenticating else { throw fail(.invalidTransition) }
        replayThroughSequence = nil
        state = .replaying
    }

    mutating func beginReplay(through boundary: UInt64) throws {
        guard state == .authenticating, boundary >= cursor.outputSequence else {
            throw fail(.invalidTransition)
        }
        replayThroughSequence = boundary
        state = boundary == cursor.outputSequence ? .live : .replaying
    }

    mutating func bindReplayBarrier(through boundary: UInt64) throws {
        guard (state == .authenticating || state == .replaying),
              boundary >= cursor.outputSequence else {
            throw fail(.invalidTransition)
        }
        replayThroughSequence = boundary
        state = boundary == cursor.outputSequence ? .live : .replaying
    }

    mutating func markLive() throws {
        guard state == .replaying || state == .live else {
            throw fail(.invalidTransition)
        }
        state = .live
    }

    mutating func observe(_ frame: TerminalOutputFrame) throws -> ReadOnlyOutputDisposition {
        guard state == .replaying || state == .live else {
            throw fail(.invalidTransition)
        }
        guard frame.sessionID == cursor.identity.sessionID else {
            throw fail(.sessionMismatch)
        }
        guard !frame.bytes.isEmpty else { throw fail(.emptyFrame) }
        guard frame.bytes.count <= limits.maxFrameBytes else {
            throw fail(.frameTooLarge)
        }
        guard frame.sequence > 0 else { throw fail(.sequenceOverflow) }

        if frame.sequence <= cursor.outputSequence {
            guard recentFrames[frame.sequence] == frame.bytes else {
                throw fail(.conflictingDuplicate)
            }
            return .duplicate
        }

        guard cursor.outputSequence < UInt64.max else {
            throw fail(.sequenceOverflow)
        }
        let expected = cursor.outputSequence + 1
        guard frame.sequence == expected else {
            state = .gap(expected: expected, received: frame.sequence)
            return .gap(expected: expected, received: frame.sequence)
        }

        cursor.outputSequence = frame.sequence
        remember(frame)
        if state == .replaying, cursor.outputSequence == replayThroughSequence {
            state = .live
        }
        return .deliver
    }

    mutating func markOffline() {
        state = .offline
    }

    mutating func markExited() {
        state = .exited
    }

    mutating func detach() {
        recentFrames.removeAll(keepingCapacity: false)
        recentOrder.removeAll(keepingCapacity: false)
        recentBytes = 0
        replayThroughSequence = nil
        snapshotBoundary = nil
        snapshotChunkCount = nil
        nextSnapshotChunk = 0
        state = .detached
    }

    private mutating func remember(_ frame: TerminalOutputFrame) {
        recentFrames[frame.sequence] = frame.bytes
        recentOrder.append(frame.sequence)
        recentBytes += frame.bytes.count
        while recentOrder.count > limits.maxQueuedFrames
            || recentBytes > limits.maxQueuedFrameBytes {
            let sequence = recentOrder.removeFirst()
            if let removed = recentFrames.removeValue(forKey: sequence) {
                recentBytes -= removed.count
            }
        }
    }

    private mutating func fail(_ failure: ReadOnlyAttachFailure) -> ReadOnlyAttachFailure {
        state = .failed(failure)
        return failure
    }
}
