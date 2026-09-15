import Foundation
import SwiftUI

struct ControllerSessionGroupID: Hashable, Sendable {
    let project: String?
    let group: String?
}

struct ControllerSessionGroup: Identifiable, Equatable, Sendable {
    let id: ControllerSessionGroupID
    var sessions: [ControllerSessionSummary]
}

enum ControllerPresentation {
    static func isOpenTerminal(_ session: ControllerSessionSummary) -> Bool {
        let liveLifecycles = ["live", "running", "running_app_attached"]
        let canAttach = session.capabilities.isEmpty
            || session.capabilities.contains(.attachOutput)
        return liveLifecycles.contains(session.lifecycle)
            && session.occupantGeneration != nil
            && canAttach
    }

    static func openTerminals(_ sessions: [ControllerSessionSummary]) -> [ControllerSessionSummary] {
        sessions.filter(isOpenTerminal)
    }

    static func previousSessions(_ sessions: [ControllerSessionSummary]) -> [ControllerSessionSummary] {
        sessions.filter { !isOpenTerminal($0) }
    }

    static func routeTitle(_ route: ControllerRemoteRouteKind) -> String {
        switch route {
        case .localIPC: "Local IPC"
        case .privateNetwork: "Private network"
        case .ssh: "SSH Controller"
        case .selfHostedRelay: "Self-hosted relay"
        }
    }

    static func routeIcon(_ route: ControllerRemoteRouteKind) -> String {
        switch route {
        case .localIPC: "desktopcomputer"
        case .privateNetwork: "network"
        case .ssh: "key.horizontal"
        case .selfHostedRelay: "point.3.connected.trianglepath.dotted"
        }
    }

    static func routeStatus(_ route: AppleControllerRouteProjection) -> String {
        let status: String = switch route.phase {
        case .disabled: "Disabled"
        case .unavailable: "Not configured"
        case .idle: "Ready"
        case .connecting: "Connecting"
        case .authenticating: "Authenticating"
        case .online: "Online"
        case .reconnecting: "Reconnecting"
        case .degraded: "Needs attention"
        case .revoked: "Authorization revoked"
        }
        return route.selected ? "Selected · \(status)" : status
    }

    static func isolated(_ value: String) -> String {
        "\u{2068}\(value)\u{2069}"
    }

    static func fingerprintForSpeech(_ fingerprint: String) -> String {
        fingerprint.map(String.init).joined(separator: " ")
    }

    static func unreadDescription(_ count: UInt32) -> String {
        String.localizedStringWithFormat(
            NSLocalizedString("session.unread.count", comment: "Unread session activity count"),
            Int64(count)
        )
    }

    static func capabilityLabels(bits: UInt16) -> [LocalizedStringKey] {
        let known: [(UInt16, LocalizedStringKey)] = [
            (1 << 0, "View session list"),
            (1 << 1, "Attach to session output"),
            (1 << 2, "Send terminal input"),
            (1 << 3, "Resize terminal"),
            (1 << 4, "Respond to approvals"),
        ]
        return known.compactMap { bit, label in bits & bit == bit ? label : nil }
    }

    static func lifecycleLabel(_ lifecycle: String) -> LocalizedStringKey {
        switch lifecycle {
        case "draft": return "Draft"
        case "validating": return "Validating"
        case "starting": return "Starting"
        case "provisioning": return "Provisioning"
        case "attaching": return "Attaching"
        case "replaying": return "Replaying"
        case "live", "running", "running_app_attached": return "Live"
        case "recording_paused": return "Recording paused"
        case "stopping": return "Stopping"
        case "offline": return "Offline"
        case "orphaned": return "Orphaned"
        case "gap": return "Output gap"
        case "permission_denied": return "Permission denied"
        case "incompatible": return "Incompatible"
        case "failed": return "Failed"
        case "cancelled": return "Cancelled"
        case "exited", "stopped": return "Exited"
        default: return "Unknown"
        }
    }

    static func activityLabel(_ activity: String) -> LocalizedStringKey {
        switch activity {
        case "idle": return "Idle"
        case "busy": return "Busy"
        case "needs_input": return "Needs input"
        case "done": return "Done"
        case "failed": return "Failed"
        default: return "Activity unknown"
        }
    }

    static func originLabel(_ origin: ControllerSessionOrigin) -> LocalizedStringKey {
        switch origin {
        case .terminal: return "Terminal"
        case .managedAgent: return "Managed agent"
        case .observedAgent: return "Observed agent"
        case .unknown: return "Session"
        }
    }

    static func sessionGroups(_ sessions: [ControllerSessionSummary]) -> [ControllerSessionGroup] {
        var positions: [ControllerSessionGroupID: Int] = [:]
        var groups: [ControllerSessionGroup] = []
        for session in sessions {
            let id = ControllerSessionGroupID(project: session.project, group: session.group)
            if let index = positions[id] {
                groups[index].sessions.append(session)
            } else {
                positions[id] = groups.count
                groups.append(ControllerSessionGroup(id: id, sessions: [session]))
            }
        }
        return groups
    }

    static func sessionGroupTitle(_ id: ControllerSessionGroupID) -> String? {
        let names = [id.project, id.group].compactMap { $0 }
        guard !names.isEmpty else { return nil }
        return names.map(isolated).joined(separator: " · ")
    }
}
