import Foundation

enum WriterLeaseState: Equatable, Sendable {
    case none
    case requesting(commandID: UUID)
    case held
    case busy
    case lost
}

enum PendingInputKind: Equatable, Sendable {
    case keyboard
    case paste
}

struct PendingInput: Equatable, Sendable {
    let commandID: UUID
    let bytes: Data
    let kind: PendingInputKind
}

enum WriterControlFailure: Error, Equatable, Sendable {
    case invalidTransition
    case capabilityDenied
    case inputEmpty
    case inputChunkTooLarge
    case inputQueueFull
    case inputQueueBytesExceeded
    case pasteConfirmationRequired
    case staleCommand
    case resizeOutOfRange
}

private struct WriterCommandEnvelope<Command: Encodable>: Encodable {
    let version = 1
    let commandId: UUID
    let sessionGeneration: UInt64
    let deadlineMillis: UInt64
    let command: Command

    private enum CodingKeys: String, CodingKey {
        case version
        case commandId = "command_id"
        case sessionGeneration = "session_generation"
        case deadlineMillis = "deadline_millis"
        case command
    }
}

private struct WriterLeaseCommand: Encodable {
    let kind: String
    let sessionId: UUID
    let occupantGeneration: UInt64

    private enum CodingKeys: String, CodingKey {
        case kind
        case sessionId = "session_id"
        case occupantGeneration = "occupant_generation"
    }
}

private struct WriterInputCommand: Encodable {
    let kind = "input"
    let sessionId: UUID
    let occupantGeneration: UInt64
    let bytes: [UInt8]

    private enum CodingKeys: String, CodingKey {
        case kind
        case sessionId = "session_id"
        case occupantGeneration = "occupant_generation"
        case bytes
    }
}

private struct WriterResizeCommand: Encodable {
    let kind = "resize"
    let sessionId: UUID
    let occupantGeneration: UInt64
    let columns: UInt32
    let rows: UInt32

    private enum CodingKeys: String, CodingKey {
        case kind
        case sessionId = "session_id"
        case occupantGeneration = "occupant_generation"
        case columns
        case rows
    }
}

enum ControllerWriterWireCodec {
    static let maxInputChunkBytes = 16 * 1_024

    static func encodeAcquireWriter(
        commandID: UUID,
        sessionGeneration: UInt64,
        deadlineMillis: UInt64,
        identity: ReadOnlyAttachIdentity
    ) throws -> Data {
        try encodeLease(
            kind: "acquire_writer",
            commandID: commandID,
            sessionGeneration: sessionGeneration,
            deadlineMillis: deadlineMillis,
            identity: identity
        )
    }

    static func encodeReleaseWriter(
        commandID: UUID,
        sessionGeneration: UInt64,
        deadlineMillis: UInt64,
        identity: ReadOnlyAttachIdentity
    ) throws -> Data {
        try encodeLease(
            kind: "release_writer",
            commandID: commandID,
            sessionGeneration: sessionGeneration,
            deadlineMillis: deadlineMillis,
            identity: identity
        )
    }

    static func encodeInput(
        commandID: UUID,
        sessionGeneration: UInt64,
        deadlineMillis: UInt64,
        identity: ReadOnlyAttachIdentity,
        bytes: Data
    ) throws -> Data {
        try validateEnvelope(
            sessionGeneration: sessionGeneration,
            deadlineMillis: deadlineMillis,
            identity: identity
        )
        guard !bytes.isEmpty else { throw WriterControlFailure.inputEmpty }
        guard bytes.count <= maxInputChunkBytes else {
            throw WriterControlFailure.inputChunkTooLarge
        }
        return try JSONEncoder().encode(WriterCommandEnvelope(
            commandId: commandID,
            sessionGeneration: sessionGeneration,
            deadlineMillis: deadlineMillis,
            command: WriterInputCommand(
                sessionId: identity.sessionID,
                occupantGeneration: identity.occupantGeneration,
                bytes: Array(bytes)
            )
        ))
    }

    static func encodeResize(
        commandID: UUID,
        sessionGeneration: UInt64,
        deadlineMillis: UInt64,
        identity: ReadOnlyAttachIdentity,
        viewport: TerminalViewportState,
        limits: TerminalLimits = .controllerDefault
    ) throws -> Data {
        try validateEnvelope(
            sessionGeneration: sessionGeneration,
            deadlineMillis: deadlineMillis,
            identity: identity
        )
        do {
            try limits.validate(viewport: viewport)
        } catch {
            throw WriterControlFailure.resizeOutOfRange
        }
        return try JSONEncoder().encode(WriterCommandEnvelope(
            commandId: commandID,
            sessionGeneration: sessionGeneration,
            deadlineMillis: deadlineMillis,
            command: WriterResizeCommand(
                sessionId: identity.sessionID,
                occupantGeneration: identity.occupantGeneration,
                columns: UInt32(viewport.columns),
                rows: UInt32(viewport.rows)
            )
        ))
    }

    private static func encodeLease(
        kind: String,
        commandID: UUID,
        sessionGeneration: UInt64,
        deadlineMillis: UInt64,
        identity: ReadOnlyAttachIdentity
    ) throws -> Data {
        try validateEnvelope(
            sessionGeneration: sessionGeneration,
            deadlineMillis: deadlineMillis,
            identity: identity
        )
        return try JSONEncoder().encode(WriterCommandEnvelope(
            commandId: commandID,
            sessionGeneration: sessionGeneration,
            deadlineMillis: deadlineMillis,
            command: WriterLeaseCommand(
                kind: kind,
                sessionId: identity.sessionID,
                occupantGeneration: identity.occupantGeneration
            )
        ))
    }

    private static func validateEnvelope(
        sessionGeneration: UInt64,
        deadlineMillis: UInt64,
        identity: ReadOnlyAttachIdentity
    ) throws {
        try identity.validate()
        // Early desktop authorities used zero as their exact initial generation.
        guard deadlineMillis > 0 else {
            throw WriterControlFailure.invalidTransition
        }
    }
}

struct WriterControlReducer: Sendable {
    static let maxQueuedChunks = 64
    static let maxQueuedBytes = 256 * 1_024
    static let pasteConfirmationBytes = 4 * 1_024

    let identity: ReadOnlyAttachIdentity
    private(set) var lease: WriterLeaseState = .none
    private(set) var isForeground = true
    private var queue: [PendingInput] = []
    private(set) var queuedBytes = 0

    init(identity: ReadOnlyAttachIdentity) throws {
        try identity.validate()
        self.identity = identity
    }

    mutating func beginAcquire(commandID: UUID) throws {
        guard isForeground,
              lease == .none || lease == .busy || lease == .lost else {
            throw WriterControlFailure.invalidTransition
        }
        lease = .requesting(commandID: commandID)
    }

    mutating func finishAcquire(commandID: UUID, applied: Bool) throws {
        guard lease == .requesting(commandID: commandID) else {
            throw WriterControlFailure.staleCommand
        }
        lease = applied ? .held : .busy
    }

    mutating func markLeaseLost() {
        lease = .lost
        clearPendingInput()
    }

    mutating func releaseLocally() {
        lease = .none
        clearPendingInput()
    }

    func pasteRequiresConfirmation(_ bytes: Data) -> Bool {
        bytes.count > Self.pasteConfirmationBytes
            || bytes.contains(0x0A)
            || bytes.contains(0x0D)
    }

    mutating func enqueue(
        _ bytes: Data,
        kind: PendingInputKind,
        confirmed: Bool = false,
        commandID: UUID = UUID()
    ) throws {
        guard isForeground, lease == .held else {
            throw WriterControlFailure.invalidTransition
        }
        guard !bytes.isEmpty else { throw WriterControlFailure.inputEmpty }
        guard bytes.count <= ControllerWriterWireCodec.maxInputChunkBytes else {
            throw WriterControlFailure.inputChunkTooLarge
        }
        if kind == .paste, pasteRequiresConfirmation(bytes), !confirmed {
            throw WriterControlFailure.pasteConfirmationRequired
        }
        guard queue.count < Self.maxQueuedChunks else {
            throw WriterControlFailure.inputQueueFull
        }
        guard bytes.count <= Self.maxQueuedBytes - queuedBytes else {
            throw WriterControlFailure.inputQueueBytesExceeded
        }
        queue.append(PendingInput(commandID: commandID, bytes: bytes, kind: kind))
        queuedBytes += bytes.count
    }

    mutating func dequeue() -> PendingInput? {
        guard !queue.isEmpty else { return nil }
        let value = queue.removeFirst()
        queuedBytes -= value.bytes.count
        return value
    }

    mutating func setForeground(_ foreground: Bool) {
        isForeground = foreground
        if !foreground {
            lease = .lost
            clearPendingInput()
        }
    }

    private mutating func clearPendingInput() {
        queue.removeAll(keepingCapacity: true)
        queuedBytes = 0
    }
}
