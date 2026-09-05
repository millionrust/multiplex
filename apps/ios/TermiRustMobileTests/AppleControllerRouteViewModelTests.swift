import Foundation
import XCTest
@testable import TermiRustMobile

@MainActor
final class AppleControllerRouteViewModelTests: XCTestCase {
    func testConfirmedSwitchCancelsOnlySourceAndUsesOnlySelectedTransport() async throws {
        let fixture = try RouteViewModelFixture.make()
        defer { try? FileManager.default.removeItem(at: fixture.directory) }
        _ = try await fixture.hostStore.upsert(fixture.host)
        let privateNetwork = RouteFixtureConnection()
        let ssh = RouteFixtureConnection()
        let relay = RouteFixtureConnection()
        let viewModel = ControllerViewModel(
            routeConnections: AppleControllerRouteConnections(
                privateNetwork: privateNetwork,
                ssh: ssh,
                selfHostedRelay: relay
            ),
            hostStore: fixture.hostStore,
            cacheStore: fixture.cacheStore,
            defaults: UserDefaults(suiteName: UUID().uuidString)!,
            retryPolicy: .singleAttempt
        )

        try await waitUntil { viewModel.state.connection == .readyReadOnly }
        let initialPrivateFetches = await privateNetwork.fetches()
        let initialSSHFetches = await ssh.fetches()
        let initialRelayFetches = await relay.fetches()
        XCTAssertEqual(initialPrivateFetches, 1)
        XCTAssertEqual(initialSSHFetches, 0)
        XCTAssertEqual(initialRelayFetches, 0)

        XCTAssertFalse(viewModel.selectControllerRoute(.ssh, explicitlyConfirmed: false))
        XCTAssertEqual(viewModel.selectedRoute, .privateNetwork)
        let unconfirmedCancellations = await privateNetwork.cancellations()
        XCTAssertEqual(unconfirmedCancellations, 0)

        XCTAssertTrue(viewModel.selectControllerRoute(.ssh, explicitlyConfirmed: true))
        try await waitUntil {
            let sshFetches = await ssh.fetches()
            return viewModel.selectedRoute == .ssh
                && viewModel.state.connection == .readyReadOnly
                && sshFetches == 1
        }
        let switchedPrivateCancellations = await privateNetwork.cancellations()
        let switchedRelayFetches = await relay.fetches()
        XCTAssertEqual(switchedPrivateCancellations, 1)
        XCTAssertEqual(switchedRelayFetches, 0)

        let privateBeforeSuspend = await privateNetwork.cancellations()
        let relayBeforeSuspend = await relay.cancellations()
        let sshBeforeSuspend = await ssh.cancellations()
        viewModel.suspend()
        try await waitUntil { await ssh.cancellations() == sshBeforeSuspend + 1 }
        let privateAfterSuspend = await privateNetwork.cancellations()
        let relayAfterSuspend = await relay.cancellations()
        XCTAssertEqual(privateAfterSuspend, privateBeforeSuspend)
        XCTAssertEqual(relayAfterSuspend, relayBeforeSuspend)
    }

    func testSelectedFailureDegradesWithoutSilentFallback() async throws {
        let fixture = try RouteViewModelFixture.make()
        defer { try? FileManager.default.removeItem(at: fixture.directory) }
        _ = try await fixture.hostStore.upsert(fixture.host)
        let privateNetwork = RouteFixtureConnection()
        let ssh = RouteFixtureConnection(failure: ControllerFailure.authenticationFailed)
        let relay = RouteFixtureConnection()
        let viewModel = ControllerViewModel(
            routeConnections: AppleControllerRouteConnections(
                privateNetwork: privateNetwork,
                ssh: ssh,
                selfHostedRelay: relay
            ),
            hostStore: fixture.hostStore,
            cacheStore: fixture.cacheStore,
            defaults: UserDefaults(suiteName: UUID().uuidString)!,
            retryPolicy: .singleAttempt
        )

        try await waitUntil { viewModel.state.connection == .readyReadOnly }
        XCTAssertTrue(viewModel.selectControllerRoute(.ssh, explicitlyConfirmed: true))
        try await waitUntil { viewModel.state.connection == .failed(.authenticationFailed) }

        XCTAssertEqual(viewModel.selectedRoute, .ssh)
        let failedSSHFetches = await ssh.fetches()
        let failedRelayFetches = await relay.fetches()
        XCTAssertEqual(failedSSHFetches, 1)
        XCTAssertEqual(failedRelayFetches, 0)
        XCTAssertEqual(
            viewModel.routeProjections.first(where: { $0.route == .ssh })?.phase,
            .degraded
        )
        XCTAssertEqual(
            viewModel.routeProjections.first(where: { $0.route == .ssh })?.recovery,
            .retry
        )
    }

    private func waitUntil(
        timeout: Duration = .seconds(2),
        condition: @escaping @MainActor () async -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while clock.now < deadline {
            if await condition() { return }
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTFail("condition did not become true before timeout")
    }
}

private actor RouteFixtureConnection: ControllerConnecting {
    private var fetchCount = 0
    private var cancelCount = 0
    private let failure: (any Error)?

    init(failure: (any Error)? = nil) {
        self.failure = failure
    }

    func beginPairing(
        offerText: String,
        hostName: String,
        deviceName: String,
        deviceID: UUID
    ) async throws -> ControllerPairingChallenge {
        throw ControllerPairingError.invalidOffer
    }

    func finishPairing(matches: Bool) async throws -> PairedHostRecord {
        throw ControllerPairingError.invalidOffer
    }

    func fetchSessions(
        host: PairedHostRecord,
        progress: @escaping @Sendable (ControllerConnectionProgress) async -> Void
    ) async throws -> ControllerFleetSnapshot {
        fetchCount += 1
        await progress(.authenticating)
        if let failure { throw failure }
        await progress(.syncing)
        return ControllerFleetSnapshot(
            revision: 1,
            updateSequence: 1,
            capabilityBits: host.capabilityBits,
            sessions: []
        )
    }

    func forgetDeviceSecret(host: PairedHostRecord) async throws {}
    func cancel() async { cancelCount += 1 }
    func fetches() -> Int { fetchCount }
    func cancellations() -> Int { cancelCount }
}

private struct RouteViewModelFixture {
    let directory: URL
    let host: PairedHostRecord
    let hostStore: PairedHostStore
    let cacheStore: ControllerFleetCacheStore

    static func make() throws -> Self {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return Self(
            directory: directory,
            host: try PairedHostRecord(
                id: "route-host",
                displayName: "Route Host",
                route: HostRoute(address: "192.168.1.20", port: 55_555),
                hostStaticPublicKey: Data(repeating: 7, count: 32),
                deviceStaticKeyId: "controller.route.fixture",
                deviceId: UUID(),
                identityGeneration: 1,
                revocationEpoch: 1,
                sessionGeneration: 1,
                capabilityBits: 0b1_1111
            ),
            hostStore: try PairedHostStore(
                fileURL: directory.appendingPathComponent("hosts.json")
            ),
            cacheStore: try ControllerFleetCacheStore(
                fileURL: directory.appendingPathComponent("cache.json")
            )
        )
    }
}

private extension ControllerRetryPolicy {
    static let singleAttempt = Self(
        maxAttempts: 1,
        maxElapsedSeconds: 90,
        baseDelaySeconds: 1,
        maxDelaySeconds: 30,
        randomUnit: { 0 },
        sleep: { _ in }
    )
}
