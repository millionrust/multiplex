import Foundation
@preconcurrency import Network
import XCTest
@testable import MultiplexMobile

final class ControllerRouteRaceTests: XCTestCase {
    private let local = try! HostRoute(address: "192.168.1.20", port: 7_420)
    private let tailscale = try! HostRoute(address: "100.101.102.103", port: 7_420)

    // MARK: - The shared cases

    func testEveryRoutePlanCaseMatchesTheSharedPlanner() throws {
        let fixture = try RoutePlanFixture.load(bundle: Bundle(for: Self.self))
        XCTAssertEqual(fixture.schemaVersion, 1)
        XCTAssertGreaterThanOrEqual(fixture.cases.count, 9)
        for testCase in fixture.cases {
            let plan = planRoutes(
                saved: testCase.saved.map(\.address),
                discovered: testCase.discovered.map(\.address),
                network: PhoneNetwork(
                    link: try testCase.phoneLink(),
                    addresses: testCase.phoneAddresses.map {
                        PhoneAddress(address: $0.address, prefixLength: $0.prefixLength)
                    },
                    fingerprint: testCase.fingerprint
                ),
                remembered: testCase.remembered.map {
                    RememberedRoute(fingerprint: $0.fingerprint, route: $0.route.address)
                }
            )
            XCTAssertEqual(plan.deadlineMillis, testCase.expected.deadlineMillis, testCase.name)
            XCTAssertEqual(
                plan.attempts.map { RoutePlanFixture.Attempt($0) },
                testCase.expected.attempts,
                testCase.name
            )
        }
    }

    // MARK: - The race

    func testAHangingFirstRouteLosesToALaterOneAndIsCancelledUnused() async throws {
        let transport = FakeRouteTransport([
            local.address: .hang,
            tailscale.address: .open(after: .zero),
        ])
        let winner = try await ControllerRouteRace.run(
            plan([(local, 0, 3_000), (tailscale, 50, 3_000)]),
            open: transport.open
        )
        XCTAssertEqual(winner.route, tailscale)
        XCTAssertEqual(winner.outcomes.map(\.result), [.cancelled, .connected])
        XCTAssertEqual(transport.cancelledOpens, [local.address])
        XCTAssertEqual(transport.bytesSent, 0)
    }

    func testAConnectionThatOpensAfterTheWinnerIsClosedUnused() async throws {
        let transport = FakeRouteTransport([
            local.address: .openIgnoringCancellation(after: .milliseconds(200)),
            tailscale.address: .open(after: .zero),
        ])
        let winner = try await ControllerRouteRace.run(
            plan([(local, 0, 3_000), (tailscale, 20, 3_000)]),
            open: transport.open
        )
        XCTAssertEqual(winner.route, tailscale)
        XCTAssertEqual(transport.closedConnections, [local.address])
        XCTAssertEqual(transport.bytesSent, 0)
    }

    func testARefusedRouteLetsTheNextOneStartBeforeItsTime() async throws {
        let transport = FakeRouteTransport([
            local.address: .fail(NWError.posix(.ECONNREFUSED)),
            tailscale.address: .open(after: .zero),
        ])
        let clock = ContinuousClock()
        let started = clock.now
        let winner = try await ControllerRouteRace.run(
            plan([(local, 0, 3_000), (tailscale, 5_000, 3_000)], deadline: 10_000),
            open: transport.open
        )
        XCTAssertLessThan(clock.now - started, .seconds(2))
        XCTAssertEqual(winner.route, tailscale)
        XCTAssertEqual(winner.outcomes.map(\.result), [.refused, .connected])
    }

    func testWhenEveryRouteFailsTheRaceSaysHowEachEnded() async throws {
        let transport = FakeRouteTransport([
            local.address: .fail(NWError.posix(.ECONNREFUSED)),
            tailscale.address: .fail(NWError.posix(.EHOSTUNREACH)),
        ])
        do {
            _ = try await ControllerRouteRace.run(
                plan([(local, 0, 3_000), (tailscale, 5_000, 3_000)]),
                open: transport.open
            )
            XCTFail("every route failed")
        } catch let failure as ControllerRouteRaceFailure {
            XCTAssertEqual(failure.outcomes.map(\.result), [.refused, .unreachable])
            XCTAssertEqual(failure.underlying as? NWError, NWError.posix(.EHOSTUNREACH))
        }
    }

    func testTheRaceGivesUpAtItsDeadline() async throws {
        let transport = FakeRouteTransport([local.address: .hang, tailscale.address: .hang])
        let clock = ContinuousClock()
        let started = clock.now
        do {
            _ = try await ControllerRouteRace.run(
                plan([(local, 0, 5_000), (tailscale, 10, 5_000)], deadline: 200),
                open: transport.open
            )
            XCTFail("nothing answered")
        } catch let failure as ControllerRouteRaceFailure {
            XCTAssertLessThan(clock.now - started, .seconds(3))
            XCTAssertEqual(failure.underlying as? ControllerPairingError, .timedOut)
            XCTAssertEqual(Set(transport.cancelledOpens), [local.address, tailscale.address])
        }
    }

    func testTransportErrorsAreNamedForTheAdvice() {
        XCTAssertEqual(ControllerRouteRace.result(for: NWError.posix(.ECONNREFUSED)), .refused)
        XCTAssertEqual(ControllerRouteRace.result(for: NWError.posix(.ENETUNREACH)), .unreachable)
        XCTAssertEqual(ControllerRouteRace.result(for: POSIXError(.EHOSTUNREACH)), .unreachable)
        XCTAssertEqual(ControllerRouteRace.result(for: ControllerPairingError.timedOut), .timedOut)
        XCTAssertEqual(ControllerRouteRace.result(for: CancellationError()), .cancelled)
        XCTAssertEqual(ControllerRouteRace.result(for: ControllerPairingError.connectionClosed), .failed)
    }

    // MARK: - Remembering

    func testARecordSavedBeforeRoutesWereRememberedStillReads() throws {
        let record = try host()
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .millisecondsSince1970
        var object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: encoder.encode(record)) as? [String: Any]
        )
        XCTAssertNotNil(object.removeValue(forKey: "routeMemory"))
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .millisecondsSince1970
        let decoded = try decoder.decode(
            PairedHostRecord.self,
            from: JSONSerialization.data(withJSONObject: object)
        )
        XCTAssertEqual(decoded.routeMemory, [])
        XCTAssertEqual(decoded.routes, record.routes)
    }

    func testTheWinnerIsRememberedPerNetworkAndSurvivesSaving() throws {
        let home = String(repeating: "a", count: 32)
        let cafe = String(repeating: "b", count: 32)
        let record = try host()
            .remembering(local, on: home)
            .remembering(tailscale, on: cafe)
            .remembering(local, on: home)
        XCTAssertEqual(record.routeMemory.map(\.fingerprint), [home, cafe])
        XCTAssertEqual(record.routeMemory.first?.address, local.address)

        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .millisecondsSince1970
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .millisecondsSince1970
        let decoded = try decoder.decode(PairedHostRecord.self, from: encoder.encode(record))
        XCTAssertEqual(decoded, record)
        XCTAssertThrowsError(try RememberedRouteRecord(fingerprint: "home", route: local))
    }

    // MARK: - Helpers

    private func plan(
        _ attempts: [(HostRoute, UInt32, UInt32)],
        deadline: UInt32 = 5_000
    ) -> RoutePlan {
        RoutePlan(
            attempts: attempts.map { route, start, timeout in
                PlannedAttempt(
                    route: RouteAddress(route),
                    kind: routeKind(address: route.address),
                    tier: 1,
                    startAfterMillis: start,
                    timeoutMillis: timeout
                )
            },
            deadlineMillis: deadline
        )
    }

    private func host() throws -> PairedHostRecord {
        try PairedHostRecord(
            id: "host",
            displayName: "Mac",
            routes: [local, tailscale],
            hostStaticPublicKey: Data(repeating: 7, count: 32),
            deviceStaticKeyId: "device-key",
            deviceId: UUID(),
            identityGeneration: 1,
            revocationEpoch: 0,
            sessionGeneration: 0,
            capabilityBits: 1,
            pairedAt: Date(timeIntervalSince1970: 1_700_000_000)
        )
    }
}

/// Opens connections by address the way each test says, and records what happened to them.
private final class FakeRouteTransport: @unchecked Sendable {
    enum Behaviour: Sendable {
        case open(after: Duration)
        case openIgnoringCancellation(after: Duration)
        case hang
        case fail(any Error)
    }

    private let behaviours: [String: Behaviour]
    private let lock = NSLock()
    private var cancelled: [String] = []
    private var closed: [String] = []
    private var sent = 0

    init(_ behaviours: [String: Behaviour]) {
        self.behaviours = behaviours
    }

    var cancelledOpens: [String] { lock.withLock { cancelled } }
    var closedConnections: [String] { lock.withLock { closed } }
    var bytesSent: Int { lock.withLock { sent } }

    var open: @Sendable (HostRoute) async throws -> any ControllerDuplexConnection {
        { [self] route in try await self.connect(route) }
    }

    private func connect(_ route: HostRoute) async throws -> any ControllerDuplexConnection {
        switch behaviours[route.address] ?? .hang {
        case .open(let delay):
            try await Task.sleep(for: delay)
        case .openIgnoringCancellation(let delay):
            await Task.detached { try? await Task.sleep(for: delay) }.value
        case .hang:
            do {
                try await Task.sleep(for: .seconds(60))
            } catch {
                lock.withLock { cancelled.append(route.address) }
                throw error
            }
        case .fail(let error):
            throw error
        }
        return FakeRouteConnection(route: route, transport: self)
    }

    fileprivate func didClose(_ route: HostRoute) {
        lock.withLock { closed.append(route.address) }
    }

    fileprivate func didSend(_ count: Int) {
        lock.withLock { sent += count }
    }
}

private final class FakeRouteConnection: ControllerDuplexConnection, @unchecked Sendable {
    let route: HostRoute
    private let transport: FakeRouteTransport

    init(route: HostRoute, transport: FakeRouteTransport) {
        self.route = route
        self.transport = transport
    }

    var remoteRoute: HostRoute? { route }

    func send(_ data: Data) async throws {
        transport.didSend(data.count)
    }

    func receive(maximumLength: Int) async throws -> Data {
        throw ControllerPairingError.connectionClosed
    }

    func cancel() {
        transport.didClose(route)
    }
}

/// `tests/fixtures/controller-routes/route-plan-v1.json`, as
/// `crates/multiplex-controller-bindings/tests/route_plan_vectors.rs` writes it.
private struct RoutePlanFixture: Decodable {
    let schemaVersion: UInt16
    let cases: [Case]

    struct Route: Decodable {
        let address: String
        let port: UInt16

        var routeAddress: RouteAddress { RouteAddress(address: address, port: port) }
    }

    struct Own: Decodable {
        let address: String
        let prefixLength: UInt8
    }

    struct Attempt: Decodable, Equatable {
        let address: String
        let port: UInt16
        let kind: String
        let tier: UInt8
        let startAfterMillis: UInt32
        let timeoutMillis: UInt32

        init(_ attempt: PlannedAttempt) {
            address = attempt.route.address
            port = attempt.route.port
            switch attempt.kind {
            case .localNetwork: kind = "local_network"
            case .tailscale: kind = "tailscale"
            case .otherPrivate: kind = "other_private"
            }
            tier = attempt.tier
            startAfterMillis = attempt.startAfterMillis
            timeoutMillis = attempt.timeoutMillis
        }
    }

    struct Expected: Decodable {
        let deadlineMillis: UInt32
        let attempts: [Attempt]
    }

    struct Case: Decodable {
        let name: String
        let saved: [RouteWrapper]
        let discovered: [RouteWrapper]
        let link: String
        let phoneAddresses: [Own]
        let fingerprint: String?
        let remembered: [RememberedWrapper]
        let expected: Expected

        func phoneLink() throws -> PhoneLink {
            switch link {
            case "wifi": return .wifi
            case "ethernet": return .ethernet
            case "cellular": return .cellular
            case "other": return .other
            case "offline": return .offline
            default: throw ControllerModelError.invalidRoute
            }
        }
    }

    /// The fixture's routes, as the generated `RouteAddress` the planner takes.
    struct RouteWrapper: Decodable {
        let address: RouteAddress

        init(from decoder: Decoder) throws {
            address = try Route(from: decoder).routeAddress
        }
    }

    struct RememberedWrapper: Decodable {
        let fingerprint: String
        let route: RouteWrapper
    }

    static func load(bundle: Bundle) throws -> RoutePlanFixture {
        let url = try XCTUnwrap(bundle.url(forResource: "route-plan-v1", withExtension: "json"))
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(Self.self, from: Data(contentsOf: url))
    }
}
