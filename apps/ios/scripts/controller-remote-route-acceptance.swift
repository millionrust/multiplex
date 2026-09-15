import Foundation

private struct Fixture: Decodable {
    let schemaVersion: Int
    let routes: [ControllerRemoteRouteKind]
    let lifecycleCases: [LifecycleCase]
    let switchMatrix: SwitchMatrix

    private enum CodingKeys: String, CodingKey {
        case routes
        case schemaVersion = "schema_version"
        case lifecycleCases = "lifecycle_cases"
        case switchMatrix = "switch_matrix"
    }
}

private struct LifecycleCase: Decodable {
    let name: String
    let steps: [Step]
    let expected: Metrics
}

private struct Step: Decodable {
    let kind: String
    let held: Bool?
    let retryable: Bool?
    let mutationInFlight: Bool?
    let available: Bool?

    private enum CodingKeys: String, CodingKey {
        case kind, held, retryable, available
        case mutationInFlight = "mutation_in_flight"
    }
}

private struct Metrics: Decodable, Equatable {
    var phase: ControllerRemoteRoutePhase?
    var transportStarts = 0
    var transportDisconnects = 0
    var inputClears = 0
    var writerReleases = 0
    var idempotentReadRetries = 0
    var mutationQueries = 0
    var mutationReplays = 0
    var automaticSwitches = 0
    var explicitActions = 0
    var terminalAllowed: Bool?

    private enum CodingKeys: String, CodingKey {
        case phase
        case transportStarts = "transport_starts"
        case transportDisconnects = "transport_disconnects"
        case inputClears = "input_clears"
        case writerReleases = "writer_releases"
        case idempotentReadRetries = "idempotent_read_retries"
        case mutationQueries = "mutation_queries"
        case mutationReplays = "mutation_replays"
        case automaticSwitches = "automatic_switches"
        case explicitActions = "explicit_actions"
        case terminalAllowed = "terminal_allowed"
    }

    mutating func observe(_ plan: AppleControllerRoutePlan) {
        transportStarts += plan.startTransport == nil ? 0 : 1
        transportDisconnects += plan.disconnectTransport == nil ? 0 : 1
        inputClears += plan.clearPendingInput ? 1 : 0
        writerReleases += plan.releaseWriter ? 1 : 0
        idempotentReadRetries += plan.retryIdempotentReads ? 1 : 0
        mutationQueries += plan.mutationDisposition == .queryByCommandID ? 1 : 0
        explicitActions += plan.requiresExplicitAction ? 1 : 0
    }
}

private struct SwitchMatrix: Decodable {
    let confirmed: ConfirmedSwitch
    let unconfirmedError: String
    let unavailableError: String

    private enum CodingKeys: String, CodingKey {
        case confirmed
        case unconfirmedError = "unconfirmed_error"
        case unavailableError = "unavailable_error"
    }
}

private struct ConfirmedSwitch: Decodable {
    let sourcePhase: ControllerRemoteRoutePhase
    let writerHeld: Bool
    let sourceDisconnects: Int
    let targetStarts: Int
    let inputClears: Int
    let writerReleases: Int
    let automaticSwitches: Int
    let targetPhase: ControllerRemoteRoutePhase

    private enum CodingKeys: String, CodingKey {
        case sourcePhase = "source_phase"
        case writerHeld = "writer_held"
        case sourceDisconnects = "source_disconnects"
        case targetStarts = "target_starts"
        case inputClears = "input_clears"
        case writerReleases = "writer_releases"
        case automaticSwitches = "automatic_switches"
        case targetPhase = "target_phase"
    }
}

private enum AcceptanceError: Error, CustomStringConvertible {
    case invalid(String)
    case mismatch(String)

    var description: String {
        switch self {
        case .invalid(let message), .mismatch(let message): message
        }
    }
}

@main
private enum ControllerRemoteRouteAcceptanceRunner {
    static func main() throws {
        guard CommandLine.arguments.count == 2 else {
            throw AcceptanceError.invalid("expected one acceptance fixture path")
        }
        let data = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
        let fixture = try JSONDecoder().decode(Fixture.self, from: data)
        try check(fixture.schemaVersion == 1, "unexpected acceptance schema")
        try check(fixture.routes == ControllerRemoteRouteKind.appleCases, "route list mismatch")

        for route in fixture.routes {
            for item in fixture.lifecycleCases {
                var coordinator = AppleControllerRouteCoordinator(availability: allAvailable())
                var actual = Metrics()
                for step in item.steps {
                    let plan: AppleControllerRoutePlan
                    switch step.kind {
                    case "select":
                        plan = try coordinator.select(route, explicitlyConfirmed: true)
                    case "connect":
                        plan = try coordinator.connectSelected()
                    case "transport_ready":
                        plan = try coordinator.transportReady(route)
                    case "authenticated":
                        plan = try coordinator.authenticated(route)
                    case "set_writer":
                        try coordinator.setWriterHeld(try required(step.held))
                        continue
                    case "failure":
                        plan = try coordinator.failed(
                            route,
                            retryable: try required(step.retryable),
                            mutationInFlight: try required(step.mutationInFlight)
                        )
                    case "retry":
                        plan = try coordinator.retrySelected()
                    case "cancel":
                        plan = try coordinator.cancelSelected()
                    case "revoke":
                        plan = try coordinator.revokeSelected()
                    case "set_available":
                        if let availabilityPlan = try coordinator.setAvailable(
                            route,
                            available: try required(step.available)
                        ) {
                            actual.observe(availabilityPlan)
                        }
                        continue
                    case "authorization_restored":
                        plan = try coordinator.authorizationRestored(route)
                    default:
                        throw AcceptanceError.invalid("unsupported step \(step.kind)")
                    }
                    actual.observe(plan)
                }
                let projection = try projection(route, coordinator)
                actual.phase = projection.phase
                actual.terminalAllowed = projection.terminalAllowed
                try check(actual == item.expected, "\(item.name) failed on \(route.rawValue)")
            }
        }

        try verifySwitchMatrix(fixture)
        print(
            "Controller route acceptance passed "
                + "\(fixture.lifecycleCases.count * fixture.routes.count) lifecycles and "
                + "\(fixture.routes.count * (fixture.routes.count - 1)) switch pairs."
        )
    }

    private static func verifySwitchMatrix(_ fixture: Fixture) throws {
        let expected = fixture.switchMatrix.confirmed
        try check(expected.sourcePhase == .online, "unexpected switch source phase")
        for source in fixture.routes {
            for target in fixture.routes where source != target {
                var coordinator = AppleControllerRouteCoordinator(availability: allAvailable())
                try connectOnline(source, &coordinator)
                try coordinator.setWriterHeld(expected.writerHeld)

                do {
                    _ = try coordinator.select(target, explicitlyConfirmed: false)
                    throw AcceptanceError.mismatch("unconfirmed switch passed")
                } catch let error as AppleControllerRouteCoordinatorError {
                    try check(errorCode(error) == fixture.switchMatrix.unconfirmedError, "wrong unconfirmed error")
                }
                try check(coordinator.selected == source, "unconfirmed switch changed selection")

                let plan = try coordinator.select(target, explicitlyConfirmed: true)
                try check((plan.disconnectTransport == source ? 1 : 0) == expected.sourceDisconnects, "source disconnect mismatch")
                try check((plan.startTransport == target ? 1 : 0) == expected.targetStarts, "target start mismatch")
                try check((plan.clearPendingInput ? 1 : 0) == expected.inputClears, "input cleanup mismatch")
                try check((plan.releaseWriter ? 1 : 0) == expected.writerReleases, "writer cleanup mismatch")
                try check(expected.automaticSwitches == 0, "fixture permits automatic switching")
                try check(try projection(target, coordinator).phase == expected.targetPhase, "target phase mismatch")

                var blocked = AppleControllerRouteCoordinator(
                    availability: availability(excluding: target)
                )
                try connectOnline(source, &blocked)
                do {
                    _ = try blocked.select(target, explicitlyConfirmed: true)
                    throw AcceptanceError.mismatch("unavailable switch passed")
                } catch let error as AppleControllerRouteCoordinatorError {
                    try check(errorCode(error) == fixture.switchMatrix.unavailableError, "wrong unavailable error")
                }
                try check(blocked.selected == source, "unavailable switch changed selection")
            }
        }
    }

    private static func connectOnline(
        _ route: ControllerRemoteRouteKind,
        _ coordinator: inout AppleControllerRouteCoordinator
    ) throws {
        _ = try coordinator.select(route, explicitlyConfirmed: true)
        _ = try coordinator.connectSelected()
        _ = try coordinator.transportReady(route)
        _ = try coordinator.authenticated(route)
    }

    private static func projection(
        _ route: ControllerRemoteRouteKind,
        _ coordinator: AppleControllerRouteCoordinator
    ) throws -> AppleControllerRouteProjection {
        guard let value = coordinator.projections.first(where: { $0.route == route }) else {
            throw AcceptanceError.invalid("missing route projection")
        }
        return value
    }

    private static func allAvailable() -> AppleControllerRouteAvailability {
        AppleControllerRouteAvailability(privateNetwork: true, ssh: true, selfHostedRelay: true)
    }

    private static func availability(
        excluding route: ControllerRemoteRouteKind
    ) -> AppleControllerRouteAvailability {
        AppleControllerRouteAvailability(
            privateNetwork: route != .privateNetwork,
            ssh: route != .ssh,
            selfHostedRelay: route != .selfHostedRelay
        )
    }

    private static func errorCode(_ error: AppleControllerRouteCoordinatorError) -> String {
        switch error {
        case .transition(.explicitConfirmationRequired): "explicit_confirmation_required"
        case .transition(.targetUnavailable): "target_unavailable"
        default: "unexpected"
        }
    }

    private static func required<T>(_ value: T?) throws -> T {
        guard let value else { throw AcceptanceError.invalid("missing step field") }
        return value
    }

    private static func check(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
        guard try condition() else { throw AcceptanceError.mismatch(message) }
    }
}
