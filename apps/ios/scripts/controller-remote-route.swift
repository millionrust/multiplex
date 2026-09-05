import Foundation

private struct Fixture: Decodable {
    let schemaVersion: Int
    let routes: [ControllerRemoteRoutePolicy]
    let transitionCases: [TransitionCase]
    let invalidTransitionCases: [TransitionCase]
    let mutationCases: [MutationCase]
    let switchCases: [SwitchCase]

    private enum CodingKeys: String, CodingKey {
        case routes
        case schemaVersion = "schema_version"
        case transitionCases = "transition_cases"
        case invalidTransitionCases = "invalid_transition_cases"
        case mutationCases = "mutation_cases"
        case switchCases = "switch_cases"
    }
}

private struct TransitionCase: Decodable {
    let name: String
    let initial: ControllerRemoteRouteState
    let event: EventFixture
    let expected: ControllerRemoteRouteTransition?
}

private struct EventFixture: Decodable {
    let kind: String
    let available: Bool?
    let retryable: Bool?
    let mutationInFlight: Bool?

    private enum CodingKeys: String, CodingKey {
        case kind, available, retryable
        case mutationInFlight = "mutation_in_flight"
    }

    func value() throws -> ControllerRemoteRouteEvent {
        switch kind {
        case "enable": .enable(available: try required(available))
        case "connect": .connect
        case "transport_ready": .transportReady
        case "authenticated": .authenticated
        case "failure": .failure(
            retryable: try required(retryable),
            mutationInFlight: try required(mutationInFlight)
        )
        case "availability_lost": .availabilityLost
        case "authorization_restored": .authorizationRestored(
            available: try required(available)
        )
        case "retry": .retry
        case "cancel": .cancel
        case "revoke": .revoke
        case "disable": .disable
        default: throw RunnerError.invalidFixture("unsupported event \(kind)")
        }
    }
}

private struct MutationCase: Decodable {
    let state: ControllerRemoteRouteState
    let completion: ControllerRemoteMutationCompletion
    let expected: ControllerRemoteMutationDisposition
}

private struct SwitchCase: Decodable {
    let name: String
    let initial: ControllerRemoteRouteState
    let target: ControllerRemoteRouteKind
    let platform: ControllerRemotePlatform
    let targetAvailable: Bool
    let confirmed: Bool
    let expected: ControllerRemoteSwitchDecision?
    let expectedError: String?

    private enum CodingKeys: String, CodingKey {
        case name, initial, target, platform, confirmed, expected
        case targetAvailable = "target_available"
        case expectedError = "expected_error"
    }
}

private enum RunnerError: Error, CustomStringConvertible {
    case invalidFixture(String)
    case mismatch(String)

    var description: String {
        switch self {
        case .invalidFixture(let message): "invalid fixture: \(message)"
        case .mismatch(let message): message
        }
    }
}

@main
private enum ControllerRemoteRouteRunner {
    static func main() throws {
        guard CommandLine.arguments.count == 2 else {
            throw RunnerError.invalidFixture("expected one fixture path")
        }
        let data = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
        let fixture = try JSONDecoder().decode(Fixture.self, from: data)
        try check(fixture.schemaVersion == 1, "unexpected schema version")
        try check(fixture.routes.count == 4, "expected four route policies")

        for policy in fixture.routes {
            try check(
                policy == ControllerRemoteRoutePolicy.canonical(policy.kind),
                "policy mismatch for \(policy.kind.rawValue)"
            )
        }
        for item in fixture.transitionCases {
            let actual = try item.initial.transition(item.event.value())
            try check(actual == item.expected, "transition mismatch: \(item.name)")
            try check(actual.mutationDisposition != .maySend, "transition may send mutation")
        }
        for item in fixture.invalidTransitionCases {
            do {
                _ = try item.initial.transition(item.event.value())
                throw RunnerError.mismatch("invalid transition passed: \(item.name)")
            } catch is ControllerRemoteRouteTransitionError {}
        }
        for item in fixture.mutationCases {
            try check(
                item.state.mutationDisposition(completion: item.completion) == item.expected,
                "mutation mismatch for \(item.state.route.rawValue)"
            )
        }
        for item in fixture.switchCases {
            do {
                let actual = try item.initial.switchTo(
                    item.target,
                    platform: item.platform,
                    targetAvailable: item.targetAvailable,
                    explicitlyConfirmed: item.confirmed
                )
                try check(item.expectedError == nil, "switch should fail: \(item.name)")
                try check(actual == item.expected, "switch mismatch: \(item.name)")
            } catch let error as ControllerRemoteRouteTransitionError {
                try check(item.expected == nil, "switch unexpectedly failed: \(item.name)")
                try check(error.rawValue == item.expectedError, "switch error mismatch: \(item.name)")
            }
        }
        try checkAppleCoordinatorMatrix()
        print("Swift Controller remote route contract passed canonical and Apple lifecycle cases.")
    }

    private static func checkAppleCoordinatorMatrix() throws {
        let availability = AppleControllerRouteAvailability(
            privateNetwork: true,
            ssh: true,
            selfHostedRelay: true
        )
        for route in ControllerRemoteRouteKind.appleCases {
            var coordinator = AppleControllerRouteCoordinator(availability: availability)
            _ = try coordinator.select(route, explicitlyConfirmed: true)
            let start = try coordinator.connectSelected()
            try check(start.startTransport == route, "start mismatch")
            _ = try coordinator.transportReady(route)
            _ = try coordinator.authenticated(route)
            try coordinator.setWriterHeld(true)
            let reconnect = try coordinator.failed(
                route,
                retryable: true,
                mutationInFlight: true
            )
            try check(reconnect.releaseWriter, "writer was not released")
            try check(reconnect.mutationDisposition == .queryByCommandID, "mutation replay risk")
            _ = try coordinator.transportReady(route)
            _ = try coordinator.authenticated(route)
            _ = try coordinator.revokeSelected()
            try check(
                coordinator.projections.first(where: { $0.route == route })?.phase == .revoked,
                "revocation was not visible"
            )
        }

        for source in ControllerRemoteRouteKind.appleCases {
            for target in ControllerRemoteRouteKind.appleCases where source != target {
                var coordinator = AppleControllerRouteCoordinator(availability: availability)
                _ = try coordinator.select(source, explicitlyConfirmed: true)
                _ = try coordinator.connectSelected()
                _ = try coordinator.transportReady(source)
                _ = try coordinator.authenticated(source)
                do {
                    _ = try coordinator.select(target, explicitlyConfirmed: false)
                    throw RunnerError.mismatch("unconfirmed switch passed")
                } catch AppleControllerRouteCoordinatorError.transition(
                    .explicitConfirmationRequired
                ) {}
                let plan = try coordinator.select(target, explicitlyConfirmed: true)
                try check(plan.disconnectTransport == source, "source was not disconnected")
                try check(plan.startTransport == nil, "switch silently started target")
            }
        }
    }
}

private func required<T>(_ value: T?) throws -> T {
    guard let value else { throw RunnerError.invalidFixture("missing event field") }
    return value
}

private func check(_ condition: @autoclosure () -> Bool, _ message: String) throws {
    guard condition() else { throw RunnerError.mismatch(message) }
}
