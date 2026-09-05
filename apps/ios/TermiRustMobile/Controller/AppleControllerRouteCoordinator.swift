import Foundation

struct AppleControllerRouteAvailability: Equatable, Sendable {
    var privateNetwork: Bool
    var ssh: Bool
    var selfHostedRelay: Bool

    func isAvailable(_ route: ControllerRemoteRouteKind) -> Bool {
        switch route {
        case .localIPC: false
        case .privateNetwork: privateNetwork
        case .ssh: ssh
        case .selfHostedRelay: selfHostedRelay
        }
    }

    mutating func set(_ route: ControllerRemoteRouteKind, available: Bool) {
        switch route {
        case .localIPC: break
        case .privateNetwork: privateNetwork = available
        case .ssh: ssh = available
        case .selfHostedRelay: selfHostedRelay = available
        }
    }
}

enum AppleControllerRouteRecovery: String, Equatable, Sendable {
    case none
    case enable
    case configure
    case retry
    case reauthorize
    case cancel
}

struct AppleControllerRouteProjection: Equatable, Identifiable, Sendable {
    var id: ControllerRemoteRouteKind { route }

    let route: ControllerRemoteRouteKind
    let selected: Bool
    let available: Bool
    let phase: ControllerRemoteRoutePhase
    let terminalAllowed: Bool
    let trustLayers: [ControllerRemoteTrustLayer]
    let capabilities: [ControllerRemoteCapability]
    let recovery: AppleControllerRouteRecovery
}

struct AppleControllerRoutePlan: Equatable, Sendable {
    var startTransport: ControllerRemoteRouteKind?
    var disconnectTransport: ControllerRemoteRouteKind?
    var clearPendingInput = false
    var releaseWriter = false
    var retryIdempotentReads = false
    var mutationDisposition: ControllerRemoteMutationDisposition?
    var requiresExplicitAction = false

    static let none = Self()
}

enum AppleControllerRouteCoordinatorError: Error, Equatable, Sendable {
    case noSelectedRoute
    case routeNotSelected
    case writerRequiresOnlineRoute
    case transition(ControllerRemoteRouteTransitionError)
}

struct AppleControllerRouteCoordinator: Sendable {
    private var availability: AppleControllerRouteAvailability
    private var states: [ControllerRemoteRouteKind: ControllerRemoteRouteState]
    private(set) var selected: ControllerRemoteRouteKind?

    init(availability: AppleControllerRouteAvailability) {
        self.availability = availability
        self.states = Dictionary(uniqueKeysWithValues: ControllerRemoteRouteKind.appleCases.map {
            ($0, ControllerRemoteRouteState(
                route: $0,
                phase: availability.isAvailable($0) ? .idle : .unavailable,
                writerHeld: false
            ))
        })
    }

    var projections: [AppleControllerRouteProjection] {
        ControllerRemoteRouteKind.appleCases.map { route in
            let state = state(route)
            let policy = ControllerRemoteRoutePolicy.canonical(route)
            return AppleControllerRouteProjection(
                route: route,
                selected: selected == route,
                available: availability.isAvailable(route),
                phase: state.phase,
                terminalAllowed: selected == route && state.phase == .online,
                trustLayers: policy.trustLayers,
                capabilities: policy.capabilities,
                recovery: Self.recovery(for: state.phase)
            )
        }
    }

    mutating func restorePersistedSelection(
        _ route: ControllerRemoteRouteKind
    ) throws {
        guard route != .localIPC,
              ControllerRemoteRoutePolicy.canonical(route).supports(.appleMobile) else {
            throw AppleControllerRouteCoordinatorError.transition(.unsupportedPlatform)
        }
        selected = route
    }

    mutating func select(
        _ target: ControllerRemoteRouteKind,
        explicitlyConfirmed: Bool
    ) throws -> AppleControllerRoutePlan {
        guard target != .localIPC else {
            throw AppleControllerRouteCoordinatorError.transition(.unsupportedPlatform)
        }
        if let selected {
            let source = state(selected)
            let targetState = state(target)
            let decision: ControllerRemoteSwitchDecision
            do {
                decision = try source.switchTo(
                    target,
                    platform: .appleMobile,
                    targetAvailable: availability.isAvailable(target),
                    explicitlyConfirmed: explicitlyConfirmed
                )
            } catch let error as ControllerRemoteRouteTransitionError {
                throw AppleControllerRouteCoordinatorError.transition(error)
            }
            replace(ControllerRemoteRouteState(
                route: selected,
                phase: inactivePhase(source.phase, available: availability.isAvailable(selected)),
                writerHeld: false
            ))
            replace(ControllerRemoteRouteState(
                route: target,
                phase: selectablePhase(targetState.phase),
                writerHeld: false
            ))
            self.selected = target
            return AppleControllerRoutePlan(
                disconnectTransport: decision.disconnectSource ? selected : nil,
                clearPendingInput: decision.clearPendingInput,
                releaseWriter: decision.releaseWriter
            )
        }

        guard explicitlyConfirmed else {
            throw AppleControllerRouteCoordinatorError.transition(.explicitConfirmationRequired)
        }
        guard ControllerRemoteRoutePolicy.canonical(target).supports(.appleMobile) else {
            throw AppleControllerRouteCoordinatorError.transition(.unsupportedPlatform)
        }
        guard availability.isAvailable(target) else {
            throw AppleControllerRouteCoordinatorError.transition(.targetUnavailable)
        }
        selected = target
        return .none
    }

    mutating func connectSelected() throws -> AppleControllerRoutePlan {
        let route = try selectedRoute()
        let transition = try apply(route, event: .connect)
        var plan = plan(route: route, transition: transition)
        plan.startTransport = route
        return plan
    }

    mutating func transportReady(
        _ route: ControllerRemoteRouteKind
    ) throws -> AppleControllerRoutePlan {
        try applySelected(route, event: .transportReady)
    }

    mutating func authenticated(
        _ route: ControllerRemoteRouteKind
    ) throws -> AppleControllerRoutePlan {
        try applySelected(route, event: .authenticated)
    }

    mutating func failed(
        _ route: ControllerRemoteRouteKind,
        retryable: Bool,
        mutationInFlight: Bool
    ) throws -> AppleControllerRoutePlan {
        try applySelected(
            route,
            event: .failure(retryable: retryable, mutationInFlight: mutationInFlight)
        )
    }

    mutating func retrySelected() throws -> AppleControllerRoutePlan {
        let route = try selectedRoute()
        let transition = try apply(route, event: .retry)
        var plan = plan(route: route, transition: transition)
        plan.startTransport = route
        return plan
    }

    mutating func cancelSelected() throws -> AppleControllerRoutePlan {
        let route = try selectedRoute()
        return plan(route: route, transition: try apply(route, event: .cancel))
    }

    mutating func revokeSelected() throws -> AppleControllerRoutePlan {
        let route = try selectedRoute()
        return plan(route: route, transition: try apply(route, event: .revoke))
    }

    mutating func disableSelected() throws -> AppleControllerRoutePlan {
        let route = try selectedRoute()
        return plan(route: route, transition: try apply(route, event: .disable))
    }

    mutating func enableSelected() throws -> AppleControllerRoutePlan {
        let route = try selectedRoute()
        return plan(
            route: route,
            transition: try apply(
                route,
                event: .enable(available: availability.isAvailable(route))
            )
        )
    }

    mutating func authorizationRestored(
        _ route: ControllerRemoteRouteKind
    ) throws -> AppleControllerRoutePlan {
        guard selected == route else { throw AppleControllerRouteCoordinatorError.routeNotSelected }
        return plan(
            route: route,
            transition: try apply(
                route,
                event: .authorizationRestored(available: availability.isAvailable(route))
            )
        )
    }

    mutating func setWriterHeld(_ held: Bool) throws {
        let route = try selectedRoute()
        var current = state(route)
        guard !held || current.phase == .online else {
            throw AppleControllerRouteCoordinatorError.writerRequiresOnlineRoute
        }
        current.writerHeld = held
        replace(current)
    }

    mutating func setAvailable(
        _ route: ControllerRemoteRouteKind,
        available: Bool
    ) throws -> AppleControllerRoutePlan? {
        guard route != .localIPC else { return nil }
        guard availability.isAvailable(route) != available else { return nil }
        availability.set(route, available: available)
        let current = state(route)
        if !available {
            if current.phase == .disabled || current.phase == .revoked { return nil }
            if selected == route {
                return plan(
                    route: route,
                    transition: try apply(route, event: .availabilityLost)
                )
            }
            replace(ControllerRemoteRouteState(
                route: route,
                phase: .unavailable,
                writerHeld: false
            ))
        } else if current.phase == .unavailable {
            replace(try current.transition(.enable(available: true)).state)
        }
        return nil
    }

    private func selectedRoute() throws -> ControllerRemoteRouteKind {
        guard let selected else { throw AppleControllerRouteCoordinatorError.noSelectedRoute }
        return selected
    }

    private func state(_ route: ControllerRemoteRouteKind) -> ControllerRemoteRouteState {
        precondition(route != .localIPC)
        return states[route]!
    }

    private mutating func replace(_ state: ControllerRemoteRouteState) {
        states[state.route] = state
    }

    private mutating func applySelected(
        _ route: ControllerRemoteRouteKind,
        event: ControllerRemoteRouteEvent
    ) throws -> AppleControllerRoutePlan {
        guard selected == route else { throw AppleControllerRouteCoordinatorError.routeNotSelected }
        return plan(route: route, transition: try apply(route, event: event))
    }

    private mutating func apply(
        _ route: ControllerRemoteRouteKind,
        event: ControllerRemoteRouteEvent
    ) throws -> ControllerRemoteRouteTransition {
        do {
            let transition = try state(route).transition(event)
            replace(transition.state)
            return transition
        } catch let error as ControllerRemoteRouteTransitionError {
            throw AppleControllerRouteCoordinatorError.transition(error)
        }
    }

    private func plan(
        route: ControllerRemoteRouteKind,
        transition: ControllerRemoteRouteTransition
    ) -> AppleControllerRoutePlan {
        AppleControllerRoutePlan(
            disconnectTransport: transition.disconnectTransport ? route : nil,
            clearPendingInput: transition.clearPendingInput,
            releaseWriter: transition.releaseWriter,
            retryIdempotentReads: transition.retryIdempotentReads,
            mutationDisposition: transition.mutationDisposition == .none
                ? nil : transition.mutationDisposition,
            requiresExplicitAction: transition.requiresExplicitAction
        )
    }

    private func inactivePhase(
        _ phase: ControllerRemoteRoutePhase,
        available: Bool
    ) -> ControllerRemoteRoutePhase {
        switch phase {
        case .revoked: .revoked
        case .disabled: .disabled
        case .unavailable: .unavailable
        default: available ? .idle : .unavailable
        }
    }

    private func selectablePhase(
        _ phase: ControllerRemoteRoutePhase
    ) -> ControllerRemoteRoutePhase {
        switch phase {
        case .revoked: .revoked
        case .disabled: .disabled
        case .unavailable: .unavailable
        default: .idle
        }
    }

    private static func recovery(
        for phase: ControllerRemoteRoutePhase
    ) -> AppleControllerRouteRecovery {
        switch phase {
        case .disabled: .enable
        case .unavailable: .configure
        case .degraded: .retry
        case .revoked: .reauthorize
        case .connecting, .authenticating, .reconnecting: .cancel
        case .idle, .online: .none
        }
    }
}
