import Foundation
@preconcurrency import Network
import Security

protocol ControllerConnecting: Sendable {
    func beginPairing(
        offerText: String,
        hostName: String,
        deviceName: String,
        deviceID: UUID
    ) async throws -> ControllerPairingChallenge
    func finishPairing(matches: Bool) async throws -> PairedHostRecord
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
    func forgetDeviceSecret(host: PairedHostRecord) async throws
    func cancel() async
}

struct AppleControllerRouteConnections: Sendable {
    let privateNetwork: (any ControllerConnecting)?
    let ssh: (any ControllerConnecting)?
    let selfHostedRelay: (any ControllerConnecting)?

    init(
        privateNetwork: (any ControllerConnecting)?,
        ssh: (any ControllerConnecting)? = nil,
        selfHostedRelay: (any ControllerConnecting)? = nil
    ) {
        self.privateNetwork = privateNetwork
        self.ssh = ssh
        self.selfHostedRelay = selfHostedRelay
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
}

extension ControllerConnecting {
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
    let addressFamily: String
    let address: String
    let port: UInt16
    let offerBytes: [UInt8]

    private enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case offerId = "offer_id"
        case identityGeneration = "identity_generation"
        case revocationEpoch = "revocation_epoch"
        case sessionGeneration = "session_generation"
        case addressFamily = "address_family"
        case address
        case port
        case offerBytes = "offer_bytes"
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
    private static let supportedCapabilityBits = observeCapability
        | attachCapability | inputCapability | resizeCapability | approvalCapability
    private static let maxOfferBytes = 4 * 1_024
    private static let maxHandshakeFrameBytes = 1_024
    private static let maxSecureFrameBytes = 64 * 1_024
    private static let maxTerminalFrameBytes = 1 * 1_024 * 1_024
    private static let handshakeTimeout: Duration = .seconds(30)

    private let securityEngine: ControllerSecurityEngine
    private var connection: NWConnection?
    private var pairing: PendingPairing?
    private var activeTerminal: ActiveTerminalConnection?

    init(blobStore: SecureBlobStore) throws {
        self.securityEngine = try ControllerSecurityEngine(blobs: blobStore)
    }

    func beginPairing(
        offerText: String,
        hostName: String,
        deviceName: String,
        deviceID: UUID
    ) async throws -> ControllerPairingChallenge {
        await cancel()
        guard !hostName.isEmpty,
              hostName.unicodeScalars.count <= 256,
              !deviceName.isEmpty,
              deviceName.unicodeScalars.count <= 64,
              !deviceName.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
        else {
            throw ControllerPairingError.invalidDeviceName
        }

        let envelope = try decodeOffer(offerText)
        let offerBytes = Data(envelope.offerBytes)
        let summary = try securityEngine.decodeOfferSummary(offerBytes: offerBytes)
        let nowSeconds = UInt64(Date().timeIntervalSince1970)
        guard summary.version.major == 1,
              summary.version.minor == 0,
              summary.expiresAtUnixSeconds > nowSeconds,
              summary.hostStaticPublicKey.count == 32,
              envelope.identityGeneration > 0 else {
            throw ControllerPairingError.expiredOrIncompatibleOffer
        }
        let route = try HostRoute(address: envelope.address, port: envelope.port)
        guard Self.isPrivateRoute(envelope: envelope) else {
            throw ControllerPairingError.publicRouteRejected
        }

        let keyID = "controller.device.\(deviceID.uuidString.lowercased()).\(Self.fingerprint(summary.hostStaticPublicKey).prefix(16))"
        var createdKey = false
        if try securityEngine.secureBlobStatus(keyId: keyID) == .missing {
            try securityEngine.storeSecureBlob(keyId: keyID, value: try Self.randomBytes(count: 32))
            createdKey = true
        }

        do {
            let network = try await withTimeout(Self.handshakeTimeout) {
                try await Self.openConnection(route: route)
            }
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
                let hello = try session.pairingOutbound(nowMillis: Self.uptimeMillis())
                try await Self.sendFrame(
                    hello,
                    maximum: Self.maxHandshakeFrameBytes,
                    over: network
                )
                let proof = try await Self.receiveFrame(
                    maximum: Self.maxHandshakeFrameBytes,
                    over: network
                )
                try session.pairingReceive(message: proof, nowMillis: Self.uptimeMillis())
                let deviceProof = try session.pairingOutbound(nowMillis: Self.uptimeMillis())
                try await Self.sendFrame(
                    deviceProof,
                    maximum: Self.maxHandshakeFrameBytes,
                    over: network
                )
            }
            let sas = try session.sas().value
            pairing = PendingPairing(
                envelope: envelope,
                route: route,
                hostName: hostName,
                deviceName: deviceName,
                deviceID: deviceID,
                keyID: keyID,
                createdKey: createdKey,
                session: session,
                sas: sas,
                hostKey: summary.hostStaticPublicKey,
                capabilityBits: summary.capabilityBits
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
        guard let pending = pairing, let connection else {
            throw ControllerPairingError.noPairingInProgress
        }
        guard matches else {
            _ = try? pending.session.confirmOrReject(
                confirmation: .reject,
                comparedSas: pending.sas,
                revocationEpoch: pending.envelope.revocationEpoch
            )
            if pending.createdKey {
                try? securityEngine.deleteSecureBlob(keyId: pending.keyID)
            }
            await cancel()
            throw ControllerPairingError.rejected
        }

        var registrationSent = false
        do {
            let result = try pending.session.confirmOrReject(
                confirmation: .confirm,
                comparedSas: pending.sas,
                revocationEpoch: pending.envelope.revocationEpoch
            )
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
                revocationEpoch: pending.envelope.revocationEpoch,
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
                      opened.revocationEpoch == pending.envelope.revocationEpoch else {
                    throw ControllerPairingError.invalidAcknowledgement
                }
                return try Self.decodeStrict(PairingHostAckPayload.self, from: opened.payload)
            }
            guard ack.schemaVersion == 1,
                  ack.deviceId == pending.deviceID,
                  ack.identityGeneration == pending.envelope.identityGeneration,
                  ack.revocationEpoch == pending.envelope.revocationEpoch,
                  ack.sessionGeneration == pending.envelope.sessionGeneration,
                  ack.capabilityBits & ~pending.capabilityBits == 0 else {
                throw ControllerPairingError.invalidAcknowledgement
            }
            let record = try PairedHostRecord(
                id: Self.fingerprint(pending.hostKey),
                displayName: pending.hostName,
                route: pending.route,
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
                        route: pending.route,
                        hostStaticPublicKey: pending.hostKey,
                        deviceStaticKeyId: pending.keyID,
                        deviceId: pending.deviceID,
                        identityGeneration: pending.envelope.identityGeneration,
                        revocationEpoch: pending.envelope.revocationEpoch,
                        sessionGeneration: pending.envelope.sessionGeneration,
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
        let network = try await withTimeout(Self.handshakeTimeout) {
            try await Self.openConnection(route: host.route)
        }
        connection = network
        await progress(.authenticating)
        let started = Self.uptimeMillis()
        let request = ConnectionStartRequest(
            staticKeyId: host.deviceStaticKeyId,
            ephemeralPrivateKey: try Self.randomBytes(count: 32),
            hostStaticPublicKey: host.hostStaticPublicKey,
            identityGeneration: host.identityGeneration,
            revocationEpoch: host.revocationEpoch,
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
            network.stateUpdateHandler = nil
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

        return try await withTimeout(Self.handshakeTimeout) {
            try await Self.fetchStableSnapshot(
                host: host,
                session: authenticatedSession,
                network: network,
                capabilityBits: publicResult.grantedCapabilityBits
            )
        }
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

        let network = try await withTimeout(Self.handshakeTimeout) {
            try await Self.openConnection(route: host.route)
        }
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
            network.stateUpdateHandler = nil
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
        let network = try await withTimeout(Self.handshakeTimeout) {
            try await Self.openConnection(route: host.route)
        }
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
            network.stateUpdateHandler = nil
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
        connection?.stateUpdateHandler = nil
        connection?.cancel()
        connection = nil
    }

    private func authenticate(
        host: PairedHostRecord,
        requestedCapabilityBits: UInt16,
        network: NWConnection
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
        let envelope = try Self.decodeStrict(
            ControllerPairingOfferEnvelope.self,
            from: Data(text.utf8)
        )
        guard envelope.schemaVersion == 1,
              envelope.offerBytes.count <= Self.maxOfferBytes else {
            throw ControllerPairingError.invalidOffer
        }
        return envelope
    }

    private static let pairingPreface = Data([0x54, 0x52, 0x43, 0x4e, 0x00, 0x01, 0x02, 0x00])
    private static let authenticationPreface = Data([0x54, 0x52, 0x43, 0x4e, 0x00, 0x01, 0x01, 0x00])

    private static func fetchStableSnapshot(
        host: PairedHostRecord,
        session: ControllerConnectionSession,
        network: NWConnection,
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

    private static func openConnection(route: HostRoute) async throws -> NWConnection {
        let host = NWEndpoint.Host(route.address)
        guard let port = NWEndpoint.Port(rawValue: route.port) else {
            throw ControllerPairingError.invalidOffer
        }
        let connection = NWConnection(host: host, port: port, using: .tcp)
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                let gate = ConnectionStartGate()
                connection.stateUpdateHandler = { state in
                    switch state {
                    case .ready:
                        if gate.claim() { continuation.resume(returning: connection) }
                    case .failed(let error):
                        if gate.claim() { continuation.resume(throwing: error) }
                    case .cancelled:
                        if gate.claim() { continuation.resume(throwing: CancellationError()) }
                    default:
                        break
                    }
                }
                connection.start(queue: DispatchQueue(label: "com.termirust.controller.connection"))
            }
        } onCancel: {
            connection.cancel()
        }
    }

    private static func send(_ data: Data, over connection: NWConnection) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            connection.send(content: data, completion: .contentProcessed { error in
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume()
                }
            })
        }
    }

    private static func sendFrame(
        _ payload: Data,
        maximum: Int,
        over connection: NWConnection
    ) async throws {
        guard !payload.isEmpty, payload.count <= maximum, payload.count <= Int(UInt32.max) else {
            throw ControllerPairingError.frameTooLarge
        }
        var length = UInt32(payload.count).bigEndian
        var framed = Data(bytes: &length, count: MemoryLayout<UInt32>.size)
        framed.append(payload)
        try await send(framed, over: connection)
    }

    private static func receiveFrame(maximum: Int, over connection: NWConnection) async throws -> Data {
        let prefix = try await receiveExactly(4, over: connection)
        let length = prefix.reduce(UInt32(0)) { ($0 << 8) | UInt32($1) }
        guard length > 0, length <= UInt32(maximum) else {
            throw ControllerPairingError.frameTooLarge
        }
        return try await receiveExactly(Int(length), over: connection)
    }

    private static func receiveExactly(_ count: Int, over connection: NWConnection) async throws -> Data {
        var result = Data()
        result.reserveCapacity(count)
        while result.count < count {
            let remaining = count - result.count
            let chunk: Data = try await withCheckedThrowingContinuation { continuation in
                connection.receive(
                    minimumIncompleteLength: 1,
                    maximumLength: remaining
                ) { data, _, complete, error in
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

    private static func isPrivateRoute(envelope: ControllerPairingOfferEnvelope) -> Bool {
        if envelope.addressFamily == "ipv4", let address = IPv4Address(envelope.address) {
            let bytes = [UInt8](address.rawValue)
            guard bytes.count == 4 else { return false }
            let privateAddress = bytes[0] == 10
                || (bytes[0] == 172 && (16...31).contains(bytes[1]))
                || (bytes[0] == 192 && bytes[1] == 168)
                || (bytes[0] == 100 && (64...127).contains(bytes[1]))
            return privateAddress && bytes != [127, 0, 0, 1] && bytes != [0, 0, 0, 0]
        }
        if envelope.addressFamily == "ipv6", let address = IPv6Address(envelope.address) {
            let bytes = [UInt8](address.rawValue)
            return bytes.count == 16 && (bytes[0] & 0xfe) == 0xfc
        }
        return false
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

private struct AuthenticatedSessionResult: Sendable {
    let publicResult: ConnectionPublicResult
    let session: ControllerConnectionSession
}

private struct ActiveTerminalConnection: Sendable {
    let token: UUID
    let hostID: String
    let identity: ReadOnlyAttachIdentity
    let grantedCapabilityBits: UInt16
    let network: NWConnection
    let session: ControllerConnectionSession
    let attachCommandID: UUID
    var pendingCapabilities: [UUID: ControllerCapability]
}

private struct PendingPairing: Sendable {
    let envelope: ControllerPairingOfferEnvelope
    let route: HostRoute
    let hostName: String
    let deviceName: String
    let deviceID: UUID
    let keyID: String
    let createdKey: Bool
    let session: ControllerPairingSession
    let sas: String
    let hostKey: Data
    let capabilityBits: UInt16
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
