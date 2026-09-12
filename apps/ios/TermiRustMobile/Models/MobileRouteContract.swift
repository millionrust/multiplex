import Foundation

enum MobileItemKind: String, Codable, Sendable {
    case savedConnection = "saved_connection"
    case pairedDevice = "paired_device"
    case durableDeviceSession = "durable_device_session"
}

enum MobileCredentialOwner: String, Codable, Sendable {
    case sshCredential = "ssh_credential"
    case devicePairingIdentity = "device_pairing_identity"
}

enum MobileContinuityOwner: String, Codable, Sendable {
    case none
    case remoteTmuxIfEnabled = "remote_tmux_if_enabled"
    case hostService = "host_service"
}

enum MobileRouteCapability: String, Codable, CaseIterable, Sendable {
    case listDeviceSessions = "list_device_sessions"
    case terminalOutput = "terminal_output"
    case terminalInput = "terminal_input"
    case terminalResize = "terminal_resize"
    case persistentTmux = "persistent_tmux"
    case durableReplay = "durable_replay"
    case authoritativeActivity = "authoritative_activity"
    case singleWriter = "single_writer"
}

enum MobileRouteContractError: String, Error, Equatable, Sendable {
    case unknownItemKind = "unknown_item_kind"
    case unknownCapability = "unknown_capability"
    case credentialOwnerMismatch = "credential_owner_mismatch"
    case continuityOwnerMismatch = "continuity_owner_mismatch"
    case capabilityMismatch = "capability_mismatch"
    case terminalOwnershipMismatch = "terminal_ownership_mismatch"
}

struct MobileRouteProjection: Equatable, Sendable {
    let itemKind: MobileItemKind
    let credentialOwner: MobileCredentialOwner
    let continuityOwner: MobileContinuityOwner
    let capabilities: Set<MobileRouteCapability>
    let canOpenTerminal: Bool

    static func validated(
        itemKind rawItemKind: String,
        credentialOwner rawCredentialOwner: String,
        continuityOwner rawContinuityOwner: String,
        capabilities rawCapabilities: [String],
        canOpenTerminal: Bool
    ) throws -> MobileRouteProjection {
        guard let itemKind = MobileItemKind(rawValue: rawItemKind) else {
            throw MobileRouteContractError.unknownItemKind
        }
        guard let credentialOwner = MobileCredentialOwner(rawValue: rawCredentialOwner) else {
            throw MobileRouteContractError.credentialOwnerMismatch
        }
        guard let continuityOwner = MobileContinuityOwner(rawValue: rawContinuityOwner) else {
            throw MobileRouteContractError.continuityOwnerMismatch
        }
        let parsedCapabilities = rawCapabilities.compactMap(MobileRouteCapability.init(rawValue:))
        guard parsedCapabilities.count == rawCapabilities.count else {
            throw MobileRouteContractError.unknownCapability
        }
        let capabilities = Set(parsedCapabilities)
        guard capabilities.count == rawCapabilities.count else {
            throw MobileRouteContractError.capabilityMismatch
        }

        let expectedCredential: MobileCredentialOwner
        let expectedContinuity: MobileContinuityOwner
        let required: Set<MobileRouteCapability>
        let allowed: Set<MobileRouteCapability>
        let expectedTerminal: Bool
        switch itemKind {
        case .savedConnection:
            expectedCredential = .sshCredential
            expectedContinuity = .remoteTmuxIfEnabled
            required = [.terminalOutput]
            allowed = [.terminalOutput, .terminalInput, .terminalResize, .persistentTmux]
            expectedTerminal = true
        case .pairedDevice:
            expectedCredential = .devicePairingIdentity
            expectedContinuity = .none
            required = [.listDeviceSessions]
            allowed = required
            expectedTerminal = false
        case .durableDeviceSession:
            expectedCredential = .devicePairingIdentity
            expectedContinuity = .hostService
            required = [.terminalOutput, .durableReplay, .authoritativeActivity, .singleWriter]
            allowed = [
                .terminalOutput, .terminalInput, .terminalResize, .durableReplay,
                .authoritativeActivity, .singleWriter,
            ]
            expectedTerminal = true
        }
        guard credentialOwner == expectedCredential else {
            throw MobileRouteContractError.credentialOwnerMismatch
        }
        guard continuityOwner == expectedContinuity else {
            throw MobileRouteContractError.continuityOwnerMismatch
        }
        guard required.isSubset(of: capabilities), capabilities.isSubset(of: allowed) else {
            throw MobileRouteContractError.capabilityMismatch
        }
        guard canOpenTerminal == expectedTerminal else {
            throw MobileRouteContractError.terminalOwnershipMismatch
        }
        return MobileRouteProjection(
            itemKind: itemKind,
            credentialOwner: credentialOwner,
            continuityOwner: continuityOwner,
            capabilities: capabilities,
            canOpenTerminal: canOpenTerminal
        )
    }
}

enum MobileRootDestination: String, CaseIterable, Sendable {
    case connections
    case devices
}

struct MobileTerminalDestination: Identifiable, Equatable, Sendable {
    let id: String
    let title: String
    let badge: String
    let route: MobileRouteProjection

    init(id: String, title: String, badge: String, route: MobileRouteProjection) throws {
        guard route.canOpenTerminal, route.itemKind != .pairedDevice else {
            throw MobileRouteContractError.terminalOwnershipMismatch
        }
        self.id = id
        self.title = title
        self.badge = badge
        self.route = route
    }
}
