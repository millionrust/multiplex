import Foundation

enum MobileTerminalRoute: String, Sendable {
    case directSSH = "direct_ssh"
    case deviceSession = "device_session"
}

enum MobileRouteEvent: String, Sendable {
    case connect
    case failure
    case cancel
    case background
    case reconnect
    case routeSwitch = "route_switch"
    case hostKeyMismatch = "host_key_mismatch"
    case missingTmux = "missing_tmux"
    case authorityRevoked = "authority_revoked"
}

enum MobileContinuityMode: String, Codable, Sendable {
    case none
    case normalShell = "normal_shell"
    case remoteTmux = "remote_tmux"
    case hostService = "host_service"
}

enum MobileAcceptanceStatus: String, Codable, Sendable {
    case connected
    case fallbackShell = "fallback_shell"
    case hostKeyBlocked = "host_key_blocked"
    case cancelled
    case backgrounded
    case hostOffline = "host_offline"
    case authorityRevoked = "authority_revoked"
    case routeSwitched = "route_switched"
}

enum MobileCrossRouteAcceptanceError: Error, Equatable, Sendable {
    case unsupportedEvent
}

struct MobileCrossRouteDecision: Codable, Equatable, Sendable {
    let terminalAllowed: Bool
    let continuityMode: MobileContinuityMode
    let fallbackToNormalShell: Bool
    let cancelConnection: Bool
    let disconnectTransport: Bool
    let coverPrivacy: Bool
    let clearPendingInput: Bool
    let releaseWriter: Bool
    let retainTerminalOutput: Bool
    let replayTerminalOutput: Bool
    let replayTerminalInput: Bool
    let nextDestination: String?
    let status: MobileAcceptanceStatus

    enum CodingKeys: String, CodingKey {
        case terminalAllowed = "terminal_allowed"
        case continuityMode = "continuity_mode"
        case fallbackToNormalShell = "fallback_to_normal_shell"
        case cancelConnection = "cancel_connection"
        case disconnectTransport = "disconnect_transport"
        case coverPrivacy = "cover_privacy"
        case clearPendingInput = "clear_pending_input"
        case releaseWriter = "release_writer"
        case retainTerminalOutput = "retain_terminal_output"
        case replayTerminalOutput = "replay_terminal_output"
        case replayTerminalInput = "replay_terminal_input"
        case nextDestination = "next_destination"
        case status
    }
}

enum MobileCrossRouteAcceptance {
    static func decide(
        route: MobileTerminalRoute,
        event: MobileRouteEvent,
        tmuxEnabled: Bool,
        tmuxAvailable: Bool,
        hostKeyMatches: Bool,
        authorityValid: Bool,
        writerHeld: Bool,
        pendingInput _: Bool
    ) throws -> MobileCrossRouteDecision {
        if event == .hostKeyMismatch || event == .missingTmux {
            guard route == .directSSH else { throw MobileCrossRouteAcceptanceError.unsupportedEvent }
        }
        if event == .authorityRevoked {
            guard route == .deviceSession else { throw MobileCrossRouteAcceptanceError.unsupportedEvent }
        }

        let continuity = continuityMode(
            route: route,
            tmuxEnabled: tmuxEnabled,
            tmuxAvailable: tmuxAvailable
        )
        switch event {
        case .connect, .reconnect:
            if route == .directSSH && !hostKeyMatches {
                return blockedHostKey()
            }
            if route == .deviceSession && !authorityValid {
                return revoked(writerHeld: writerHeld)
            }
            return MobileCrossRouteDecision(
                terminalAllowed: true,
                continuityMode: continuity,
                fallbackToNormalShell: false,
                cancelConnection: false,
                disconnectTransport: false,
                coverPrivacy: false,
                clearPendingInput: false,
                releaseWriter: false,
                retainTerminalOutput: true,
                replayTerminalOutput: event == .reconnect && route == .deviceSession,
                replayTerminalInput: false,
                nextDestination: nil,
                status: .connected
            )
        case .missingTmux:
            return MobileCrossRouteDecision(
                terminalAllowed: true,
                continuityMode: .normalShell,
                fallbackToNormalShell: true,
                cancelConnection: false,
                disconnectTransport: false,
                coverPrivacy: false,
                clearPendingInput: false,
                releaseWriter: false,
                retainTerminalOutput: true,
                replayTerminalOutput: false,
                replayTerminalInput: false,
                nextDestination: nil,
                status: .fallbackShell
            )
        case .hostKeyMismatch:
            return blockedHostKey()
        case .authorityRevoked:
            return revoked(writerHeld: writerHeld)
        case .failure:
            return inactive(
                continuity: continuity,
                coverPrivacy: false,
                releaseWriter: false,
                retainOutput: true,
                nextDestination: nil,
                status: .hostOffline
            )
        case .cancel:
            return inactive(
                continuity: route == .directSSH ? .none : .hostService,
                coverPrivacy: false,
                releaseWriter: false,
                retainOutput: true,
                nextDestination: nil,
                status: .cancelled
            )
        case .background:
            return inactive(
                continuity: continuity,
                coverPrivacy: true,
                releaseWriter: route == .deviceSession && writerHeld,
                retainOutput: true,
                nextDestination: nil,
                status: .backgrounded
            )
        case .routeSwitch:
            return inactive(
                continuity: continuity,
                coverPrivacy: true,
                releaseWriter: route == .deviceSession && writerHeld,
                retainOutput: true,
                nextDestination: route == .directSSH ? "devices" : "connections",
                status: .routeSwitched
            )
        }
    }

    private static func continuityMode(
        route: MobileTerminalRoute,
        tmuxEnabled: Bool,
        tmuxAvailable: Bool
    ) -> MobileContinuityMode {
        switch route {
        case .deviceSession:
            return .hostService
        case .directSSH:
            return tmuxEnabled && tmuxAvailable ? .remoteTmux : .normalShell
        }
    }

    private static func inactive(
        continuity: MobileContinuityMode,
        coverPrivacy: Bool,
        releaseWriter: Bool,
        retainOutput: Bool,
        nextDestination: String?,
        status: MobileAcceptanceStatus
    ) -> MobileCrossRouteDecision {
        MobileCrossRouteDecision(
            terminalAllowed: false,
            continuityMode: continuity,
            fallbackToNormalShell: false,
            cancelConnection: true,
            disconnectTransport: true,
            coverPrivacy: coverPrivacy,
            clearPendingInput: true,
            releaseWriter: releaseWriter,
            retainTerminalOutput: retainOutput,
            replayTerminalOutput: false,
            replayTerminalInput: false,
            nextDestination: nextDestination,
            status: status
        )
    }

    private static func blockedHostKey() -> MobileCrossRouteDecision {
        MobileCrossRouteDecision(
            terminalAllowed: false,
            continuityMode: .none,
            fallbackToNormalShell: false,
            cancelConnection: true,
            disconnectTransport: true,
            coverPrivacy: false,
            clearPendingInput: true,
            releaseWriter: false,
            retainTerminalOutput: false,
            replayTerminalOutput: false,
            replayTerminalInput: false,
            nextDestination: nil,
            status: .hostKeyBlocked
        )
    }

    private static func revoked(writerHeld: Bool) -> MobileCrossRouteDecision {
        MobileCrossRouteDecision(
            terminalAllowed: false,
            continuityMode: .hostService,
            fallbackToNormalShell: false,
            cancelConnection: true,
            disconnectTransport: true,
            coverPrivacy: true,
            clearPendingInput: true,
            releaseWriter: writerHeld,
            retainTerminalOutput: false,
            replayTerminalOutput: false,
            replayTerminalInput: false,
            nextDestination: nil,
            status: .authorityRevoked
        )
    }
}
