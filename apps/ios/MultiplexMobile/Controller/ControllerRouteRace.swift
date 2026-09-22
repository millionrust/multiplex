import Foundation
@preconcurrency import Network

/// The address that connected first, and how every attempt that started ended.
struct ControllerRouteRaceWinner: Sendable {
    let network: any ControllerDuplexConnection
    let route: HostRoute
    let outcomes: [AttemptOutcome]
}

/// Every attempt failed. `underlying` is the error to report, as a single address would have.
struct ControllerRouteRaceFailure: Error {
    let underlying: any Error
    let outcomes: [AttemptOutcome]
}

/// Races a Host's addresses in the order and at the times the shared planner gives
/// (`planRoutes`, `docs/route-selection-plan.md`).
///
/// Each attempt starts at its `startAfterMillis`, or as soon as every attempt already started
/// has failed, whichever comes first. The first to connect wins. Every other attempt is
/// cancelled, and a connection that finishes opening after the winner is closed unused: nothing
/// is ever sent on a loser, so a lost race never counts as a failed login on the computer.
enum ControllerRouteRace {
    private enum Event: Sendable {
        /// A scheduled start time arrived.
        case due
        case deadline
        case opened(Int, any ControllerDuplexConnection)
        case failed(Int, any Error)
    }

    static func run(
        _ plan: RoutePlan,
        deadline: ContinuousClock.Instant? = nil,
        open: @escaping @Sendable (HostRoute) async throws -> any ControllerDuplexConnection
    ) async throws -> ControllerRouteRaceWinner {
        let attempts: [(route: HostRoute, planned: PlannedAttempt)] = plan.attempts.compactMap { planned in
            (try? HostRoute(address: planned.route.address, port: planned.route.port))
                .map { (route: $0, planned: planned) }
        }
        guard !attempts.isEmpty else {
            throw ControllerRouteRaceFailure(underlying: ControllerPairingError.connectionClosed, outcomes: [])
        }
        let clock = ContinuousClock()
        let started = clock.now
        var end = started + .milliseconds(Int(plan.deadlineMillis))
        if let deadline { end = min(end, deadline) }
        let raceEnd = end

        var results = [AttemptResult?](repeating: nil, count: attempts.count)
        var winner: (index: Int, network: any ControllerDuplexConnection)?
        var lastError: (any Error)?
        var deadlineReached = false

        await withTaskGroup(of: Event.self) { group in
            group.addTask {
                try? await clock.sleep(until: raceEnd)
                return .deadline
            }
            var next = 0
            var running = 0
            var timerPending = false
            race: while true {
                // Start whatever is due, and the next attempt early while nothing is running.
                while next < attempts.count,
                      running == 0
                        || clock.now - started >= .milliseconds(Int(attempts[next].planned.startAfterMillis)) {
                    let index = next
                    let route = attempts[index].route
                    let limit = min(
                        Duration.milliseconds(Int(attempts[index].planned.timeoutMillis)),
                        max(raceEnd - clock.now, .zero)
                    )
                    group.addTask { await attempt(index, route: route, limit: limit, open: open) }
                    next += 1
                    running += 1
                }
                if next < attempts.count, !timerPending {
                    let due = started + .milliseconds(Int(attempts[next].planned.startAfterMillis))
                    group.addTask {
                        try? await clock.sleep(until: due)
                        return .due
                    }
                    timerPending = true
                }
                guard let event = await group.next() else { break }
                switch event {
                case .due:
                    timerPending = false
                case .deadline:
                    deadlineReached = true
                    break race
                case .opened(let index, let network):
                    results[index] = .connected
                    winner = (index, network)
                    break race
                case .failed(let index, let error):
                    running -= 1
                    results[index] = result(for: error)
                    if results[index] != .cancelled { lastError = error }
                    if running == 0, next >= attempts.count { break race }
                }
            }
            group.cancelAll()
            // Whatever is still opening is cancelled; one that opened anyway is closed unused.
            while let event = await group.next() {
                switch event {
                case .opened(let index, let network):
                    network.cancel()
                    results[index] = .cancelled
                case .failed(let index, _):
                    results[index] = .cancelled
                case .due, .deadline:
                    break
                }
            }
        }

        let outcomes = attempts.indices.compactMap { index in
            results[index].map { result in
                AttemptOutcome(route: attempts[index].planned.route, result: result)
            }
        }
        if let winner {
            return ControllerRouteRaceWinner(
                network: winner.network,
                route: attempts[winner.index].route,
                outcomes: outcomes
            )
        }
        if Task.isCancelled { throw CancellationError() }
        let underlying: any Error = deadlineReached
            ? ControllerPairingError.timedOut
            : (lastError ?? ControllerPairingError.timedOut)
        throw ControllerRouteRaceFailure(underlying: underlying, outcomes: outcomes)
    }

    /// One attempt, bounded by `limit`. A connection that opens as the limit passes is closed.
    private static func attempt(
        _ index: Int,
        route: HostRoute,
        limit: Duration,
        open: @escaping @Sendable (HostRoute) async throws -> any ControllerDuplexConnection
    ) async -> Event {
        await withTaskGroup(of: Result<(any ControllerDuplexConnection)?, any Error>.self) { group in
            group.addTask {
                do {
                    return .success(try await open(route))
                } catch {
                    return .failure(error)
                }
            }
            group.addTask {
                do {
                    try await Task.sleep(for: limit)
                    return .success(nil)
                } catch {
                    return .failure(error)
                }
            }
            let first = await group.next() ?? .failure(CancellationError())
            group.cancelAll()
            for await late in group {
                if case .success(let network?) = late { network.cancel() }
            }
            switch first {
            case .success(let network?):
                return .opened(index, network)
            case .success(nil):
                return .failed(index, ControllerPairingError.timedOut)
            case .failure(let error):
                return .failed(index, error)
            }
        }
    }

    /// How an attempt ended, from the error its transport reported.
    static func result(for error: any Error) -> AttemptResult {
        if error is CancellationError { return .cancelled }
        if let error = error as? ControllerPairingError {
            switch error {
            case .timedOut: return .timedOut
            case .cancelled: return .cancelled
            default: return .failed
            }
        }
        let code: POSIXErrorCode?
        if let error = error as? NWError, case .posix(let posix) = error {
            code = posix
        } else if let error = error as? POSIXError {
            code = error.code
        } else {
            code = nil
        }
        switch code {
        case .ECONNREFUSED?: return .refused
        case .ENETUNREACH?, .EHOSTUNREACH?, .ENETDOWN?, .EHOSTDOWN?: return .unreachable
        case .ETIMEDOUT?: return .timedOut
        case .ECANCELED?: return .cancelled
        default: return .failed
        }
    }
}

/// What the phone tells the person after a race, beyond connected or not.
enum ControllerRouteAdvice: String, Equatable, Sendable {
    case localNetworkBlockedWhileTailscaleWorks
    case remoteAccessOff
    case notOnComputersNetwork
    case needsRemoteRoute

    /// `nil` when there is nothing more to say than the connection state already does.
    init?(_ advice: RouteAdvice) {
        switch advice {
        case .localNetworkBlockedWhileTailscaleWorks: self = .localNetworkBlockedWhileTailscaleWorks
        case .remoteAccessOff: self = .remoteAccessOff
        case .notOnComputersNetwork: self = .notOnComputersNetwork
        case .needsRemoteRoute: self = .needsRemoteRoute
        case .none, .computerUnreachable: return nil
        }
    }
}

extension RouteAddress {
    init(_ route: HostRoute) {
        self.init(address: route.address, port: route.port)
    }
}
