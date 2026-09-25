import CryptoKit
import Foundation
@preconcurrency import Network

struct HostRoute: Codable, Hashable, Sendable {
    static let maxRoutesPerHost = 8

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

    /// True for the addresses a Host may listen on: RFC 1918 and CGNAT IPv4, and IPv6
    /// unique local addresses. Names, loopback, link-local, and public addresses are rejected.
    var isPrivateNetworkRoute: Bool {
        if let address = IPv4Address(address) {
            let bytes = [UInt8](address.rawValue)
            guard bytes.count == 4 else { return false }
            let privateAddress = bytes[0] == 10
                || (bytes[0] == 172 && (16...31).contains(bytes[1]))
                || (bytes[0] == 192 && bytes[1] == 168)
                || (bytes[0] == 100 && (64...127).contains(bytes[1]))
            return privateAddress && bytes != [127, 0, 0, 1] && bytes != [0, 0, 0, 0]
        }
        if !address.contains("%"), let address = IPv6Address(address) {
            let bytes = [UInt8](address.rawValue)
            return bytes.count == 16 && (bytes[0] & 0xfe) == 0xfc
        }
        return false
    }
}

/// The address that last connected to a Host on one network, by that network's fingerprint.
struct RememberedRouteRecord: Codable, Hashable, Sendable {
    static let maxPerHost = 8

    let fingerprint: String
    let address: String
    let port: UInt16

    init(fingerprint: String, route: HostRoute) throws {
        guard fingerprint.utf8.count == 32,
              fingerprint.utf8.allSatisfy({ (0x30...0x39).contains($0) || (0x61...0x66).contains($0) }) else {
            throw ControllerModelError.invalidRoute
        }
        self.fingerprint = fingerprint
        self.address = route.address
        self.port = route.port
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        try self.init(
            fingerprint: try values.decode(String.self, forKey: .fingerprint),
            route: try HostRoute(
                address: try values.decode(String.self, forKey: .address),
                port: try values.decode(UInt16.self, forKey: .port)
            )
        )
    }

    var remembered: RememberedRoute {
        RememberedRoute(fingerprint: fingerprint, route: RouteAddress(address: address, port: port))
    }

    private enum CodingKeys: String, CodingKey {
        case fingerprint
        case address
        case port
    }
}

struct PairedHostRecord: Codable, Identifiable, Hashable, Sendable {
    static let currentSchemaVersion = 1

    let schemaVersion: Int
    let id: String
    let displayName: String
    /// Every address the Host was reached on, most recently working first. Never empty.
    let routes: [HostRoute]
    let hostStaticPublicKey: Data
    let deviceStaticKeyId: String
    let deviceId: UUID
    let identityGeneration: UInt64
    let revocationEpoch: UInt64
    let sessionGeneration: UInt64
    let capabilityBits: UInt16
    let pairedAt: Date
    /// The Bonjour `id` the Host announces, used to find it again after its address changes.
    let discoveryId: String?
    /// Which address connected last on each network the phone has reached the Host from, newest
    /// first, so the route race tries it first there next time.
    let routeMemory: [RememberedRouteRecord]

    var route: HostRoute { routes[0] }

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
        try self.init(
            id: id,
            displayName: displayName,
            routes: [route],
            hostStaticPublicKey: hostStaticPublicKey,
            deviceStaticKeyId: deviceStaticKeyId,
            deviceId: deviceId,
            identityGeneration: identityGeneration,
            revocationEpoch: revocationEpoch,
            sessionGeneration: sessionGeneration,
            capabilityBits: capabilityBits,
            pairedAt: pairedAt
        )
    }

    init(
        id: String,
        displayName: String,
        routes: [HostRoute],
        hostStaticPublicKey: Data,
        deviceStaticKeyId: String,
        deviceId: UUID,
        identityGeneration: UInt64,
        revocationEpoch: UInt64,
        sessionGeneration: UInt64,
        capabilityBits: UInt16,
        pairedAt: Date = .now,
        discoveryId: String? = nil,
        routeMemory: [RememberedRouteRecord] = []
    ) throws {
        let uniqueRoutes = Self.uniqued(routes)
        guard routeMemory.count <= RememberedRouteRecord.maxPerHost,
              Set(routeMemory.map(\.fingerprint)).count == routeMemory.count,
              hostStaticPublicKey.count == 32,
              identityGeneration > 0,
              !uniqueRoutes.isEmpty,
              uniqueRoutes.count <= HostRoute.maxRoutesPerHost,
              !deviceStaticKeyId.isEmpty,
              deviceStaticKeyId.utf8.count <= 128,
              !displayName.isEmpty,
              displayName.unicodeScalars.count <= 256 else {
            throw ControllerModelError.invalidHost
        }
        self.schemaVersion = Self.currentSchemaVersion
        self.id = id
        self.displayName = displayName
        self.routes = uniqueRoutes
        self.hostStaticPublicKey = hostStaticPublicKey
        self.deviceStaticKeyId = deviceStaticKeyId
        self.deviceId = deviceId
        self.identityGeneration = identityGeneration
        self.revocationEpoch = revocationEpoch
        self.sessionGeneration = sessionGeneration
        self.capabilityBits = capabilityBits
        self.pairedAt = pairedAt
        self.discoveryId = discoveryId ?? Self.discoveryID(hostStaticPublicKey: hostStaticPublicKey)
        self.routeMemory = routeMemory
    }

    private enum CodingKeys: String, CodingKey {
        case schemaVersion
        case id
        case displayName
        case route
        case routes
        case hostStaticPublicKey
        case deviceStaticKeyId
        case deviceId
        case identityGeneration
        case revocationEpoch
        case sessionGeneration
        case capabilityBits
        case pairedAt
        case discoveryId
        case routeMemory
    }

    /// Records saved before a Host could have several addresses carry only `route`.
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        let routes = try values.decodeIfPresent([HostRoute].self, forKey: .routes)
            ?? [try values.decode(HostRoute.self, forKey: .route)]
        let schemaVersion = try values.decode(Int.self, forKey: .schemaVersion)
        let record = try Self(
            id: try values.decode(String.self, forKey: .id),
            displayName: try values.decode(String.self, forKey: .displayName),
            routes: routes,
            hostStaticPublicKey: try values.decode(Data.self, forKey: .hostStaticPublicKey),
            deviceStaticKeyId: try values.decode(String.self, forKey: .deviceStaticKeyId),
            deviceId: try values.decode(UUID.self, forKey: .deviceId),
            identityGeneration: try values.decode(UInt64.self, forKey: .identityGeneration),
            revocationEpoch: try values.decode(UInt64.self, forKey: .revocationEpoch),
            sessionGeneration: try values.decode(UInt64.self, forKey: .sessionGeneration),
            capabilityBits: try values.decode(UInt16.self, forKey: .capabilityBits),
            pairedAt: try values.decode(Date.self, forKey: .pairedAt),
            discoveryId: try values.decodeIfPresent(String.self, forKey: .discoveryId),
            // Records saved before routes were remembered per network carry none. A memory that
            // does not read back is dropped rather than losing the pairing with it.
            routeMemory: Self.readableMemory(
                (try? values.decodeIfPresent([RememberedRouteRecord].self, forKey: .routeMemory)) ?? []
            )
        )
        guard schemaVersion == Self.currentSchemaVersion else {
            throw ControllerModelError.invalidHost
        }
        self = record
    }

    /// Writes `route` alongside `routes` so an older build can still read the record.
    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(schemaVersion, forKey: .schemaVersion)
        try values.encode(id, forKey: .id)
        try values.encode(displayName, forKey: .displayName)
        try values.encode(route, forKey: .route)
        try values.encode(routes, forKey: .routes)
        try values.encode(hostStaticPublicKey, forKey: .hostStaticPublicKey)
        try values.encode(deviceStaticKeyId, forKey: .deviceStaticKeyId)
        try values.encode(deviceId, forKey: .deviceId)
        try values.encode(identityGeneration, forKey: .identityGeneration)
        try values.encode(revocationEpoch, forKey: .revocationEpoch)
        try values.encode(sessionGeneration, forKey: .sessionGeneration)
        try values.encode(capabilityBits, forKey: .capabilityBits)
        try values.encode(pairedAt, forKey: .pairedAt)
        try values.encodeIfPresent(discoveryId, forKey: .discoveryId)
        try values.encode(routeMemory, forKey: .routeMemory)
    }

    var fingerprint: String {
        hostStaticPublicKey.map { String(format: "%02x", $0) }.joined()
    }

    /// The same record with `route` first, keeping at most `HostRoute.maxRoutesPerHost` routes.
    func preferring(_ route: HostRoute) throws -> PairedHostRecord {
        try replacing(routes: [route] + routes.filter { $0 != route })
    }

    /// The same record remembering that `route` connected on the network `fingerprint` names.
    func remembering(_ route: HostRoute, on fingerprint: String) throws -> PairedHostRecord {
        let memory = rememberRoute(
            remembered: routeMemory.map(\.remembered),
            fingerprint: fingerprint,
            route: RouteAddress(route)
        )
        return try replacing(routeMemory: try memory.map { entry in
            try RememberedRouteRecord(
                fingerprint: entry.fingerprint,
                route: try HostRoute(address: entry.route.address, port: entry.route.port)
            )
        })
    }

    func replacing(
        routes: [HostRoute]? = nil,
        capabilityBits: UInt16? = nil,
        routeMemory: [RememberedRouteRecord]? = nil
    ) throws -> PairedHostRecord {
        try PairedHostRecord(
            id: id,
            displayName: displayName,
            routes: Array((routes ?? self.routes).prefix(HostRoute.maxRoutesPerHost)),
            hostStaticPublicKey: hostStaticPublicKey,
            deviceStaticKeyId: deviceStaticKeyId,
            deviceId: deviceId,
            identityGeneration: identityGeneration,
            revocationEpoch: revocationEpoch,
            sessionGeneration: sessionGeneration,
            capabilityBits: capabilityBits ?? self.capabilityBits,
            pairedAt: pairedAt,
            discoveryId: discoveryId,
            routeMemory: routeMemory ?? self.routeMemory
        )
    }

    /// The first 16 bytes of the Host fingerprint in lowercase hex, matching the Bonjour TXT `id`.
    static func discoveryID(hostStaticPublicKey: Data) -> String {
        var hasher = SHA256()
        hasher.update(data: Data("multiplex-host-fingerprint-v1\0".utf8))
        hasher.update(data: hostStaticPublicKey)
        return hasher.finalize().prefix(16).map { String(format: "%02x", $0) }.joined()
    }

    private static func readableMemory(_ memory: [RememberedRouteRecord]) -> [RememberedRouteRecord] {
        var seen = Set<String>()
        return Array(memory.filter { seen.insert($0.fingerprint).inserted }
            .prefix(RememberedRouteRecord.maxPerHost))
    }

    private static func uniqued(_ routes: [HostRoute]) -> [HostRoute] {
        var seen = Set<HostRoute>()
        return routes.filter { seen.insert($0).inserted }
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
    /// The direct address that answered, when the transport connects by address.
    var connectedRoute: HostRoute? = nil
    /// The network the phone reached it from, for remembering `connectedRoute` there.
    var routeFingerprint: String? = nil
    /// What the route race suggests telling the person, if anything.
    var routeAdvice: ControllerRouteAdvice? = nil
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
    case codeRejected
    case invalidAddress
    /// A computer answering a command by name rather than failing. A Session Host has no window
    /// to open a terminal in; the app may be busy, may refuse, or may not answer at all.
    case createSessionUnavailable
    case createSessionDenied
    case createSessionBusy
    case createSessionRefused
    case completionUnknown
    case unsupportedCommand
    case hostRefused
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
