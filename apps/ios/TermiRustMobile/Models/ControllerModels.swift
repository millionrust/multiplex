import Foundation

struct HostRoute: Codable, Hashable, Sendable {
    let address: String
    let port: UInt16

    init(address: String, port: UInt16) throws {
        let trimmed = address.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed.utf8.count <= 255, port > 0 else {
            throw ControllerModelError.invalidRoute
        }
        self.address = trimmed
        self.port = port
    }
}

struct PairedHostRecord: Codable, Identifiable, Hashable, Sendable {
    static let currentSchemaVersion = 1

    let schemaVersion: Int
    let id: String
    let displayName: String
    let route: HostRoute
    let hostStaticPublicKey: Data
    let deviceStaticKeyId: String
    let deviceId: UUID
    let identityGeneration: UInt64
    let revocationEpoch: UInt64
    let sessionGeneration: UInt64
    let capabilityBits: UInt16
    let pairedAt: Date

    init(
        id: String,
        displayName: String,
        route: HostRoute,
        hostStaticPublicKey: Data,
        deviceStaticKeyId: String,
        deviceId: UUID,
        identityGeneration: UInt64,
        revocationEpoch: UInt64,
        sessionGeneration: UInt64,
        capabilityBits: UInt16,
        pairedAt: Date = .now
    ) throws {
        guard hostStaticPublicKey.count == 32,
              identityGeneration > 0,
              !deviceStaticKeyId.isEmpty,
              deviceStaticKeyId.utf8.count <= 128,
              !displayName.isEmpty,
              displayName.unicodeScalars.count <= 256 else {
            throw ControllerModelError.invalidHost
        }
        self.schemaVersion = Self.currentSchemaVersion
        self.id = id
        self.displayName = displayName
        self.route = route
        self.hostStaticPublicKey = hostStaticPublicKey
        self.deviceStaticKeyId = deviceStaticKeyId
        self.deviceId = deviceId
        self.identityGeneration = identityGeneration
        self.revocationEpoch = revocationEpoch
        self.sessionGeneration = sessionGeneration
        self.capabilityBits = capabilityBits
        self.pairedAt = pairedAt
    }

    var fingerprint: String {
        hostStaticPublicKey.map { String(format: "%02x", $0) }.joined()
    }
}

struct HostSummary: Codable, Identifiable, Hashable, Sendable {
    let id: String
    let title: String
    let route: HostRoute
    let fingerprint: String
    let capabilityBits: UInt16
}

enum ControllerSessionOrigin: String, Codable, Hashable, Sendable {
    case terminal
    case managedAgent = "managed_agent"
    case observedAgent = "observed_agent"
    case unknown
}

enum ControllerSessionCapability: String, Codable, Hashable, Sendable {
    case observeSessions = "observe_sessions"
    case attachOutput = "attach_output"
    case sendInput = "send_input"
    case resize
    case respondToApproval = "respond_to_approval"
}

struct ControllerSessionSummary: Codable, Identifiable, Hashable, Sendable {
    let id: UUID
    let hostInstanceID: UUID?
    let origin: ControllerSessionOrigin
    let runtime: String?
    let capabilities: [ControllerSessionCapability]
    let title: String
    let project: String?
    let group: String?
    let lifecycle: String
    let activity: String?
    let occupantGeneration: UInt64?
    let lastOutputSequence: UInt64
    let hasWriter: Bool
    let unreadCount: UInt32

    init(
        id: UUID,
        hostInstanceID: UUID? = nil,
        origin: ControllerSessionOrigin = .unknown,
        runtime: String? = nil,
        capabilities: [ControllerSessionCapability] = [],
        title: String,
        project: String?,
        group: String?,
        lifecycle: String,
        activity: String?,
        occupantGeneration: UInt64?,
        lastOutputSequence: UInt64,
        hasWriter: Bool,
        unreadCount: UInt32
    ) {
        self.id = id
        self.hostInstanceID = hostInstanceID
        self.origin = origin
        self.runtime = runtime
        self.capabilities = capabilities
        self.title = title
        self.project = project
        self.group = group
        self.lifecycle = lifecycle
        self.activity = activity
        self.occupantGeneration = occupantGeneration
        self.lastOutputSequence = lastOutputSequence
        self.hasWriter = hasWriter
        self.unreadCount = unreadCount
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case hostInstanceID
        case origin
        case runtime
        case capabilities
        case title
        case project
        case group
        case lifecycle
        case activity
        case occupantGeneration
        case lastOutputSequence
        case hasWriter
        case unreadCount
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            id: try values.decode(UUID.self, forKey: .id),
            hostInstanceID: try values.decodeIfPresent(UUID.self, forKey: .hostInstanceID),
            origin: try values.decodeIfPresent(ControllerSessionOrigin.self, forKey: .origin) ?? .unknown,
            runtime: try values.decodeIfPresent(String.self, forKey: .runtime),
            capabilities: try values.decodeIfPresent([ControllerSessionCapability].self, forKey: .capabilities) ?? [],
            title: try values.decode(String.self, forKey: .title),
            project: try values.decodeIfPresent(String.self, forKey: .project),
            group: try values.decodeIfPresent(String.self, forKey: .group),
            lifecycle: try values.decode(String.self, forKey: .lifecycle),
            activity: try values.decodeIfPresent(String.self, forKey: .activity),
            occupantGeneration: try values.decodeIfPresent(UInt64.self, forKey: .occupantGeneration),
            lastOutputSequence: try values.decode(UInt64.self, forKey: .lastOutputSequence),
            hasWriter: try values.decode(Bool.self, forKey: .hasWriter),
            unreadCount: try values.decode(UInt32.self, forKey: .unreadCount)
        )
    }

    func validate() throws {
        guard !title.isEmpty,
              title.unicodeScalars.count <= ControllerCacheLimits.maxTitleScalars,
              lifecycle.utf8.count <= 64,
              runtime?.utf8.count ?? 0 <= 128,
              capabilities.count <= 5,
              Set(capabilities).count == capabilities.count,
              activity?.utf8.count ?? 0 <= 64,
              project?.unicodeScalars.count ?? 0 <= ControllerCacheLimits.maxTitleScalars,
              group?.unicodeScalars.count ?? 0 <= ControllerCacheLimits.maxTitleScalars else {
            throw ControllerModelError.invalidSession
        }
    }
}

struct SessionSummaryPage: Codable, Equatable, Sendable {
    let revision: UInt64
    let updateSequence: UInt64
    let sessions: [ControllerSessionSummary]
    let nextCursor: String?

    func validate(encodedBy encoder: JSONEncoder = JSONEncoder()) throws {
        guard revision > 0,
              updateSequence > 0,
              sessions.count <= ControllerCacheLimits.maxPageRecords,
              nextCursor?.utf8.count ?? 0 <= 256 else {
            throw ControllerModelError.resourceLimit
        }
        try sessions.forEach { try $0.validate() }
        guard try encoder.encode(self).count <= ControllerCacheLimits.maxPageBytes else {
            throw ControllerModelError.resourceLimit
        }
    }
}

struct ControllerFleetSnapshot: Equatable, Sendable {
    let revision: UInt64
    let updateSequence: UInt64
    let capabilityBits: UInt16
    let sessions: [ControllerSessionSummary]
}

enum ControllerConnectionState: Equatable, Sendable {
    case unpaired
    case pairing
    case sasReady(String)
    case pairedOffline
    case connecting
    case authenticating
    case syncing
    case readyReadOnly
    case revoked
    case incompatible
    case failed(ControllerFailure)
}

enum ControllerFailure: String, Codable, Error, Sendable {
    case cancelled
    case invalidOffer
    case offerExpired
    case sasMismatch
    case networkUnavailable
    case timedOut
    case authenticationFailed
    case keychainUnavailable
    case malformedResponse
    case sequenceGap
    case resourceLimit
    case storageUnavailable
    case pairingUncertain
}

struct ControllerViewState: Equatable, Sendable {
    let hosts: [HostSummary]
    let selectedHostID: String?
    let sessions: [ControllerSessionSummary]
    let connection: ControllerConnectionState
    let cacheUpdatedAt: Date?
    let isCachedReadOnly: Bool

    static let empty = Self(
        hosts: [],
        selectedHostID: nil,
        sessions: [],
        connection: .unpaired,
        cacheUpdatedAt: nil,
        isCachedReadOnly: false
    )
}

enum ControllerModelError: Error, Equatable {
    case invalidRoute
    case invalidHost
    case invalidSession
    case resourceLimit
}

enum ControllerCacheLimits {
    static let maxHosts = 16
    static let maxSessionsPerHost = 5_000
    static let maxSessionsGlobal = 10_000
    static let maxEncodedBytes = 8 * 1_024 * 1_024
    static let maxPageRecords = 1_000
    static let maxPageBytes = 1 * 1_024 * 1_024
    static let maxTitleScalars = 256
}
