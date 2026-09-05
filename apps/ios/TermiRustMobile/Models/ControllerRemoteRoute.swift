import Foundation

enum ControllerRemoteRouteKind: String, Codable, CaseIterable, Sendable {
    case localIPC = "local_ipc"
    case privateNetwork = "private_network"
    case ssh
    case selfHostedRelay = "self_hosted_relay"

    static let appleCases: [Self] = [.privateNetwork, .ssh, .selfHostedRelay]
}

enum ControllerRemotePlatform: String, Codable, Sendable {
    case desktop
    case appleMobile = "apple_mobile"
    case android
}

enum ControllerRemoteTrustLayer: String, Codable, Sendable {
    case sameUserOSBoundary = "same_user_os_boundary"
    case privateAddress = "private_address"
    case sshHostKey = "ssh_host_key"
    case systemTLS = "system_tls"
    case spkiPin = "spki_pin"
    case relayAdmission = "relay_admission"
    case controllerAuthentication = "controller_authentication"
}

enum ControllerRemoteConfigurationRequirement: String, Codable, Sendable {
    case privateEndpoint = "private_endpoint"
    case sshEndpoint = "ssh_endpoint"
    case sshCredential = "ssh_credential"
    case relayEndpoint = "relay_endpoint"
    case relaySPKIPin = "relay_spki_pin"
    case relayCredential = "relay_credential"
    case pairedDevice = "paired_device"
}

enum ControllerRemoteCapability: String, Codable, CaseIterable, Sendable {
    case listSessions = "list_sessions"
    case attachOutput = "attach_output"
    case sendInput = "send_input"
    case resize
    case respondToApproval = "respond_to_approval"
    case detach
}

struct ControllerRemoteRoutePolicy: Codable, Equatable, Sendable {
    let kind: ControllerRemoteRouteKind
    let platforms: [ControllerRemotePlatform]
    let trustLayers: [ControllerRemoteTrustLayer]
    let configuration: [ControllerRemoteConfigurationRequirement]
    let capabilities: [ControllerRemoteCapability]
    let allowsAutomaticSwitch: Bool
    let allowsOfflineMutations: Bool

    private enum CodingKeys: String, CodingKey {
        case kind, platforms, configuration, capabilities
        case trustLayers = "trust_layers"
        case allowsAutomaticSwitch = "allows_automatic_switch"
        case allowsOfflineMutations = "allows_offline_mutations"
    }

    static func canonical(_ kind: ControllerRemoteRouteKind) -> Self {
        let capabilities = ControllerRemoteCapability.allCases
        switch kind {
        case .localIPC:
            return Self(
                kind: kind,
                platforms: [.desktop],
                trustLayers: [.sameUserOSBoundary],
                configuration: [],
                capabilities: capabilities,
                allowsAutomaticSwitch: false,
                allowsOfflineMutations: false
            )
        case .privateNetwork:
            return Self(
                kind: kind,
                platforms: [.desktop, .appleMobile, .android],
                trustLayers: [.privateAddress, .controllerAuthentication],
                configuration: [.privateEndpoint, .pairedDevice],
                capabilities: capabilities,
                allowsAutomaticSwitch: false,
                allowsOfflineMutations: false
            )
        case .ssh:
            return Self(
                kind: kind,
                platforms: [.desktop, .appleMobile, .android],
                trustLayers: [.sshHostKey, .controllerAuthentication],
                configuration: [.sshEndpoint, .sshCredential, .pairedDevice],
                capabilities: capabilities,
                allowsAutomaticSwitch: false,
                allowsOfflineMutations: false
            )
        case .selfHostedRelay:
            return Self(
                kind: kind,
                platforms: [.desktop, .appleMobile, .android],
                trustLayers: [
                    .systemTLS, .spkiPin, .relayAdmission, .controllerAuthentication,
                ],
                configuration: [
                    .relayEndpoint, .relaySPKIPin, .relayCredential, .pairedDevice,
                ],
                capabilities: capabilities,
                allowsAutomaticSwitch: false,
                allowsOfflineMutations: false
            )
        }
    }

    func supports(_ platform: ControllerRemotePlatform) -> Bool {
        platforms.contains(platform)
    }
}

enum ControllerRemoteRoutePhase: String, Codable, Sendable {
    case disabled
    case unavailable
    case idle
    case connecting
    case authenticating
    case online
    case reconnecting
    case degraded
    case revoked
}

struct ControllerRemoteRouteState: Codable, Equatable, Sendable {
    let route: ControllerRemoteRouteKind
    var phase: ControllerRemoteRoutePhase
    var writerHeld: Bool

    private enum CodingKeys: String, CodingKey {
        case route, phase
        case writerHeld = "writer_held"
    }
}

enum ControllerRemoteRouteEvent: Equatable, Sendable {
    case enable(available: Bool)
    case connect
    case transportReady
    case authenticated
    case failure(retryable: Bool, mutationInFlight: Bool)
    case availabilityLost
    case authorizationRestored(available: Bool)
    case retry
    case cancel
    case revoke
    case disable
}

enum ControllerRemoteMutationDisposition: String, Codable, Sendable {
    case none
    case maySend = "may_send"
    case doNotReplay = "do_not_replay"
    case queryByCommandID = "query_by_command_id"
}

enum ControllerRemoteMutationCompletion: String, Codable, Sendable {
    case notSent = "not_sent"
    case acknowledged
    case unknown
    case rejected
}

struct ControllerRemoteRouteTransition: Codable, Equatable, Sendable {
    let state: ControllerRemoteRouteState
    let terminalAllowed: Bool
    let disconnectTransport: Bool
    let clearPendingInput: Bool
    let releaseWriter: Bool
    let retryIdempotentReads: Bool
    let mutationDisposition: ControllerRemoteMutationDisposition
    let requiresExplicitAction: Bool

    private enum CodingKeys: String, CodingKey {
        case state
        case terminalAllowed = "terminal_allowed"
        case disconnectTransport = "disconnect_transport"
        case clearPendingInput = "clear_pending_input"
        case releaseWriter = "release_writer"
        case retryIdempotentReads = "retry_idempotent_reads"
        case mutationDisposition = "mutation_disposition"
        case requiresExplicitAction = "requires_explicit_action"
    }
}

struct ControllerRemoteSwitchDecision: Codable, Equatable, Sendable {
    let from: ControllerRemoteRouteKind
    let to: ControllerRemoteRouteKind
    let disconnectSource: Bool
    let clearPendingInput: Bool
    let releaseWriter: Bool
    let automatic: Bool

    private enum CodingKeys: String, CodingKey {
        case from, to, automatic
        case disconnectSource = "disconnect_source"
        case clearPendingInput = "clear_pending_input"
        case releaseWriter = "release_writer"
    }
}

enum ControllerRemoteRouteTransitionError: String, Error, Equatable, Sendable {
    case invalidTransition = "InvalidTransition"
    case explicitConfirmationRequired = "ExplicitConfirmationRequired"
    case sameRoute = "SameRoute"
    case unsupportedPlatform = "UnsupportedPlatform"
    case targetUnavailable = "TargetUnavailable"
}

extension ControllerRemoteRouteState {
    func transition(
        _ event: ControllerRemoteRouteEvent
    ) throws -> ControllerRemoteRouteTransition {
        switch event {
        case .enable(let available) where phase == .disabled || phase == .unavailable:
            return neutral(phase: available ? .idle : .unavailable)
        case .connect where phase == .idle || phase == .degraded:
            return neutral(phase: .connecting)
        case .transportReady where phase == .connecting || phase == .reconnecting:
            return neutral(phase: .authenticating)
        case .authenticated where phase == .authenticating:
            return neutral(phase: .online, writerHeld: writerHeld)
        case .failure(let retryable, let mutationInFlight)
            where [.connecting, .authenticating, .online, .reconnecting].contains(phase):
            return ControllerRemoteRouteTransition(
                state: replacing(phase: retryable ? .reconnecting : .degraded),
                terminalAllowed: false,
                disconnectTransport: true,
                clearPendingInput: true,
                releaseWriter: writerHeld,
                retryIdempotentReads: retryable,
                mutationDisposition: mutationInFlight ? .queryByCommandID : .none,
                requiresExplicitAction: !retryable
            )
        case .availabilityLost where phase != .disabled && phase != .revoked:
            return cleanup(phase: .unavailable, explicit: true)
        case .authorizationRestored(let available) where phase == .revoked:
            return neutral(phase: available ? .idle : .unavailable)
        case .retry where phase == .degraded:
            return neutral(phase: .connecting)
        case .cancel
            where [.connecting, .authenticating, .online, .reconnecting, .degraded]
                .contains(phase):
            return cleanup(phase: .idle, explicit: false)
        case .revoke:
            return cleanup(phase: .revoked, explicit: true)
        case .disable:
            return cleanup(phase: .disabled, explicit: true)
        default:
            throw ControllerRemoteRouteTransitionError.invalidTransition
        }
    }

    func switchTo(
        _ target: ControllerRemoteRouteKind,
        platform: ControllerRemotePlatform,
        targetAvailable: Bool,
        explicitlyConfirmed: Bool
    ) throws -> ControllerRemoteSwitchDecision {
        guard explicitlyConfirmed else {
            throw ControllerRemoteRouteTransitionError.explicitConfirmationRequired
        }
        guard route != target else { throw ControllerRemoteRouteTransitionError.sameRoute }
        guard ControllerRemoteRoutePolicy.canonical(target).supports(platform) else {
            throw ControllerRemoteRouteTransitionError.unsupportedPlatform
        }
        guard targetAvailable else {
            throw ControllerRemoteRouteTransitionError.targetUnavailable
        }
        return ControllerRemoteSwitchDecision(
            from: route,
            to: target,
            disconnectSource: transportIsActive,
            clearPendingInput: true,
            releaseWriter: writerHeld,
            automatic: false
        )
    }

    func mutationDisposition(
        completion: ControllerRemoteMutationCompletion
    ) -> ControllerRemoteMutationDisposition {
        switch completion {
        case .notSent where phase == .online:
            return .maySend
        case .notSent, .acknowledged, .rejected:
            return .doNotReplay
        case .unknown:
            return .queryByCommandID
        }
    }

    private func neutral(
        phase: ControllerRemoteRoutePhase,
        writerHeld: Bool = false
    ) -> ControllerRemoteRouteTransition {
        let state = replacing(phase: phase, writerHeld: writerHeld)
        return ControllerRemoteRouteTransition(
            state: state,
            terminalAllowed: state.phase == .online,
            disconnectTransport: false,
            clearPendingInput: false,
            releaseWriter: false,
            retryIdempotentReads: false,
            mutationDisposition: .none,
            requiresExplicitAction: false
        )
    }

    private func cleanup(
        phase: ControllerRemoteRoutePhase,
        explicit: Bool
    ) -> ControllerRemoteRouteTransition {
        ControllerRemoteRouteTransition(
            state: replacing(phase: phase),
            terminalAllowed: false,
            disconnectTransport: transportIsActive,
            clearPendingInput: true,
            releaseWriter: writerHeld,
            retryIdempotentReads: false,
            mutationDisposition: .none,
            requiresExplicitAction: explicit
        )
    }

    private func replacing(
        phase: ControllerRemoteRoutePhase,
        writerHeld: Bool = false
    ) -> ControllerRemoteRouteState {
        ControllerRemoteRouteState(route: route, phase: phase, writerHeld: writerHeld)
    }

    private var transportIsActive: Bool {
        [.connecting, .authenticating, .online, .reconnecting].contains(phase)
    }
}
