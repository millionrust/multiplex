import Foundation
@preconcurrency import Network
import Security

protocol ControllerDuplexConnection: AnyObject, Sendable {
    /// The address the connection reached, when the transport connects by address.
    var remoteRoute: HostRoute? { get }
    func send(_ data: Data) async throws
    func receive(maximumLength: Int) async throws -> Data
    func cancel()
}

extension ControllerDuplexConnection {
    var remoteRoute: HostRoute? { nil }
}

struct ControllerTransportFactory: Sendable {
    let open: @Sendable (HostRoute) async throws -> any ControllerDuplexConnection
    /// Opens a Bonjour service. Only transports that reach the Host by its address set this,
    /// and only those try each saved route in turn.
    let openEndpoint: (@Sendable (NWEndpoint) async throws -> any ControllerDuplexConnection)?

    init(
        openEndpoint: (@Sendable (NWEndpoint) async throws -> any ControllerDuplexConnection)? = nil,
        open: @escaping @Sendable (HostRoute) async throws -> any ControllerDuplexConnection
    ) {
        self.openEndpoint = openEndpoint
        self.open = open
    }

    static let tcp = Self(
        openEndpoint: { endpoint in
            try await NWControllerDuplexConnection.open(endpoint: endpoint)
        },
        open: { route in
            try await NWControllerDuplexConnection.open(route: route)
        }
    )
}

private final class NWControllerDuplexConnection: ControllerDuplexConnection, @unchecked Sendable {
    private let connection: NWConnection

    private init(_ connection: NWConnection) {
        self.connection = connection
    }

    var remoteRoute: HostRoute? {
        guard case .hostPort(let host, let port) = connection.currentPath?.remoteEndpoint else {
            return nil
        }
        let address: String
        switch host {
        case .ipv4(let value): address = "\(value)"
        case .ipv6(let value): address = "\(value)"
        case .name(let value, _): address = value
        @unknown default: return nil
        }
        let unscoped = address.split(separator: "%", maxSplits: 1).first.map(String.init) ?? ""
        return try? HostRoute(address: unscoped, port: port.rawValue)
    }

    static func open(route: HostRoute) async throws -> NWControllerDuplexConnection {
        let host = NWEndpoint.Host(route.address)
        guard let port = NWEndpoint.Port(rawValue: route.port) else {
            throw ControllerPairingError.invalidOffer
        }
        return try await open(endpoint: .hostPort(host: host, port: port))
    }

    static func open(endpoint: NWEndpoint) async throws -> NWControllerDuplexConnection {
        let connection = NWConnection(to: endpoint, using: .tcp)
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                let gate = ConnectionStartGate()
                connection.stateUpdateHandler = { state in
                    switch state {
                    case .ready:
                        if gate.claim() {
                            continuation.resume(returning: NWControllerDuplexConnection(connection))
                        }
                    case .failed(let error):
                        if gate.claim() { continuation.resume(throwing: error) }
                    case .waiting(let error):
                        // Waiting will not change a refusal or a missing route, and the route
                        // race moves on to the next address as soon as this one fails.
                        if case .posix(let code) = error,
                           [.ECONNREFUSED, .ENETUNREACH, .EHOSTUNREACH].contains(code),
                           gate.claim() {
                            connection.stateUpdateHandler = nil
                            connection.cancel()
                            continuation.resume(throwing: error)
                        }
                    case .cancelled:
                        if gate.claim() { continuation.resume(throwing: CancellationError()) }
                    default:
                        break
                    }
                }
                connection.start(queue: DispatchQueue(label: "com.multiplex.controller.connection"))
            }
        } onCancel: {
            connection.cancel()
        }
    }

    func send(_ data: Data) async throws {
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            connection.send(content: data, completion: .contentProcessed { error in
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume()
                }
            })
        }
    }

    func receive(maximumLength: Int) async throws -> Data {
        try await withCheckedThrowingContinuation { continuation in
            connection.receive(minimumIncompleteLength: 1, maximumLength: maximumLength) {
                data, _, complete, error in
                if let error {
                    continuation.resume(throwing: error)
                } else if let data, !data.isEmpty {
                    continuation.resume(returning: data)
                } else if complete {
                    continuation.resume(throwing: ControllerPairingError.connectionClosed)
                } else {
                    continuation.resume(throwing: ControllerPairingError.connectionClosed)
                }
            }
        }
    }

    func cancel() {
        connection.stateUpdateHandler = nil
        connection.cancel()
    }
}

/// The terminal a computer opened because a device asked it to.
struct CreatedControllerSession: Equatable, Sendable {
    let sessionID: UUID
    let occupantGeneration: UInt64
}

protocol ControllerConnecting: Sendable {
    func beginPairing(
        offerText: String,
        hostName: String,
        deviceName: String,
        deviceID: UUID
    ) async throws -> ControllerPairingChallenge
    func finishPairing(matches: Bool) async throws -> PairedHostRecord
    func pairWithCode(
        target: ControllerPairingTarget,
        code: String,
        hostName: String,
        deviceName: String,
        deviceID: UUID
    ) async throws -> PairedHostRecord
    func fetchSessions(
        host: PairedHostRecord,
        progress: @escaping @Sendable (ControllerConnectionProgress) async -> Void
    ) async throws -> ControllerFleetSnapshot
    func attachReadOnly(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewportState,
        onEvent: @escaping @Sendable (ControllerReadOnlyWireEvent) async throws -> Void
    ) async throws
    func attachInteractive(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewportState,
        onEvent: @escaping @Sendable (ControllerReadOnlyWireEvent) async throws -> Void
    ) async throws
    func requestWriter(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID
    ) async throws
    func releaseWriter(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID
    ) async throws
    func sendInput(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID,
        bytes: Data
    ) async throws
    func sendResize(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID,
        viewport: TerminalViewportState
    ) async throws
    /// Watches a computer's screen until `onEvent` throws or the task is cancelled. A `nil`
    /// surface means the first display the computer offers.
    func watchScreen(
        host: PairedHostRecord,
        surface: UInt32?,
        preview: Bool,
        onOpened: @escaping @Sendable (ControllerScreenTicket, ScreenViewer) async -> Void,
        onEvent: @escaping @Sendable ([ScreenEvent]) async throws -> Void
    ) async throws
    /// Starts a terminal on the computer and says which session it is, so the phone can attach.
    ///
    /// Every field is the device's suggestion; the computer's own defaults answer for whatever is
    /// left out.
    func createSession(
        host: PairedHostRecord,
        folder: String?,
        shell: String?,
        title: String?,
        viewport: TerminalViewportState
    ) async throws -> CreatedControllerSession
    func forgetDeviceSecret(host: PairedHostRecord) async throws
    func cancel() async
    /// What the last attempt to reach `hostID` suggests telling the person, if anything.
    func lastRouteAdvice(hostID: String) async -> ControllerRouteAdvice?
}

final class AppleControllerRouteConnections: @unchecked Sendable {
    let privateNetwork: (any ControllerConnecting)?
    private let lock = NSLock()
    private var storedSSH: (any ControllerConnecting)?
    private var storedRelay: (any ControllerConnecting)?

    init(
        privateNetwork: (any ControllerConnecting)?,
        ssh: (any ControllerConnecting)? = nil,
        selfHostedRelay: (any ControllerConnecting)? = nil
    ) {
        self.privateNetwork = privateNetwork
        self.storedSSH = ssh
        self.storedRelay = selfHostedRelay
    }

    var ssh: (any ControllerConnecting)? {
        lock.withLock { storedSSH }
    }

    var selfHostedRelay: (any ControllerConnecting)? {
        lock.withLock { storedRelay }
    }

    var availability: AppleControllerRouteAvailability {
        AppleControllerRouteAvailability(
            privateNetwork: privateNetwork != nil,
            ssh: ssh != nil,
            selfHostedRelay: selfHostedRelay != nil
        )
    }

    func connection(
        for route: ControllerRemoteRouteKind
    ) -> (any ControllerConnecting)? {
        switch route {
        case .localIPC: nil
        case .privateNetwork: privateNetwork
        case .ssh: ssh
        case .selfHostedRelay: selfHostedRelay
        }
    }

    @discardableResult
    func replaceSSH(
        _ connection: (any ControllerConnecting)?
    ) -> (any ControllerConnecting)? {
        lock.withLock {
            let previous = storedSSH
            storedSSH = connection
            return previous
        }
    }

    @discardableResult
    func replaceRelay(
        _ connection: (any ControllerConnecting)?
    ) -> (any ControllerConnecting)? {
        lock.withLock {
            let previous = storedRelay
            storedRelay = connection
            return previous
        }
    }
}

extension ControllerConnecting {
    /// Only a transport that chooses between a Host's addresses has advice to give.
    func lastRouteAdvice(hostID: String) async -> ControllerRouteAdvice? {
        _ = hostID
        return nil
    }

    /// A transport that cannot start terminals says so, rather than every caller knowing which can.
    func createSession(
        host: PairedHostRecord,
        folder: String?,
        shell: String?,
        title: String?,
        viewport: TerminalViewportState
    ) async throws -> CreatedControllerSession {
        _ = (host, folder, shell, title, viewport)
        throw ControllerConnectionError.capabilityDenied
    }

    /// A transport that cannot carry screens says so, rather than every one of them having to.
    func watchScreen(
        host: PairedHostRecord,
        surface: UInt32?,
        preview: Bool,
        onOpened: @escaping @Sendable (ControllerScreenTicket, ScreenViewer) async -> Void,
        onEvent: @escaping @Sendable ([ScreenEvent]) async throws -> Void
    ) async throws {
        _ = (host, surface, preview, onOpened, onEvent)
        throw ControllerConnectionError.capabilityDenied
    }

    func pairWithCode(
        target: ControllerPairingTarget,
        code: String,
        hostName: String,
        deviceName: String,
        deviceID: UUID
    ) async throws -> PairedHostRecord {
        _ = (target, code, hostName, deviceName, deviceID)
        throw ControllerConnectionError.capabilityDenied
    }

    func attachReadOnly(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewportState,
        onEvent: @escaping @Sendable (ControllerReadOnlyWireEvent) async throws -> Void
    ) async throws {
        _ = (host, cursor, viewport, onEvent)
        throw ControllerConnectionError.capabilityDenied
    }

    func attachInteractive(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewportState,
        onEvent: @escaping @Sendable (ControllerReadOnlyWireEvent) async throws -> Void
    ) async throws {
        _ = (host, cursor, viewport, onEvent)
        throw ControllerConnectionError.capabilityDenied
    }

    func requestWriter(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID
    ) async throws {
        _ = (host, identity, commandID)
        throw ControllerConnectionError.capabilityDenied
    }

    func releaseWriter(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID
    ) async throws {
        _ = (host, identity, commandID)
        throw ControllerConnectionError.capabilityDenied
    }

    func sendInput(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID,
        bytes: Data
    ) async throws {
        _ = (host, identity, commandID, bytes)
        throw ControllerConnectionError.capabilityDenied
    }

    func sendResize(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID,
        viewport: TerminalViewportState
    ) async throws {
        _ = (host, identity, commandID, viewport)
        throw ControllerConnectionError.capabilityDenied
    }
}

enum ControllerConnectionProgress: Sendable {
    case authenticating
    case syncing
}

struct ControllerPairingChallenge: Equatable, Sendable {
    let hostFingerprint: String
    let route: HostRoute
    let sas: String
    let expiresAt: Date
}

private struct ControllerPairingOfferEnvelope: Decodable, Sendable {
    let schemaVersion: UInt16
    let offerId: UUID
    let identityGeneration: UInt64
    let revocationEpoch: UInt64
    let sessionGeneration: UInt64
    let routes: [PairingRoutePayload]
    let offerBytes: [UInt8]

    private enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case offerId = "offer_id"
        case identityGeneration = "identity_generation"
        case revocationEpoch = "revocation_epoch"
        case sessionGeneration = "session_generation"
        case routes
        case offerBytes = "offer_bytes"
    }
}

private struct PairingRoutePayload: Decodable, Sendable {
    let address: String
    let port: UInt16
}

/// The offer a Host sends over a code pairing connection. It names no routes: the phone is
/// already connected to one.
private struct CodePairingOfferEnvelope: Decodable, Sendable {
    let schemaVersion: UInt16
    let offerId: UUID
    let identityGeneration: UInt64
    let revocationEpoch: UInt64
    let sessionGeneration: UInt64
    let offerBytes: [UInt8]

    private enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case offerId = "offer_id"
        case identityGeneration = "identity_generation"
        case revocationEpoch = "revocation_epoch"
        case sessionGeneration = "session_generation"
        case offerBytes = "offer_bytes"
    }
}

private struct CodePairingHelloPayload: Encodable {
    let schemaVersion = 1
    let deviceNonce: [UInt8]

    private enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case deviceNonce = "device_nonce"
    }
}

private struct PairingConnectPayload: Encodable {
    let schemaVersion = 1
    let offerId: UUID

    private enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case offerId = "offer_id"
    }
}

private struct PairingRegistrationPayload: Encodable {
    let schemaVersion = 1
    let deviceId: UUID
    let displayName: String

    private enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case deviceId = "device_id"
        case displayName = "display_name"
    }
}

private struct PairingHostAckPayload: Decodable {
    let schemaVersion: UInt16
    let deviceId: UUID
    let identityGeneration: UInt64
    let revocationEpoch: UInt64
    let sessionGeneration: UInt64
    let capabilityBits: UInt16

    private enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case deviceId = "device_id"
        case identityGeneration = "identity_generation"
        case revocationEpoch = "revocation_epoch"
        case sessionGeneration = "session_generation"
        case capabilityBits = "capability_bits"
    }
}

private struct ListSessionsCommandEnvelope: Encodable {
    let version = 1
    let commandId: UUID
    let sessionGeneration: UInt64
    let deadlineMillis: UInt64
    let command: ListSessionsCommand

    private enum CodingKeys: String, CodingKey {
        case version
        case commandId = "command_id"
        case sessionGeneration = "session_generation"
        case deadlineMillis = "deadline_millis"
        case command
    }
}

private struct ListSessionsCommand: Encodable {
    let kind = "list_sessions"
    let offset: UInt32
    let limit: UInt16
    let expectedRevision: UInt64?

    private enum CodingKeys: String, CodingKey {
        case kind
        case offset
        case limit
        case expectedRevision = "expected_revision"
    }
}

private struct SessionsResponsePayload: Decodable {
    let kind: String
    let commandId: UUID
    let revision: UInt64
    let updateSequence: UInt64
    let sessions: [SessionSummaryPayload]
    let nextOffset: UInt32?

    private enum CodingKeys: String, CodingKey {
        case kind
        case commandId = "command_id"
        case revision
        case updateSequence = "update_sequence"
        case sessions
        case nextOffset = "next_offset"
    }
}

private struct SessionSummaryPayload: Decodable {
    let sessionId: UUID
    let hostInstanceId: UUID?
    let origin: ControllerSessionOrigin?
    let runtime: String?
    let capabilities: [ControllerSessionCapability]?
    let title: String
    let project: String?
    let group: String?
    let lifecycle: String
    let activity: String?
    let occupantGeneration: UInt64?
    let lastOutputSequence: UInt64
    let hasWriter: Bool
    let unread: Bool?

    private enum CodingKeys: String, CodingKey {
        case sessionId = "session_id"
        case hostInstanceId = "host_instance_id"
        case origin
        case runtime
        case capabilities
        case title
        case project
        case group
        case lifecycle
        case activity
        case occupantGeneration = "occupant_generation"
        case lastOutputSequence = "last_output_sequence"
        case hasWriter = "has_writer"
        case unread
    }
}

private struct ErrorResponsePayload: Decodable {
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

actor ControllerConnectionActor: ControllerConnecting {
    private static let observeCapability: UInt16 = 1
    private static let attachCapability: UInt16 = 1 << 1
    private static let inputCapability: UInt16 = 1 << 2
    private static let resizeCapability: UInt16 = 1 << 3
    private static let approvalCapability: UInt16 = 1 << 4
    /// Remote Screens, from amendment 1 of the Controller-v1 ADR.
    private static let observeScreensCapability: UInt16 = 1 << 5
    private static let controlPointerCapability: UInt16 = 1 << 6
    private static let controlKeyboardCapability: UInt16 = 1 << 7
    /// Starting a terminal, from amendment 2 of the Controller-v1 ADR.
    private static let createSessionCapability: UInt16 = 1 << 8
    private static let supportedCapabilityBits = observeCapability
        | attachCapability | inputCapability | resizeCapability | approvalCapability
        | observeScreensCapability | controlPointerCapability | controlKeyboardCapability
        | createSessionCapability
    /// The tile cache a phone keeps for one screen. The computer models the same budget.
    private static let screenCacheBytes: UInt64 = 32 * 1_024 * 1_024
    private static let maxOfferBytes = 4 * 1_024
    private static let maxHandshakeFrameBytes = 1_024
    private static let maxSecureFrameBytes = 64 * 1_024
    private static let maxTerminalFrameBytes = 1 * 1_024 * 1_024
    private static let handshakeTimeout: Duration = .seconds(30)
    /// The Host gives a code pairing connection this long, from its hello to its acknowledgement.
    private static let codePairingTimeout: Duration = .seconds(60)
    private static let discoveryLookupTimeout: Duration = .seconds(3)
    private static let codePairingShareBytes = 32
    private static let codePairingOfferBytes = 84

    private let securityEngine: ControllerSecurityEngine
    private let transportFactory: ControllerTransportFactory
    private let discovery: (any ControllerComputerLookup)?
    private let networkProvider: any ControllerPhoneNetworkProviding
    private var connection: (any ControllerDuplexConnection)?
    /// Per Host: the network the last race ran on, and what it suggests telling the person.
    private var lastRouteFingerprints: [String: String] = [:]
    private var lastRouteAdvices: [String: ControllerRouteAdvice] = [:]
    private var pairing: PendingPairing?
    private var activeTerminal: ActiveTerminalConnection?

    init(
        blobStore: SecureBlobStore,
        transportFactory: ControllerTransportFactory = .tcp,
        discovery: (any ControllerComputerLookup)? = nil,
        networkProvider: (any ControllerPhoneNetworkProviding)? = nil
    ) throws {
        self.securityEngine = try ControllerSecurityEngine(blobs: blobStore)
        self.transportFactory = transportFactory
        self.discovery = discovery
        // Only a transport that connects by address races routes and needs to watch the network.
        self.networkProvider = networkProvider ?? (transportFactory.openEndpoint == nil
            ? ControllerFixedPhoneNetwork(network: PhoneNetwork(link: .other, addresses: [], fingerprint: nil))
            : ControllerPhoneNetworkMonitor())
    }

    func lastRouteAdvice(hostID: String) -> ControllerRouteAdvice? {
        lastRouteAdvices[hostID]
    }

    func beginPairing(
        offerText: String,
        hostName: String,
        deviceName: String,
        deviceID: UUID
    ) async throws -> ControllerPairingChallenge {
        await cancel()
        try Self.validateNames(hostName: hostName, deviceName: deviceName)

        let envelope = try decodeOffer(offerText)
        let offerBytes = Data(envelope.offerBytes)
        let summary = try securityEngine.decodeOfferSummary(offerBytes: offerBytes)
        let nowSeconds = UInt64(Date().timeIntervalSince1970)
        guard summary.version.major == 2,
              summary.version.minor == 0,
              summary.expiresAtUnixSeconds > nowSeconds,
              summary.hostStaticPublicKey.count == 32,
              envelope.identityGeneration > 0 else {
            throw ControllerPairingError.expiredOrIncompatibleOffer
        }
        let routes = try envelope.routes.map { try HostRoute(address: $0.address, port: $0.port) }
        guard routes.allSatisfy(\.isPrivateNetworkRoute) else {
            throw ControllerPairingError.publicRouteRejected
        }

        let keyID = Self.deviceKeyID(deviceID: deviceID, hostKey: summary.hostStaticPublicKey)
        var createdKey = false
        if try securityEngine.secureBlobStatus(keyId: keyID) == .missing {
            try securityEngine.storeSecureBlob(keyId: keyID, value: try Self.randomBytes(count: 32))
            createdKey = true
        }

        do {
            let (network, route) = try await openFirstRoute(routes)
            connection = network
            try await withTimeout(Self.handshakeTimeout) {
                try await Self.send(Self.pairingPreface, over: network)
                let connect = try JSONEncoder().encode(PairingConnectPayload(offerId: envelope.offerId))
                try await Self.sendFrame(connect, maximum: Self.maxOfferBytes, over: network)
            }

            let started = Self.uptimeMillis()
            let session = try securityEngine.pairingStart(request: PairingStartRequest(
                role: .deviceInitiator,
                offerBytes: offerBytes,
                staticKeyId: keyID,
                ephemeralPrivateKey: try Self.randomBytes(count: 32),
                nowMillis: started,
                nowUnixSeconds: nowSeconds
            ))
            try await withTimeout(Self.handshakeTimeout) {
                try await Self.runPairingHandshake(session, over: network)
            }
            let sas = try session.sas().value
            pairing = PendingPairing(
                routes: [route] + routes.filter { $0 != route },
                hostName: hostName,
                deviceName: deviceName,
                deviceID: deviceID,
                keyID: keyID,
                createdKey: createdKey,
                session: session,
                sas: sas,
                hostKey: summary.hostStaticPublicKey,
                capabilityBits: summary.capabilityBits,
                identityGeneration: envelope.identityGeneration,
                revocationEpoch: envelope.revocationEpoch,
                sessionGeneration: envelope.sessionGeneration
            )
            return ControllerPairingChallenge(
                hostFingerprint: Self.fingerprint(summary.hostStaticPublicKey),
                route: route,
                sas: sas,
                expiresAt: Date(timeIntervalSince1970: TimeInterval(summary.expiresAtUnixSeconds))
            )
        } catch {
            if createdKey { try? securityEngine.deleteSecureBlob(keyId: keyID) }
            await cancel()
            throw error
        }
    }

    func finishPairing(matches: Bool) async throws -> PairedHostRecord {
        guard let pending = pairing, let sas = pending.sas, let connection else {
            throw ControllerPairingError.noPairingInProgress
        }
        guard matches else {
            _ = try? pending.session.confirmOrReject(
                confirmation: .reject,
                comparedSas: sas,
                revocationEpoch: pending.revocationEpoch
            )
            if pending.createdKey {
                try? securityEngine.deleteSecureBlob(keyId: pending.keyID)
            }
            await cancel()
            throw ControllerPairingError.rejected
        }

        return try await registerPairedDevice(pending, over: connection) {
            try pending.session.confirmOrReject(
                confirmation: .confirm,
                comparedSas: sas,
                revocationEpoch: pending.revocationEpoch
            )
        }
    }

    /// Pairs with the six-digit code the Host is showing. The code keys CPace, whose result is
    /// bound into the pairing handshake, so no SAS comparison follows.
    func pairWithCode(
        target: ControllerPairingTarget,
        code: String,
        hostName: String,
        deviceName: String,
        deviceID: UUID
    ) async throws -> PairedHostRecord {
        await cancel()
        try Self.validateNames(hostName: hostName, deviceName: deviceName)
        guard code.utf8.count == 6, code.utf8.allSatisfy({ (0x30...0x39).contains($0) }) else {
            throw ControllerPairingError.invalidCode
        }
        let deadline = ContinuousClock.now + Self.codePairingTimeout
        let (network, route) = try await openPairingTarget(target, deadline: deadline)
        connection = network

        var keyID: String?
        var createdKey = false
        let pending: PendingPairing
        do {
            let nonce = try Self.randomBytes(count: 32)
            let envelope: CodePairingOfferEnvelope = try await withTimeout(Self.remaining(until: deadline)) {
                try await Self.send(Self.codePairingPreface, over: network)
                let hello = try JSONEncoder().encode(CodePairingHelloPayload(deviceNonce: [UInt8](nonce)))
                try await Self.sendFrame(hello, maximum: Self.maxOfferBytes, over: network)
                let offer = try await Self.receiveFrame(maximum: Self.maxOfferBytes, over: network)
                return try Self.decodeCodeOffer(offer)
            }
            let offerBytes = Data(envelope.offerBytes)
            let summary = try securityEngine.decodeOfferSummary(offerBytes: offerBytes)
            guard summary.version.major == 2,
                  summary.version.minor == 0,
                  summary.expiresAtUnixSeconds > UInt64(Date().timeIntervalSince1970),
                  summary.hostStaticPublicKey.count == 32 else {
                throw ControllerPairingError.expiredOrIncompatibleOffer
            }

            let deviceKeyID = Self.deviceKeyID(deviceID: deviceID, hostKey: summary.hostStaticPublicKey)
            keyID = deviceKeyID
            if try securityEngine.secureBlobStatus(keyId: deviceKeyID) == .missing {
                try securityEngine.storeSecureBlob(keyId: deviceKeyID, value: try Self.randomBytes(count: 32))
                createdKey = true
            }

            let exchange = try securityEngine.codePairingStart(request: CodePairingStartRequest(
                code: code,
                offerBytes: offerBytes,
                deviceNonce: nonce,
                scalarEntropy: try Self.randomBytes(count: 64)
            ))
            let ephemeralPrivateKey = try Self.randomBytes(count: 32)
            let session: ControllerPairingSession = try await withTimeout(Self.remaining(until: deadline)) {
                try await Self.sendFrame(
                    try exchange.share(),
                    maximum: Self.codePairingShareBytes,
                    over: network
                )
                let hostShare = try await Self.receiveFrame(
                    maximum: Self.codePairingShareBytes,
                    over: network
                )
                guard hostShare.count == Self.codePairingShareBytes else {
                    throw ControllerPairingError.codeRejected
                }
                let session = try exchange.finish(request: CodePairingFinishRequest(
                    hostShare: hostShare,
                    staticKeyId: deviceKeyID,
                    ephemeralPrivateKey: ephemeralPrivateKey,
                    nowMillis: Self.uptimeMillis(),
                    nowUnixSeconds: UInt64(Date().timeIntervalSince1970)
                ))
                try await Self.runPairingHandshake(session, over: network)
                return session
            }
            pending = PendingPairing(
                routes: [route],
                hostName: hostName,
                deviceName: deviceName,
                deviceID: deviceID,
                keyID: deviceKeyID,
                createdKey: createdKey,
                session: session,
                sas: nil,
                hostKey: summary.hostStaticPublicKey,
                capabilityBits: summary.capabilityBits,
                identityGeneration: envelope.identityGeneration,
                revocationEpoch: envelope.revocationEpoch,
                sessionGeneration: envelope.sessionGeneration
            )
            pairing = pending
        } catch {
            if createdKey, let keyID { try? securityEngine.deleteSecureBlob(keyId: keyID) }
            await cancel()
            throw Self.codePairingFailure(error)
        }

        do {
            return try await registerPairedDevice(pending, over: network) {
                try pending.session.confirmCodePairing(revocationEpoch: pending.revocationEpoch)
            }
        } catch ControllerPairingError.acknowledgementUncertain {
            throw ControllerPairingError.acknowledgementUncertain
        } catch {
            throw Self.codePairingFailure(error)
        }
    }

    /// Confirms a finished pairing handshake, registers this device, and waits for the Host's
    /// acknowledgement. When the acknowledgement is lost after registering, an authenticated
    /// session list decides whether the Host saved the device.
    private func registerPairedDevice(
        _ pending: PendingPairing,
        over connection: any ControllerDuplexConnection,
        confirm: () throws -> PairingPublicResult
    ) async throws -> PairedHostRecord {
        var registrationSent = false
        do {
            let result = try confirm()
            guard result.hostStaticPublicKey == pending.hostKey else {
                throw ControllerPairingError.hostIdentityChanged
            }
            let registration = try JSONEncoder().encode(PairingRegistrationPayload(
                deviceId: pending.deviceID,
                displayName: pending.deviceName
            ))
            let sealed = try pending.session.sealFrame(
                kind: .control,
                capability: .observeSessions,
                revocationEpoch: pending.revocationEpoch,
                payload: registration
            )
            try await withTimeout(Self.handshakeTimeout) {
                try await Self.sendFrame(
                    sealed,
                    maximum: Self.maxSecureFrameBytes,
                    over: connection
                )
            }
            registrationSent = true
            let ack: PairingHostAckPayload = try await withTimeout(Self.handshakeTimeout) {
                let sealedAck = try await Self.receiveFrame(
                    maximum: Self.maxSecureFrameBytes,
                    over: connection
                )
                let opened = try pending.session.openFrame(frame: sealedAck)
                guard opened.kind == .control,
                      opened.capability == .observeSessions,
                      opened.revocationEpoch == pending.revocationEpoch else {
                    throw ControllerPairingError.invalidAcknowledgement
                }
                return try Self.decodeStrict(PairingHostAckPayload.self, from: opened.payload)
            }
            guard ack.schemaVersion == 1,
                  ack.deviceId == pending.deviceID,
                  ack.identityGeneration == pending.identityGeneration,
                  ack.revocationEpoch == pending.revocationEpoch,
                  ack.sessionGeneration == pending.sessionGeneration,
                  ack.capabilityBits & ~pending.capabilityBits == 0 else {
                throw ControllerPairingError.invalidAcknowledgement
            }
            let record = try PairedHostRecord(
                id: Self.fingerprint(pending.hostKey),
                displayName: pending.hostName,
                routes: pending.routes,
                hostStaticPublicKey: pending.hostKey,
                deviceStaticKeyId: pending.keyID,
                deviceId: pending.deviceID,
                identityGeneration: ack.identityGeneration,
                revocationEpoch: ack.revocationEpoch,
                sessionGeneration: ack.sessionGeneration,
                capabilityBits: ack.capabilityBits
            )
            await cancel(deleteCreatedKey: false)
            return record
        } catch {
            await cancel(deleteCreatedKey: false)
            if registrationSent {
                do {
                    let provisionalRecord = try PairedHostRecord(
                        id: Self.fingerprint(pending.hostKey),
                        displayName: pending.hostName,
                        routes: pending.routes,
                        hostStaticPublicKey: pending.hostKey,
                        deviceStaticKeyId: pending.keyID,
                        deviceId: pending.deviceID,
                        identityGeneration: pending.identityGeneration,
                        revocationEpoch: pending.revocationEpoch,
                        sessionGeneration: pending.sessionGeneration,
                        capabilityBits: pending.capabilityBits
                    )
                    _ = try await fetchSessions(host: provisionalRecord) { _ in }
                    return provisionalRecord
                } catch {
                    await cancel(deleteCreatedKey: false)
                    throw ControllerPairingError.acknowledgementUncertain
                }
            }
            throw error
        }
    }

    func fetchSessions(
        host: PairedHostRecord,
        progress: @escaping @Sendable (ControllerConnectionProgress) async -> Void
    ) async throws -> ControllerFleetSnapshot {
        await cancel()
        guard host.schemaVersion == PairedHostRecord.currentSchemaVersion,
              host.capabilityBits & Self.observeCapability == Self.observeCapability else {
            throw ControllerConnectionError.capabilityDenied
        }
        let (network, connectedRoute) = try await openHostConnection(host)
        connection = network
        await progress(.authenticating)
        let started = Self.uptimeMillis()
        let request = ConnectionStartRequest(
            staticKeyId: host.deviceStaticKeyId,
            ephemeralPrivateKey: try Self.randomBytes(count: 32),
            hostStaticPublicKey: host.hostStaticPublicKey,
            identityGeneration: host.identityGeneration,
            revocationEpoch: host.revocationEpoch,
            // Ask for everything this app version understands. The computer answers with what
            // it actually granted, which is how a new grant reaches a device that never had it.
            requestedCapabilityBits: Self.supportedCapabilityBits,
            clientNonce: try Self.randomBytes(count: 32),
            nowMillis: started
        )
        let authentication: AuthenticatedSessionResult = try await withTimeout(Self.handshakeTimeout) {
            try await Self.send(Self.authenticationPreface, over: network)
            let prelude = try self.securityEngine.connectionPrelude(request: request)
            try await Self.send(prelude, over: network)
            let challenge = try await Self.receiveExactly(36, over: network)
            let session = try self.securityEngine.connectionStart(
                request: request,
                challengeBytes: challenge
            )
            let hello = try session.handshakeOutbound(nowMillis: Self.uptimeMillis())
            try await Self.sendFrame(
                hello,
                maximum: Self.maxHandshakeFrameBytes,
                over: network
            )
            let accept = try await Self.receiveFrame(
                maximum: Self.maxHandshakeFrameBytes,
                over: network
            )
            let result = try session.handshakeReceiveAccept(
                message: accept,
                nowMillis: Self.uptimeMillis()
            )
            return AuthenticatedSessionResult(publicResult: result, session: session)
        }
        let publicResult = authentication.publicResult
        let authenticatedSession = authentication.session
        defer {
            try? authenticatedSession.finish()
            network.cancel()
            connection = nil
        }
        guard publicResult.hostStaticPublicKey == host.hostStaticPublicKey,
              publicResult.identityGeneration == host.identityGeneration,
              publicResult.revocationEpoch == host.revocationEpoch,
              publicResult.grantedCapabilityBits & Self.observeCapability == Self.observeCapability,
              publicResult.grantedCapabilityBits & ~Self.supportedCapabilityBits == 0 else {
            throw ControllerConnectionError.authenticationFailed
        }

        await progress(.syncing)

        var snapshot = try await withTimeout(Self.handshakeTimeout) {
            try await Self.fetchStableSnapshot(
                host: host,
                session: authenticatedSession,
                network: network,
                capabilityBits: publicResult.grantedCapabilityBits
            )
        }
        snapshot.connectedRoute = connectedRoute
        snapshot.routeFingerprint = lastRouteFingerprints[host.id]
        snapshot.routeAdvice = lastRouteAdvices[host.id]
        return snapshot
    }

    func attachReadOnly(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewportState,
        onEvent: @escaping @Sendable (ControllerReadOnlyWireEvent) async throws -> Void
    ) async throws {
        await cancel()
        guard host.schemaVersion == PairedHostRecord.currentSchemaVersion,
              host.capabilityBits & Self.attachCapability == Self.attachCapability,
              cursor.identity.hostID == host.id else {
            throw ControllerConnectionError.capabilityDenied
        }
        try cursor.identity.validate()
        try TerminalLimits.controllerDefault.validate(viewport: viewport)

        let (network, _) = try await openHostConnection(host)
        connection = network
        let started = Self.uptimeMillis()
        let request = ConnectionStartRequest(
            staticKeyId: host.deviceStaticKeyId,
            ephemeralPrivateKey: try Self.randomBytes(count: 32),
            hostStaticPublicKey: host.hostStaticPublicKey,
            identityGeneration: host.identityGeneration,
            revocationEpoch: host.revocationEpoch,
            requestedCapabilityBits: Self.attachCapability,
            clientNonce: try Self.randomBytes(count: 32),
            nowMillis: started
        )
        let authentication: AuthenticatedSessionResult = try await withTimeout(Self.handshakeTimeout) {
            try await Self.send(Self.authenticationPreface, over: network)
            let prelude = try self.securityEngine.connectionPrelude(request: request)
            try await Self.send(prelude, over: network)
            let challenge = try await Self.receiveExactly(36, over: network)
            let session = try self.securityEngine.connectionStart(
                request: request,
                challengeBytes: challenge
            )
            let hello = try session.handshakeOutbound(nowMillis: Self.uptimeMillis())
            try await Self.sendFrame(
                hello,
                maximum: Self.maxHandshakeFrameBytes,
                over: network
            )
            let accept = try await Self.receiveFrame(
                maximum: Self.maxHandshakeFrameBytes,
                over: network
            )
            let result = try session.handshakeReceiveAccept(
                message: accept,
                nowMillis: Self.uptimeMillis()
            )
            return AuthenticatedSessionResult(publicResult: result, session: session)
        }
        let publicResult = authentication.publicResult
        let authenticatedSession = authentication.session
        defer {
            try? authenticatedSession.finish()
            network.cancel()
            if connection === network { connection = nil }
        }
        guard publicResult.hostStaticPublicKey == host.hostStaticPublicKey,
              publicResult.identityGeneration == host.identityGeneration,
              publicResult.revocationEpoch == host.revocationEpoch,
              publicResult.grantedCapabilityBits == Self.attachCapability else {
            throw ControllerConnectionError.authenticationFailed
        }

        let commandID = UUID()
        let command = try ControllerReadOnlyWireCodec.encodeAttach(
            commandID: commandID,
            sessionGeneration: host.sessionGeneration,
            deadlineMillis: Self.wallClockMillis().saturatingAdd(30_000),
            cursor: cursor,
            viewport: viewport
        )
        let sealed = try authenticatedSession.sealFrame(
            kind: .control,
            capability: .attachOutput,
            revocationEpoch: host.revocationEpoch,
            payload: command
        )
        try await Self.sendFrame(
            sealed,
            maximum: Self.maxSecureFrameBytes,
            over: network
        )

        var attached = false
        while !Task.isCancelled {
            let sealedResponse = try await Self.receiveFrame(
                maximum: Self.maxTerminalFrameBytes,
                over: network
            )
            let opened = try authenticatedSession.openFrame(frame: sealedResponse)
            guard opened.capability == .attachOutput,
                  opened.revocationEpoch == host.revocationEpoch,
                  opened.kind == .control || opened.kind == .terminal else {
                throw ControllerConnectionError.malformedResponse
            }
            let event = try ControllerReadOnlyWireCodec.decode(
                opened.payload,
                commandID: commandID,
                identity: cursor.identity
            )
            switch event {
            case .snapshot:
                guard !attached, opened.kind == .terminal else {
                    throw ControllerConnectionError.malformedResponse
                }
            case .attached:
                guard !attached, opened.kind == .control else {
                    throw ControllerConnectionError.malformedResponse
                }
                attached = true
            case .output:
                guard attached, opened.kind == .terminal else {
                    throw ControllerConnectionError.malformedResponse
                }
            case .completed:
                throw ControllerConnectionError.malformedResponse
            case .error(let errorCommandID, let code, _):
                guard errorCommandID == commandID else {
                    throw ControllerConnectionError.malformedResponse
                }
                throw ControllerConnectionError.hostError(code)
            }
            try await onEvent(event)
        }
        throw CancellationError()
    }

    func attachInteractive(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewportState,
        onEvent: @escaping @Sendable (ControllerReadOnlyWireEvent) async throws -> Void
    ) async throws {
        await cancel()
        let required = Self.attachCapability | Self.inputCapability
        guard host.schemaVersion == PairedHostRecord.currentSchemaVersion,
              host.capabilityBits & required == required,
              cursor.identity.hostID == host.id else {
            throw ControllerConnectionError.capabilityDenied
        }
        try cursor.identity.validate()
        try TerminalLimits.controllerDefault.validate(viewport: viewport)

        let requested = host.capabilityBits
            & (Self.attachCapability | Self.inputCapability
                | Self.resizeCapability | Self.approvalCapability)
        let (network, _) = try await openHostConnection(host)
        connection = network
        let authentication = try await authenticate(
            host: host,
            requestedCapabilityBits: requested,
            network: network
        )
        let publicResult = authentication.publicResult
        let authenticatedSession = authentication.session
        let token = UUID()
        defer {
            if activeTerminal?.token == token { activeTerminal = nil }
            try? authenticatedSession.finish()
            network.cancel()
            if connection === network { connection = nil }
        }
        guard publicResult.hostStaticPublicKey == host.hostStaticPublicKey,
              publicResult.identityGeneration == host.identityGeneration,
              publicResult.revocationEpoch == host.revocationEpoch,
              publicResult.grantedCapabilityBits == requested else {
            throw ControllerConnectionError.authenticationFailed
        }

        let attachCommandID = UUID()
        activeTerminal = ActiveTerminalConnection(
            token: token,
            hostID: host.id,
            identity: cursor.identity,
            grantedCapabilityBits: requested,
            network: network,
            session: authenticatedSession,
            attachCommandID: attachCommandID,
            pendingCapabilities: [:]
        )
        let command = try ControllerReadOnlyWireCodec.encodeAttach(
            commandID: attachCommandID,
            sessionGeneration: host.sessionGeneration,
            deadlineMillis: Self.wallClockMillis().saturatingAdd(30_000),
            cursor: cursor,
            viewport: viewport
        )
        let sealed = try authenticatedSession.sealFrame(
            kind: .control,
            capability: .attachOutput,
            revocationEpoch: host.revocationEpoch,
            payload: command
        )
        try await Self.sendFrame(sealed, maximum: Self.maxSecureFrameBytes, over: network)

        var attached = false
        while !Task.isCancelled {
            let sealedResponse = try await Self.receiveFrame(
                maximum: Self.maxTerminalFrameBytes,
                over: network
            )
            let opened = try authenticatedSession.openFrame(frame: sealedResponse)
            guard opened.revocationEpoch == host.revocationEpoch,
                  opened.kind == .control || opened.kind == .terminal else {
                throw ControllerConnectionError.malformedResponse
            }
            let event = try ControllerReadOnlyWireCodec.decode(
                opened.payload,
                commandID: attachCommandID,
                identity: cursor.identity
            )
            switch event {
            case .snapshot:
                guard !attached,
                      opened.kind == .terminal,
                      opened.capability == .attachOutput else {
                    throw ControllerConnectionError.malformedResponse
                }
            case .attached:
                guard !attached,
                      opened.kind == .control,
                      opened.capability == .attachOutput else {
                    throw ControllerConnectionError.malformedResponse
                }
                attached = true
            case .output:
                guard attached,
                      opened.kind == .terminal,
                      opened.capability == .attachOutput else {
                    throw ControllerConnectionError.malformedResponse
                }
            case .completed(let commandID, _), .error(let commandID, _, _):
                guard attached,
                      opened.kind == .control,
                      let expected = activeTerminal?.pendingCapabilities.removeValue(
                          forKey: commandID
                      ),
                      opened.capability == expected else {
                    throw ControllerConnectionError.malformedResponse
                }
            }
            try await onEvent(event)
        }
        throw CancellationError()
    }

    func requestWriter(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID
    ) async throws {
        let payload = try ControllerWriterWireCodec.encodeAcquireWriter(
            commandID: commandID,
            sessionGeneration: host.sessionGeneration,
            deadlineMillis: Self.wallClockMillis().saturatingAdd(30_000),
            identity: identity
        )
        try await sendTerminalMutation(
            host: host,
            identity: identity,
            commandID: commandID,
            capability: .sendInput,
            capabilityBit: Self.inputCapability,
            payload: payload
        )
    }

    func releaseWriter(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID
    ) async throws {
        let payload = try ControllerWriterWireCodec.encodeReleaseWriter(
            commandID: commandID,
            sessionGeneration: host.sessionGeneration,
            deadlineMillis: Self.wallClockMillis().saturatingAdd(2_000),
            identity: identity
        )
        try await sendTerminalMutation(
            host: host,
            identity: identity,
            commandID: commandID,
            capability: .sendInput,
            capabilityBit: Self.inputCapability,
            payload: payload
        )
    }

    func sendInput(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID,
        bytes: Data
    ) async throws {
        let payload = try ControllerWriterWireCodec.encodeInput(
            commandID: commandID,
            sessionGeneration: host.sessionGeneration,
            deadlineMillis: Self.wallClockMillis().saturatingAdd(10_000),
            identity: identity,
            bytes: bytes
        )
        try await sendTerminalMutation(
            host: host,
            identity: identity,
            commandID: commandID,
            capability: .sendInput,
            capabilityBit: Self.inputCapability,
            payload: payload
        )
    }

    func sendResize(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID,
        viewport: TerminalViewportState
    ) async throws {
        let payload = try ControllerWriterWireCodec.encodeResize(
            commandID: commandID,
            sessionGeneration: host.sessionGeneration,
            deadlineMillis: Self.wallClockMillis().saturatingAdd(10_000),
            identity: identity,
            viewport: viewport
        )
        try await sendTerminalMutation(
            host: host,
            identity: identity,
            commandID: commandID,
            capability: .resize,
            capabilityBit: Self.resizeCapability,
            payload: payload
        )
    }

    /// Watches a computer's screen until `onEvent` throws or the task is cancelled.
    ///
    /// The session starts with an `open_screen` command, whose one-time ticket the screen
    /// protocol's hello proves. After that every frame is a screen frame claiming the capability
    /// its contents need, which the computer checks again before acting on it.
    ///
    /// A `nil` surface means the first display the computer offers, which is the only thing a
    /// phone can know before the welcome: a Mac names its displays by their own ids, not by 1.
    func watchScreen(
        host: PairedHostRecord,
        surface: UInt32?,
        preview: Bool,
        onOpened: @escaping @Sendable (ControllerScreenTicket, ScreenViewer) async -> Void,
        onEvent: @escaping @Sendable ([ScreenEvent]) async throws -> Void
    ) async throws {
        await cancel()
        guard host.schemaVersion == PairedHostRecord.currentSchemaVersion,
              host.capabilityBits & Self.observeScreensCapability == Self.observeScreensCapability
        else {
            throw ControllerConnectionError.capabilityDenied
        }
        let requested = host.capabilityBits & Self.supportedCapabilityBits
        let (network, _) = try await openHostConnection(host)
        connection = network
        let started = Self.uptimeMillis()
        let request = ConnectionStartRequest(
            staticKeyId: host.deviceStaticKeyId,
            ephemeralPrivateKey: try Self.randomBytes(count: 32),
            hostStaticPublicKey: host.hostStaticPublicKey,
            identityGeneration: host.identityGeneration,
            revocationEpoch: host.revocationEpoch,
            requestedCapabilityBits: requested,
            clientNonce: try Self.randomBytes(count: 32),
            nowMillis: started
        )
        let authentication: AuthenticatedSessionResult = try await withTimeout(Self.handshakeTimeout) {
            try await Self.send(Self.authenticationPreface, over: network)
            let prelude = try self.securityEngine.connectionPrelude(request: request)
            try await Self.send(prelude, over: network)
            let challenge = try await Self.receiveExactly(36, over: network)
            let session = try self.securityEngine.connectionStart(
                request: request,
                challengeBytes: challenge
            )
            let hello = try session.handshakeOutbound(nowMillis: Self.uptimeMillis())
            try await Self.sendFrame(hello, maximum: Self.maxHandshakeFrameBytes, over: network)
            let accept = try await Self.receiveFrame(
                maximum: Self.maxHandshakeFrameBytes,
                over: network
            )
            let result = try session.handshakeReceiveAccept(
                message: accept,
                nowMillis: Self.uptimeMillis()
            )
            return AuthenticatedSessionResult(publicResult: result, session: session)
        }
        let publicResult = authentication.publicResult
        let authenticatedSession = authentication.session
        defer {
            try? authenticatedSession.finish()
            network.cancel()
            if connection === network { connection = nil }
        }
        guard publicResult.hostStaticPublicKey == host.hostStaticPublicKey,
              publicResult.identityGeneration == host.identityGeneration,
              publicResult.revocationEpoch == host.revocationEpoch,
              publicResult.grantedCapabilityBits & Self.observeScreensCapability
                  == Self.observeScreensCapability,
              publicResult.grantedCapabilityBits & ~Self.supportedCapabilityBits == 0 else {
            throw ControllerConnectionError.authenticationFailed
        }

        let ticket = try await openScreenSession(
            host: host,
            session: authenticatedSession,
            network: network
        )
        let viewer = ScreenViewer(cacheBytes: Self.screenCacheBytes)
        try viewer.connect(ticket: ticket.ticket)
        var watching = surface
        if let watching {
            viewer.subscribe(surface: watching, preview: preview)
        }
        await onOpened(ticket, viewer)

        let pump = ControllerScreenPump(ticket: ticket)
        while !Task.isCancelled {
            for (capability, bytes) in try pump.drain(viewer) {
                let sealed = try authenticatedSession.sealFrame(
                    kind: .screen,
                    capability: capability,
                    revocationEpoch: host.revocationEpoch,
                    payload: bytes
                )
                try await Self.sendFrame(
                    sealed,
                    maximum: Self.maxTerminalFrameBytes,
                    over: network
                )
            }
            let sealedFrame = try await Self.receiveFrame(
                maximum: Self.maxTerminalFrameBytes,
                over: network
            )
            let opened = try authenticatedSession.openFrame(frame: sealedFrame)
            guard opened.kind == .screen,
                  opened.revocationEpoch == host.revocationEpoch,
                  opened.capability == .observeScreens else {
                throw ControllerConnectionError.malformedResponse
            }
            let events = try viewer.receive(bytes: opened.payload)
            // The welcome is the first thing that names what this computer shares, so a phone
            // that asked for "whatever you have" subscribes here rather than guessing an id.
            if watching == nil {
                for case let .welcomed(surfaces, _) in events {
                    guard let first = surfaces.first else { continue }
                    watching = first.id
                    viewer.subscribe(surface: first.id, preview: preview)
                    break
                }
            }
            try await onEvent(events)
        }
    }

    /// Asks for a screen session and reads the one-time ticket it answers with.
    /// Starts a terminal on the computer and says which session it is.
    ///
    /// One command on a connection of its own: the computer opens the terminal, publishes it, and
    /// answers with the session to attach to. What it runs is this device's suggestion and the
    /// computer's decision, so a refusal is an answer rather than a failure.
    func createSession(
        host: PairedHostRecord,
        folder: String?,
        shell: String?,
        title: String?,
        viewport: TerminalViewportState
    ) async throws -> CreatedControllerSession {
        await cancel()
        guard host.schemaVersion == PairedHostRecord.currentSchemaVersion,
              host.capabilityBits & Self.createSessionCapability == Self.createSessionCapability
        else {
            throw ControllerConnectionError.capabilityDenied
        }
        let requested = host.capabilityBits & Self.supportedCapabilityBits
        let (network, _) = try await openHostConnection(host)
        connection = network
        let request = ConnectionStartRequest(
            staticKeyId: host.deviceStaticKeyId,
            ephemeralPrivateKey: try Self.randomBytes(count: 32),
            hostStaticPublicKey: host.hostStaticPublicKey,
            identityGeneration: host.identityGeneration,
            revocationEpoch: host.revocationEpoch,
            requestedCapabilityBits: requested,
            clientNonce: try Self.randomBytes(count: 32),
            nowMillis: Self.uptimeMillis()
        )
        let authentication: AuthenticatedSessionResult = try await withTimeout(Self.handshakeTimeout) {
            try await Self.send(Self.authenticationPreface, over: network)
            let prelude = try self.securityEngine.connectionPrelude(request: request)
            try await Self.send(prelude, over: network)
            let challenge = try await Self.receiveExactly(36, over: network)
            let session = try self.securityEngine.connectionStart(
                request: request,
                challengeBytes: challenge
            )
            let hello = try session.handshakeOutbound(nowMillis: Self.uptimeMillis())
            try await Self.sendFrame(hello, maximum: Self.maxHandshakeFrameBytes, over: network)
            let accept = try await Self.receiveFrame(
                maximum: Self.maxHandshakeFrameBytes,
                over: network
            )
            let result = try session.handshakeReceiveAccept(
                message: accept,
                nowMillis: Self.uptimeMillis()
            )
            return AuthenticatedSessionResult(publicResult: result, session: session)
        }
        let publicResult = authentication.publicResult
        let authenticatedSession = authentication.session
        defer {
            try? authenticatedSession.finish()
            network.cancel()
            if connection === network { connection = nil }
        }
        guard publicResult.hostStaticPublicKey == host.hostStaticPublicKey,
              publicResult.identityGeneration == host.identityGeneration,
              publicResult.revocationEpoch == host.revocationEpoch,
              publicResult.grantedCapabilityBits & Self.createSessionCapability
                  == Self.createSessionCapability,
              publicResult.grantedCapabilityBits & ~Self.supportedCapabilityBits == 0 else {
            throw ControllerConnectionError.authenticationFailed
        }

        let commandID = UUID()
        var command: [String: Any] = [
            "kind": "create_session",
            "columns": UInt32(viewport.columns),
            "rows": UInt32(viewport.rows),
        ]
        command["folder"] = Self.suggestion(folder) ?? NSNull()
        command["shell"] = Self.suggestion(shell) ?? NSNull()
        command["title"] = Self.suggestion(title) ?? NSNull()
        let envelope: [String: Any] = [
            "version": 1,
            "command_id": commandID.uuidString,
            "session_generation": host.sessionGeneration,
            "deadline_millis": Self.wallClockMillis().saturatingAdd(30_000),
            "command": command,
        ]
        let payload = try JSONSerialization.data(withJSONObject: envelope)
        let sealed = try authenticatedSession.sealFrame(
            kind: .control,
            capability: .createSession,
            revocationEpoch: host.revocationEpoch,
            payload: payload
        )
        try await Self.sendFrame(sealed, maximum: Self.maxSecureFrameBytes, over: network)
        let response = try await Self.receiveFrame(
            maximum: Self.maxSecureFrameBytes,
            over: network
        )
        let opened = try authenticatedSession.openFrame(frame: response)
        guard opened.kind == .control, opened.revocationEpoch == host.revocationEpoch else {
            throw ControllerConnectionError.malformedResponse
        }
        return try Self.createdSession(from: opened.payload, commandID: commandID)
    }

    /// A field the person left empty is not a suggestion; the computer's own default answers.
    private static func suggestion(_ value: String?) -> String? {
        guard let value, !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return nil
        }
        return value
    }

    private static func createdSession(
        from payload: Data,
        commandID: UUID
    ) throws -> CreatedControllerSession {
        guard let object = try JSONSerialization.jsonObject(with: payload) as? [String: Any],
              let kind = object["kind"] as? String else {
            throw ControllerConnectionError.malformedResponse
        }
        if kind == "error" {
            throw ControllerConnectionError.hostError((object["code"] as? String) ?? "unknown")
        }
        guard kind == "session_created",
              let answered = object["command_id"] as? String,
              UUID(uuidString: answered) == commandID,
              let sessionText = object["session_id"] as? String,
              let sessionID = UUID(uuidString: sessionText),
              let generation = object["occupant_generation"] as? UInt64,
              generation > 0 else {
            throw ControllerConnectionError.malformedResponse
        }
        return CreatedControllerSession(sessionID: sessionID, occupantGeneration: generation)
    }

    private func openScreenSession(
        host: PairedHostRecord,
        session: ControllerConnectionSession,
        network: any ControllerDuplexConnection
    ) async throws -> ControllerScreenTicket {
        let commandID = UUID()
        let envelope: [String: Any] = [
            "version": 1,
            "command_id": commandID.uuidString,
            "session_generation": host.sessionGeneration,
            "deadline_millis": Self.wallClockMillis().saturatingAdd(30_000),
            "command": ControllerScreenCommand.openBody(),
        ]
        let payload = try JSONSerialization.data(withJSONObject: envelope)
        let sealed = try session.sealFrame(
            kind: .control,
            capability: .observeScreens,
            revocationEpoch: host.revocationEpoch,
            payload: payload
        )
        try await Self.sendFrame(sealed, maximum: Self.maxSecureFrameBytes, over: network)
        let response = try await Self.receiveFrame(
            maximum: Self.maxSecureFrameBytes,
            over: network
        )
        let opened = try session.openFrame(frame: response)
        guard opened.kind == .control,
              opened.capability == .observeScreens,
              opened.revocationEpoch == host.revocationEpoch else {
            throw ControllerConnectionError.malformedResponse
        }
        let ticket = try ControllerScreenResponse.ticket(from: opened.payload)
        guard ticket.commandId == commandID else {
            throw ControllerConnectionError.malformedResponse
        }
        return ticket
    }

    func forgetDeviceSecret(host: PairedHostRecord) async throws {
        await cancel()
        try securityEngine.deleteSecureBlob(keyId: host.deviceStaticKeyId)
    }

    func cancel() async {
        await cancel(deleteCreatedKey: true)
    }

    private func cancel(deleteCreatedKey: Bool) async {
        if deleteCreatedKey, let pending = pairing, pending.createdKey {
            try? securityEngine.deleteSecureBlob(keyId: pending.keyID)
        }
        try? pairing?.session.finish()
        pairing = nil
        if let activeTerminal {
            try? activeTerminal.session.finish()
        }
        activeTerminal = nil
        connection?.cancel()
        connection = nil
    }

    /// Connects to the best of `routes` that answers, racing them as the shared route planner
    /// says. Transports that do not connect by address only ever use the first route.
    private func openFirstRoute(
        _ routes: [HostRoute],
        deadline: ContinuousClock.Instant? = nil
    ) async throws -> (network: any ControllerDuplexConnection, route: HostRoute) {
        do {
            let raced = try await raceRoutes(routes, deadline: deadline)
            return (raced.network, raced.route)
        } catch let raced as RacedRouteFailure {
            throw raced.failure.underlying
        }
    }

    /// Races `routes`, reporting the phone's network and how each attempt ended so the caller
    /// can remember the winner and explain a failure. Throws `RacedRouteFailure` when
    /// every address failed.
    private func raceRoutes(
        _ routes: [HostRoute],
        deadline: ContinuousClock.Instant? = nil,
        remembered: [RememberedRoute] = []
    ) async throws -> RacedRoute {
        guard transportFactory.openEndpoint != nil else {
            guard let route = routes.first else { throw ControllerPairingError.connectionClosed }
            var timeout = Self.handshakeTimeout
            if let deadline { timeout = min(timeout, Self.remaining(until: deadline)) }
            let network = try await withTimeout(timeout) {
                try await self.transportFactory.open(route)
            }
            return RacedRoute(network: network, route: route, phone: nil, outcomes: [])
        }
        let phone = networkProvider.currentNetwork()
        let plan = planRoutes(
            saved: routes.map { RouteAddress($0) },
            discovered: [],
            network: phone,
            remembered: remembered
        )
        do {
            let winner = try await ControllerRouteRace.run(
                plan,
                deadline: deadline,
                open: transportFactory.open
            )
            return RacedRoute(
                network: winner.network,
                route: winner.route,
                phone: phone,
                outcomes: winner.outcomes
            )
        } catch let failure as ControllerRouteRaceFailure {
            throw RacedRouteFailure(phone: phone, failure: failure)
        }
    }

    /// Keeps what a race to `host` found: the network it ran on, so the winner can be remembered
    /// for it, and the advice for the person.
    private func recordRace(
        for host: PairedHostRecord,
        phone: PhoneNetwork?,
        outcomes: [AttemptOutcome]
    ) {
        guard let phone else {
            lastRouteFingerprints[host.id] = nil
            lastRouteAdvices[host.id] = nil
            return
        }
        lastRouteFingerprints[host.id] = phone.fingerprint
        lastRouteAdvices[host.id] = ControllerRouteAdvice(routeAdvice(
            saved: host.routes.map { RouteAddress($0) },
            network: phone,
            outcomes: outcomes
        ))
    }

    /// Opens a paired Host by racing its saved routes, the one that worked on this network
    /// first. When none answers and the Host is announcing itself on this network, its Bonjour
    /// service is tried instead and the address it resolves to is returned so it can be saved.
    private func openHostConnection(
        _ host: PairedHostRecord
    ) async throws -> (network: any ControllerDuplexConnection, route: HostRoute?) {
        do {
            let raced = try await raceRoutes(
                host.routes,
                remembered: host.routeMemory.map(\.remembered)
            )
            recordRace(for: host, phone: raced.phone, outcomes: raced.outcomes)
            return (raced.network, transportFactory.openEndpoint == nil ? nil : raced.route)
        } catch let raced as RacedRouteFailure {
            recordRace(for: host, phone: raced.phone, outcomes: raced.failure.outcomes)
            let error = raced.failure.underlying
            guard !Task.isCancelled,
                  let openEndpoint = transportFactory.openEndpoint,
                  let discovery,
                  let discoveryID = host.discoveryId,
                  let endpoint = await discovery.endpoint(
                      discoveryID: discoveryID,
                      within: Self.discoveryLookupTimeout
                  ) else {
                throw error
            }
            let network = try await withTimeout(Self.handshakeTimeout) {
                try await openEndpoint(endpoint)
            }
            guard let route = network.remoteRoute, route.isPrivateNetworkRoute else {
                network.cancel()
                throw error
            }
            // Bonjour found it on this network, so nothing is left to advise.
            lastRouteAdvices[host.id] = nil
            return (network, route)
        }
    }

    private func openPairingTarget(
        _ target: ControllerPairingTarget,
        deadline: ContinuousClock.Instant
    ) async throws -> (network: any ControllerDuplexConnection, route: HostRoute) {
        guard let openEndpoint = transportFactory.openEndpoint else {
            throw ControllerConnectionError.capabilityDenied
        }
        switch target {
        case .discovered(let computer):
            let timeout = min(Self.handshakeTimeout, Self.remaining(until: deadline))
            let network = try await withTimeout(timeout) {
                try await openEndpoint(computer.endpoint)
            }
            guard let route = network.remoteRoute, route.isPrivateNetworkRoute else {
                network.cancel()
                throw ControllerPairingError.publicRouteRejected
            }
            return (network, route)
        case .address(let text):
            let routes = try await ControllerTypedAddress(text).resolvePrivateRoutes()
            return try await openFirstRoute(routes, deadline: deadline)
        }
    }

    private func authenticate(
        host: PairedHostRecord,
        requestedCapabilityBits: UInt16,
        network: any ControllerDuplexConnection
    ) async throws -> AuthenticatedSessionResult {
        let started = Self.uptimeMillis()
        let request = ConnectionStartRequest(
            staticKeyId: host.deviceStaticKeyId,
            ephemeralPrivateKey: try Self.randomBytes(count: 32),
            hostStaticPublicKey: host.hostStaticPublicKey,
            identityGeneration: host.identityGeneration,
            revocationEpoch: host.revocationEpoch,
            requestedCapabilityBits: requestedCapabilityBits,
            clientNonce: try Self.randomBytes(count: 32),
            nowMillis: started
        )
        return try await withTimeout(Self.handshakeTimeout) {
            try await Self.send(Self.authenticationPreface, over: network)
            let prelude = try self.securityEngine.connectionPrelude(request: request)
            try await Self.send(prelude, over: network)
            let challenge = try await Self.receiveExactly(36, over: network)
            let session = try self.securityEngine.connectionStart(
                request: request,
                challengeBytes: challenge
            )
            let hello = try session.handshakeOutbound(nowMillis: Self.uptimeMillis())
            try await Self.sendFrame(
                hello,
                maximum: Self.maxHandshakeFrameBytes,
                over: network
            )
            let accept = try await Self.receiveFrame(
                maximum: Self.maxHandshakeFrameBytes,
                over: network
            )
            let result = try session.handshakeReceiveAccept(
                message: accept,
                nowMillis: Self.uptimeMillis()
            )
            return AuthenticatedSessionResult(publicResult: result, session: session)
        }
    }

    private func sendTerminalMutation(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID,
        capability: ControllerCapability,
        capabilityBit: UInt16,
        payload: Data
    ) async throws {
        guard var terminal = activeTerminal,
              terminal.hostID == host.id,
              terminal.identity == identity,
              terminal.grantedCapabilityBits & capabilityBit == capabilityBit,
              connection === terminal.network else {
            throw ControllerConnectionError.capabilityDenied
        }
        guard terminal.pendingCapabilities.count < WriterControlReducer.maxQueuedChunks else {
            throw WriterControlFailure.inputQueueFull
        }
        terminal.pendingCapabilities[commandID] = capability
        activeTerminal = terminal
        do {
            let sealed = try terminal.session.sealFrame(
                kind: .control,
                capability: capability,
                revocationEpoch: host.revocationEpoch,
                payload: payload
            )
            try await Self.sendFrame(
                sealed,
                maximum: Self.maxSecureFrameBytes,
                over: terminal.network
            )
        } catch {
            if activeTerminal?.token == terminal.token {
                activeTerminal?.pendingCapabilities.removeValue(forKey: commandID)
            }
            throw error
        }
    }

    private func decodeOffer(_ text: String) throws -> ControllerPairingOfferEnvelope {
        guard !text.isEmpty, text.utf8.count <= Self.maxOfferBytes else {
            throw ControllerPairingError.invalidOffer
        }
        let data = Data(text.utf8)
        let object = try Self.exactPairingObject(
            data,
            keys: [
                "schema_version", "offer_id", "identity_generation", "revocation_epoch",
                "session_generation", "routes", "offer_bytes",
            ]
        )
        guard let routes = object["routes"] as? [[String: Any]],
              (1...HostRoute.maxRoutesPerHost).contains(routes.count),
              routes.allSatisfy({ Set($0.keys) == ["address", "port"] }) else {
            throw ControllerPairingError.invalidOffer
        }
        let envelope = try Self.decodeStrict(ControllerPairingOfferEnvelope.self, from: data)
        guard envelope.schemaVersion == 2,
              envelope.offerBytes.count <= Self.maxOfferBytes else {
            throw ControllerPairingError.invalidOffer
        }
        return envelope
    }

    private static func decodeCodeOffer(_ data: Data) throws -> CodePairingOfferEnvelope {
        _ = try exactPairingObject(
            data,
            keys: [
                "schema_version", "offer_id", "identity_generation", "revocation_epoch",
                "session_generation", "offer_bytes",
            ]
        )
        let envelope = try decodeStrict(CodePairingOfferEnvelope.self, from: data)
        guard envelope.schemaVersion == 1,
              envelope.identityGeneration > 0,
              envelope.offerBytes.count == codePairingOfferBytes else {
            throw ControllerPairingError.invalidOffer
        }
        return envelope
    }

    private static func exactPairingObject(_ data: Data, keys: Set<String>) throws -> [String: Any] {
        guard data.count <= maxOfferBytes,
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              Set(object.keys) == keys else {
            throw ControllerPairingError.invalidOffer
        }
        return object
    }

    private static func validateNames(hostName: String, deviceName: String) throws {
        guard !hostName.isEmpty,
              hostName.unicodeScalars.count <= 256,
              !deviceName.isEmpty,
              deviceName.unicodeScalars.count <= 64,
              !deviceName.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
        else {
            throw ControllerPairingError.invalidDeviceName
        }
    }

    private static func deviceKeyID(deviceID: UUID, hostKey: Data) -> String {
        "controller.device.\(deviceID.uuidString.lowercased()).\(fingerprint(hostKey).prefix(16))"
    }

    /// The device side of the three-message pairing handshake.
    private static func runPairingHandshake(
        _ session: ControllerPairingSession,
        over network: any ControllerDuplexConnection
    ) async throws {
        let hello = try session.pairingOutbound(nowMillis: uptimeMillis())
        try await sendFrame(hello, maximum: maxHandshakeFrameBytes, over: network)
        let proof = try await receiveFrame(maximum: maxHandshakeFrameBytes, over: network)
        try session.pairingReceive(message: proof, nowMillis: uptimeMillis())
        let deviceProof = try session.pairingOutbound(nowMillis: uptimeMillis())
        try await sendFrame(deviceProof, maximum: maxHandshakeFrameBytes, over: network)
    }

    /// A wrong code shows up as a failed exchange or a closed connection, never as a distinct
    /// answer, so every failure after connecting reads as a rejected code unless it is local.
    private static func codePairingFailure(_ error: Error) -> Error {
        if error is CancellationError || error is SecureBlobError { return error }
        if let error = error as? ControllerPairingError {
            switch error {
            case .timedOut, .cancelled, .randomUnavailable, .invalidDeviceName,
                 .expiredOrIncompatibleOffer, .acknowledgementUncertain:
                return error
            default:
                return ControllerPairingError.codeRejected
            }
        }
        if let error = error as? ControllerBindingError {
            switch error {
            case .IncompatibleVersion, .SecureBlobMissing, .SecureBlobLocked,
                 .SecureBlobPermissionDenied, .SecureBlobInvalid, .SecureBlobUnavailable:
                return error
            default:
                return ControllerPairingError.codeRejected
            }
        }
        return ControllerPairingError.codeRejected
    }

    private static func remaining(until deadline: ContinuousClock.Instant) -> Duration {
        max(deadline - ContinuousClock.now, .zero)
    }

    private static let pairingPreface = Data([0x54, 0x52, 0x43, 0x4e, 0x00, 0x01, 0x02, 0x00])
    private static let codePairingPreface = Data([0x54, 0x52, 0x43, 0x4e, 0x00, 0x01, 0x03, 0x00])
    private static let authenticationPreface = Data([0x54, 0x52, 0x43, 0x4e, 0x00, 0x01, 0x01, 0x00])

    private static func fetchStableSnapshot(
        host: PairedHostRecord,
        session: ControllerConnectionSession,
        network: any ControllerDuplexConnection,
        capabilityBits: UInt16
    ) async throws -> ControllerFleetSnapshot {
        for _ in 0..<3 {
            var offset: UInt32 = 0
            var revision: UInt64?
            var updateSequence: UInt64?
            var summaries: [ControllerSessionSummary] = []
            var restart = false

            repeat {
                let commandID = UUID()
                let envelope = ListSessionsCommandEnvelope(
                    commandId: commandID,
                    sessionGeneration: host.sessionGeneration,
                    deadlineMillis: wallClockMillis().saturatingAdd(30_000),
                    command: ListSessionsCommand(
                        offset: offset,
                        limit: UInt16(ControllerCacheLimits.maxPageRecords),
                        expectedRevision: revision
                    )
                )
                let command = try JSONEncoder().encode(envelope)
                let sealed = try session.sealFrame(
                    kind: .control,
                    capability: .observeSessions,
                    revocationEpoch: host.revocationEpoch,
                    payload: command
                )
                try await sendFrame(sealed, maximum: maxSecureFrameBytes, over: network)
                let responseFrame = try await receiveFrame(
                    maximum: maxSecureFrameBytes,
                    over: network
                )
                let opened = try session.openFrame(frame: responseFrame)
                guard opened.kind == .control,
                      opened.capability == .observeSessions,
                      opened.revocationEpoch == host.revocationEpoch else {
                    throw ControllerConnectionError.malformedResponse
                }

                if responseKind(opened.payload) == "error" {
                    let error = try decodeExactError(opened.payload)
                    guard error.commandId == commandID else {
                        throw ControllerConnectionError.malformedResponse
                    }
                    if error.code == "snapshot_changed" && !error.completionUnknown {
                        restart = true
                        break
                    }
                    throw ControllerConnectionError.hostError(error.code)
                }

                let page = try decodeExactSessions(opened.payload)
                guard page.kind == "sessions",
                      page.commandId == commandID,
                      page.revision > 0,
                      page.updateSequence > 0,
                      page.sessions.count <= ControllerCacheLimits.maxPageRecords,
                      revision == nil || revision == page.revision,
                      updateSequence == nil || updateSequence == page.updateSequence else {
                    throw ControllerConnectionError.sequenceGap
                }
                revision = page.revision
                updateSequence = page.updateSequence
                let mapped = try page.sessions.map { value in
                    let summary = ControllerSessionSummary(
                        id: value.sessionId,
                        hostInstanceID: value.hostInstanceId,
                        origin: value.origin ?? .unknown,
                        runtime: value.runtime,
                        capabilities: value.capabilities ?? [],
                        title: value.title,
                        project: value.project,
                        group: value.group,
                        lifecycle: value.lifecycle,
                        activity: value.activity,
                        occupantGeneration: value.occupantGeneration,
                        lastOutputSequence: value.lastOutputSequence,
                        hasWriter: value.hasWriter,
                        unreadCount: value.unread == true ? 1 : 0
                    )
                    try summary.validate()
                    return summary
                }
                summaries.append(contentsOf: mapped)
                guard summaries.count <= ControllerCacheLimits.maxSessionsPerHost else {
                    throw ControllerConnectionError.resourceLimit
                }
                guard let next = page.nextOffset else { break }
                guard next > offset, Int(next) == summaries.count else {
                    throw ControllerConnectionError.sequenceGap
                }
                offset = next
            } while true

            if restart { continue }
            guard let revision, let updateSequence else {
                throw ControllerConnectionError.malformedResponse
            }
            return ControllerFleetSnapshot(
                revision: revision,
                updateSequence: updateSequence,
                capabilityBits: capabilityBits,
                sessions: summaries
            )
        }
        throw ControllerConnectionError.sequenceGap
    }

    private static func responseKind(_ data: Data) -> String? {
        (try? JSONSerialization.jsonObject(with: data) as? [String: Any])?["kind"] as? String
    }

    private static func decodeExactSessions(_ data: Data) throws -> SessionsResponsePayload {
        let object = try exactObject(
            data,
            keys: ["kind", "command_id", "revision", "update_sequence", "sessions", "next_offset"]
        )
        let legacyKeys: Set<String> = [
            "session_id", "title", "lifecycle", "occupant_generation",
            "last_output_sequence", "has_writer",
        ]
        let enrichedKeys = legacyKeys.union([
            "host_instance_id", "origin", "runtime", "capabilities",
            "project", "group", "activity", "unread",
        ])
        guard let sessions = object["sessions"] as? [[String: Any]],
              sessions.allSatisfy({ summary in
                  let keys = Set(summary.keys)
                  return keys == legacyKeys || keys == enrichedKeys
              }) else {
            throw ControllerConnectionError.malformedResponse
        }
        return try JSONDecoder().decode(SessionsResponsePayload.self, from: data)
    }

    private static func decodeExactError(_ data: Data) throws -> ErrorResponsePayload {
        _ = try exactObject(
            data,
            keys: ["kind", "command_id", "code", "completion_unknown"]
        )
        return try JSONDecoder().decode(ErrorResponsePayload.self, from: data)
    }

    private static func exactObject(_ data: Data, keys: Set<String>) throws -> [String: Any] {
        guard data.count <= maxSecureFrameBytes,
              let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              Set(object.keys) == keys else {
            throw ControllerConnectionError.malformedResponse
        }
        return object
    }

    private static func wallClockMillis() -> UInt64 {
        UInt64(Date().timeIntervalSince1970 * 1_000)
    }

    private static func send(
        _ data: Data,
        over connection: any ControllerDuplexConnection
    ) async throws {
        try await connection.send(data)
    }

    private static func sendFrame(
        _ payload: Data,
        maximum: Int,
        over connection: any ControllerDuplexConnection
    ) async throws {
        guard !payload.isEmpty, payload.count <= maximum, payload.count <= Int(UInt32.max) else {
            throw ControllerPairingError.frameTooLarge
        }
        var length = UInt32(payload.count).bigEndian
        var framed = Data(bytes: &length, count: MemoryLayout<UInt32>.size)
        framed.append(payload)
        try await send(framed, over: connection)
    }

    private static func receiveFrame(
        maximum: Int,
        over connection: any ControllerDuplexConnection
    ) async throws -> Data {
        let prefix = try await receiveExactly(4, over: connection)
        let length = prefix.reduce(UInt32(0)) { ($0 << 8) | UInt32($1) }
        guard length > 0, length <= UInt32(maximum) else {
            throw ControllerPairingError.frameTooLarge
        }
        return try await receiveExactly(Int(length), over: connection)
    }

    private static func receiveExactly(
        _ count: Int,
        over connection: any ControllerDuplexConnection
    ) async throws -> Data {
        var result = Data()
        result.reserveCapacity(count)
        while result.count < count {
            let remaining = count - result.count
            let chunk = try await connection.receive(maximumLength: remaining)
            result.append(chunk)
        }
        return result
    }

    private static func randomBytes(count: Int) throws -> Data {
        var bytes = Data(count: count)
        let status = bytes.withUnsafeMutableBytes { buffer in
            SecRandomCopyBytes(kSecRandomDefault, count, buffer.baseAddress!)
        }
        guard status == errSecSuccess, bytes.contains(where: { $0 != 0 }) else {
            throw ControllerPairingError.randomUnavailable
        }
        return bytes
    }

    private static func decodeStrict<T: Decodable>(_ type: T.Type, from data: Data) throws -> T {
        do {
            return try JSONDecoder().decode(type, from: data)
        } catch {
            throw ControllerPairingError.invalidOffer
        }
    }

    private static func fingerprint(_ key: Data) -> String {
        key.map { String(format: "%02x", $0) }.joined()
    }

    private static func uptimeMillis() -> UInt64 {
        UInt64(ProcessInfo.processInfo.systemUptime * 1_000)
    }

    private func withTimeout<T: Sendable>(
        _ duration: Duration,
        operation: @escaping @Sendable () async throws -> T
    ) async throws -> T {
        try await withThrowingTaskGroup(of: T.self) { group in
            group.addTask { try await operation() }
            group.addTask {
                try await Task.sleep(for: duration)
                throw ControllerPairingError.timedOut
            }
            guard let result = try await group.next() else {
                throw ControllerPairingError.cancelled
            }
            group.cancelAll()
            return result
        }
    }
}

private final class ConnectionStartGate: @unchecked Sendable {
    private let lock = NSLock()
    private var completed = false

    func claim() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard !completed else { return false }
        completed = true
        return true
    }
}

private struct RacedRoute: Sendable {
    let network: any ControllerDuplexConnection
    let route: HostRoute
    /// The phone's network, when the transport chooses between addresses.
    let phone: PhoneNetwork?
    let outcomes: [AttemptOutcome]
}

private struct RacedRouteFailure: Error {
    let phone: PhoneNetwork
    let failure: ControllerRouteRaceFailure
}

private struct AuthenticatedSessionResult: Sendable {
    let publicResult: ConnectionPublicResult
    let session: ControllerConnectionSession
}

private struct ActiveTerminalConnection: Sendable {
    let token: UUID
    let hostID: String
    let identity: ReadOnlyAttachIdentity
    let grantedCapabilityBits: UInt16
    let network: any ControllerDuplexConnection
    let session: ControllerConnectionSession
    let attachCommandID: UUID
    var pendingCapabilities: [UUID: ControllerCapability]
}

private struct PendingPairing: Sendable {
    let routes: [HostRoute]
    let hostName: String
    let deviceName: String
    let deviceID: UUID
    let keyID: String
    let createdKey: Bool
    let session: ControllerPairingSession
    /// Nil for code pairing, which has no SAS to compare.
    let sas: String?
    let hostKey: Data
    let capabilityBits: UInt16
    let identityGeneration: UInt64
    let revocationEpoch: UInt64
    let sessionGeneration: UInt64
}

enum ControllerPairingError: Error, Equatable {
    case invalidOffer
    case expiredOrIncompatibleOffer
    case publicRouteRejected
    case invalidDeviceName
    case randomUnavailable
    case frameTooLarge
    case connectionClosed
    case timedOut
    case cancelled
    case noPairingInProgress
    case rejected
    case hostIdentityChanged
    case invalidAcknowledgement
    case acknowledgementUncertain
    case invalidCode
    case codeRejected
    case invalidAddress
}

enum ControllerConnectionError: Error, Equatable {
    case capabilityDenied
    case authenticationFailed
    case malformedResponse
    case sequenceGap
    case resourceLimit
    case hostError(String)
}

private extension UInt64 {
    func saturatingAdd(_ value: UInt64) -> UInt64 {
        addingReportingOverflow(value).overflow ? .max : self + value
    }
}
