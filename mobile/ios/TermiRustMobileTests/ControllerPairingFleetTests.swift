import Foundation
@preconcurrency import Network
import XCTest
@testable import TermiRustMobile

@MainActor
final class ControllerPairingFleetTests: XCTestCase {
    func testLiveRustControllerPairingTerminalLifecycleAndRevocation() async throws {
        guard let config = LiveControllerFixtureConfig.load() else {
            throw XCTSkip("Run scripts/test-mobile-ios-controller-host.sh to provide the live Rust Controller fixture.")
        }

        let control = LiveControllerControlClient(config: config)
        let blobStore = ControllerKeychainBlobStore(
            service: "com.termirust.mobile.tests.\(config.fixtureID.uuidString.lowercased())"
        )
        let connection = try ControllerConnectionActor(blobStore: blobStore)
        var pairedHost: PairedHostRecord?
        var stage = "begin_pairing"

        do {
            let deviceID = UUID()
            let challenge = try await connection.beginPairing(
                offerText: config.offerText,
                hostName: "Live Rust Host",
                deviceName: "TermiRust XCTest",
                deviceID: deviceID
            )
            let hostSAS = try await control.waitForValue(command: "sas")
            XCTAssertEqual(challenge.sas, hostSAS, "Device and Host SAS values must match exactly")

            let confirmResponse = try await control.command("confirm")
            XCTAssertEqual(confirmResponse.value, "confirmed")
            stage = "finish_pairing"
            let initialHost = try await connection.finishPairing(matches: true)
            pairedHost = initialHost
            XCTAssertNotNil(try blobStore.load(keyId: initialHost.deviceStaticKeyId))
            let pairingStatus = try await control.waitForValue(command: "status", expected: "paired")
            XCTAssertEqual(pairingStatus, "paired")

            stage = "initial_fleet"
            let initialSnapshot = try await connection.fetchSessions(host: initialHost) { _ in }
            XCTAssertEqual(initialSnapshot.sessions.count, 1)
            XCTAssertEqual(initialSnapshot.sessions[0].id, config.sessionID)
            XCTAssertEqual(initialSnapshot.capabilityBits, 0b11)

            stage = "grant_input"
            let grantResponse = try await control.command("grant_input")
            XCTAssertEqual(grantResponse.value, "granted")
            stage = "refresh_capabilities"
            let writableSnapshot = try await waitForCapabilities(
                connection: connection,
                host: initialHost,
                required: 0b1_1111
            )
            let writableHost = try initialHost.replacingCapabilities(writableSnapshot.capabilityBits)
            pairedHost = writableHost
            let terminal = try ControllerTerminalViewModel(
                host: writableHost,
                session: writableSnapshot.sessions[0],
                connection: connection,
                viewport: TerminalViewportState(columns: 80, rows: 24)
            )

            stage = "readonly_attach"
            terminal.start()
            try await waitUntil(attempts: 750) {
                terminal.attachState == .live
                    && terminal.screen.lines.joined(separator: "\n").contains("MOBILE-CONTROLLER-READY")
            }

            stage = "writer_acquire"
            terminal.requestControl()
            try await waitUntil(attempts: 750) { terminal.writerLease == .held }
            stage = "input"
            terminal.sendKeyboardBytes(Data("N04-LIVE-MARKER\n".utf8))
            try await waitUntil(attempts: 750) {
                terminal.screen.lines.joined(separator: "\n")
                    .contains("MOBILE-CONTROLLER-OUT:N04-LIVE-MARKER")
            }
            terminal.updateViewport(columns: 118, rows: 34, final: true)

            stage = "suspend"
            terminal.suspend()
            XCTAssertTrue(terminal.privacyCovered)
            XCTAssertEqual(terminal.attachState, .offline)
            XCTAssertEqual(terminal.writerLease, .lost)

            stage = "resume"
            terminal.resume()
            try await waitUntil(attempts: 750) {
                terminal.attachState == .live && !terminal.privacyCovered
            }
            try await waitUntil(attempts: 750) { terminal.writerLease == .held }

            stage = "revoke"
            let revokeResponse = try await control.command("revoke")
            XCTAssertEqual(revokeResponse.value, "revoked")
            terminal.sendKeyboardBytes(Data("REVOKED-MUTATION-MUST-FAIL\n".utf8))
            try await waitUntil(attempts: 750) {
                terminal.writerLease == .lost || terminal.attachState == .offline
            }
            do {
                _ = try await connection.fetchSessions(host: writableHost) { _ in }
                XCTFail("A revoked Controller device authenticated again")
            } catch {
                // Any closed or rejected authenticated path is fail-closed after revocation.
            }
            terminal.detach()

            try await connection.forgetDeviceSecret(host: writableHost)
            XCTAssertNil(try blobStore.load(keyId: writableHost.deviceStaticKeyId))
            pairedHost = nil
        } catch {
            XCTFail("Live Rust Controller stage \(stage) failed: \(error)")
            if let pairedHost {
                try? await connection.forgetDeviceSecret(host: pairedHost)
            } else {
                await connection.cancel()
            }
            throw error
        }
    }

    func testPairingPersistsHostThenLoadsAuthoritativeReadOnlyFleet() async throws {
        let fixture = try Fixture.make()
        defer { try? FileManager.default.removeItem(at: fixture.directory) }
        let connection = FixtureConnection(record: fixture.record)
        let viewModel = ControllerViewModel(
            connectionActor: connection,
            hostStore: fixture.hostStore,
            cacheStore: fixture.cacheStore,
            defaults: UserDefaults(suiteName: UUID().uuidString)!
        )
        viewModel.pairingOfferText = "bounded-offer"
        viewModel.pairingHostName = "Office Mac"
        viewModel.pairingDeviceName = "Test iPhone"

        viewModel.beginPairing()
        try await waitUntil { viewModel.pairingChallenge?.sas == "ABCD-1234" }
        viewModel.finishPairing(matches: true)
        try await waitUntil { viewModel.state.connection == .readyReadOnly }

        XCTAssertEqual(viewModel.state.hosts.map(\.title), ["Office Mac"])
        XCTAssertEqual(viewModel.state.sessions.map(\.title), ["Build agent"])
        XCTAssertFalse(viewModel.state.isCachedReadOnly)
        let storedHosts = try await fixture.hostStore.load()
        let storedCache = try await fixture.cacheStore.load()
        XCTAssertEqual(storedHosts.count, 1)
        XCTAssertEqual(storedCache.hosts[fixture.record.id]?.sessions.count, 1)
    }

    func testFailedPairingDiscardsConsumedOfferBeforeRetry() async throws {
        let fixture = try Fixture.make()
        defer { try? FileManager.default.removeItem(at: fixture.directory) }
        let connection = FixtureConnection(
            record: fixture.record,
            finishFailure: .timedOut
        )
        let viewModel = ControllerViewModel(
            connectionActor: connection,
            hostStore: fixture.hostStore,
            cacheStore: fixture.cacheStore,
            defaults: UserDefaults(suiteName: UUID().uuidString)!
        )
        viewModel.pairingOfferText = "one-use-offer"

        viewModel.beginPairing()
        try await waitUntil { viewModel.pairingChallenge != nil }
        viewModel.finishPairing(matches: true)
        try await waitUntil { viewModel.state.connection == .failed(.timedOut) }

        XCTAssertNil(viewModel.pairingChallenge)
        XCTAssertTrue(viewModel.pairingOfferText.isEmpty)
    }

    func testOfflineRefreshPreservesAndMarksLastCompleteCacheReadOnly() async throws {
        let fixture = try Fixture.make()
        defer { try? FileManager.default.removeItem(at: fixture.directory) }
        var cache = ControllerFleetCache()
        try cache.replace(
            hostFingerprint: fixture.record.id,
            revision: 3,
            updateSequence: 3,
            sessions: [Fixture.session],
            selectedHostFingerprint: fixture.record.id,
            now: Date(timeIntervalSince1970: 5)
        )
        _ = try await fixture.hostStore.upsert(fixture.record)
        try await fixture.cacheStore.save(cache)
        let connection = FixtureConnection(record: fixture.record, failures: [.networkUnavailable])
        let viewModel = ControllerViewModel(
            connectionActor: connection,
            hostStore: fixture.hostStore,
            cacheStore: fixture.cacheStore,
            defaults: UserDefaults(suiteName: UUID().uuidString)!,
            retryPolicy: .immediateTestPolicy(maxAttempts: 1)
        )

        try await waitUntil {
            viewModel.state.isCachedReadOnly && viewModel.state.connection == .failed(.networkUnavailable)
        }

        XCTAssertEqual(viewModel.state.sessions, [Fixture.session])
        XCTAssertEqual(viewModel.state.cacheUpdatedAt, Date(timeIntervalSince1970: 5))
    }

    func testTransientFailuresRetryThenRecover() async throws {
        let fixture = try Fixture.make()
        defer { try? FileManager.default.removeItem(at: fixture.directory) }
        _ = try await fixture.hostStore.upsert(fixture.record)
        let connection = FixtureConnection(
            record: fixture.record,
            failures: [.networkUnavailable, .timedOut]
        )
        let viewModel = ControllerViewModel(
            connectionActor: connection,
            hostStore: fixture.hostStore,
            cacheStore: fixture.cacheStore,
            defaults: UserDefaults(suiteName: UUID().uuidString)!,
            retryPolicy: .immediateTestPolicy(maxAttempts: 8)
        )

        try await waitUntil { viewModel.state.connection == .readyReadOnly }

        let fetchCount = await connection.fetchCount
        XCTAssertEqual(fetchCount, 3)
    }

    func testAuthenticatedRefreshPersistsCurrentHostCapabilities() async throws {
        let fixture = try Fixture.make()
        defer { try? FileManager.default.removeItem(at: fixture.directory) }
        _ = try await fixture.hostStore.upsert(fixture.record)
        let connection = FixtureConnection(
            record: fixture.record,
            grantedCapabilityBits: 0b1_1111
        )
        let viewModel = ControllerViewModel(
            connectionActor: connection,
            hostStore: fixture.hostStore,
            cacheStore: fixture.cacheStore,
            defaults: UserDefaults(suiteName: UUID().uuidString)!,
            retryPolicy: .immediateTestPolicy(maxAttempts: 1)
        )

        try await waitUntil { viewModel.state.connection == .readyReadOnly }

        XCTAssertEqual(viewModel.state.hosts.first?.capabilityBits, 0b1_1111)
        let stored = try await fixture.hostStore.load()
        XCTAssertEqual(stored.first?.capabilityBits, 0b1_1111)
    }

    func testRetryStopsAtConfiguredAttemptLimit() async throws {
        let fixture = try Fixture.make()
        defer { try? FileManager.default.removeItem(at: fixture.directory) }
        _ = try await fixture.hostStore.upsert(fixture.record)
        let connection = FixtureConnection(record: fixture.record, repeatingFailure: .networkUnavailable)
        let viewModel = ControllerViewModel(
            connectionActor: connection,
            hostStore: fixture.hostStore,
            cacheStore: fixture.cacheStore,
            defaults: UserDefaults(suiteName: UUID().uuidString)!,
            retryPolicy: .immediateTestPolicy(maxAttempts: 8)
        )

        try await waitUntil { viewModel.state.connection == .failed(.networkUnavailable) }

        let fetchCount = await connection.fetchCount
        XCTAssertEqual(fetchCount, 8)
    }

    func testAuthenticationFailureDoesNotRetry() async throws {
        let fixture = try Fixture.make()
        defer { try? FileManager.default.removeItem(at: fixture.directory) }
        _ = try await fixture.hostStore.upsert(fixture.record)
        let connection = FixtureConnection(record: fixture.record, repeatingFailure: .authenticationFailed)
        let viewModel = ControllerViewModel(
            connectionActor: connection,
            hostStore: fixture.hostStore,
            cacheStore: fixture.cacheStore,
            defaults: UserDefaults(suiteName: UUID().uuidString)!,
            retryPolicy: .immediateTestPolicy(maxAttempts: 8)
        )

        try await waitUntil { viewModel.state.connection == .failed(.authenticationFailed) }

        let fetchCount = await connection.fetchCount
        XCTAssertEqual(fetchCount, 1)
    }

    func testAuthenticationConnectionCloseDoesNotRemainReconnecting() async throws {
        let fixture = try Fixture.make()
        defer { try? FileManager.default.removeItem(at: fixture.directory) }
        _ = try await fixture.hostStore.upsert(fixture.record)
        let connection = FixtureConnection(
            record: fixture.record,
            fetchPairingFailure: .connectionClosed
        )
        let viewModel = ControllerViewModel(
            connectionActor: connection,
            hostStore: fixture.hostStore,
            cacheStore: fixture.cacheStore,
            defaults: UserDefaults(suiteName: UUID().uuidString)!,
            retryPolicy: .immediateTestPolicy(maxAttempts: 8)
        )

        try await waitUntil { viewModel.state.connection == .failed(.authenticationFailed) }

        let fetchCount = await connection.fetchCount
        XCTAssertEqual(fetchCount, 1)
        XCTAssertEqual(
            viewModel.routeProjections.first(where: { $0.selected })?.phase,
            .degraded
        )
    }

    func testFullJitterDelayIsBoundedByBackoffAndDeadline() {
        let policy = ControllerRetryPolicy(
            maxAttempts: 8,
            maxElapsedSeconds: 90,
            baseDelaySeconds: 1,
            maxDelaySeconds: 30,
            randomUnit: { 0.75 },
            sleep: { _ in }
        )

        XCTAssertEqual(policy.delayAfterFailure(attempt: 1, elapsedSeconds: 0), 0.75)
        XCTAssertEqual(policy.delayAfterFailure(attempt: 6, elapsedSeconds: 0), 22.5)
        XCTAssertEqual(policy.delayAfterFailure(attempt: 6, elapsedSeconds: 89), 1)
        XCTAssertNil(policy.delayAfterFailure(attempt: 8, elapsedSeconds: 0))
        XCTAssertNil(policy.delayAfterFailure(attempt: 1, elapsedSeconds: 90))
    }

    func testUnreconciledPairingAcknowledgementHasExplicitRecoveryState() async throws {
        let fixture = try Fixture.make()
        defer { try? FileManager.default.removeItem(at: fixture.directory) }
        let connection = FixtureConnection(
            record: fixture.record,
            finishFailure: .acknowledgementUncertain
        )
        let viewModel = ControllerViewModel(
            connectionActor: connection,
            hostStore: fixture.hostStore,
            cacheStore: fixture.cacheStore,
            defaults: UserDefaults(suiteName: UUID().uuidString)!,
            retryPolicy: .immediateTestPolicy(maxAttempts: 1)
        )
        viewModel.pairingOfferText = "bounded-offer"
        viewModel.pairingHostName = "Office Mac"
        viewModel.pairingDeviceName = "Test iPhone"

        viewModel.beginPairing()
        try await waitUntil { viewModel.pairingChallenge != nil }
        viewModel.finishPairing(matches: true)
        try await waitUntil { viewModel.state.connection == .failed(.pairingUncertain) }

        let storedHosts = try await fixture.hostStore.load()
        XCTAssertTrue(storedHosts.isEmpty)
    }

    func testControllerPresentationBoundsUserTextAndCapabilities() {
        XCTAssertEqual(
            ControllerPresentation.isolated("Host"),
            "\u{2068}Host\u{2069}"
        )
        XCTAssertEqual(
            ControllerPresentation.fingerprintForSpeech("a1"),
            "a 1"
        )
        XCTAssertEqual(ControllerPresentation.capabilityLabels(bits: 0).count, 0)
        XCTAssertEqual(ControllerPresentation.capabilityLabels(bits: 0b1_1111).count, 5)
        XCTAssertFalse(ControllerPresentation.unreadDescription(2).isEmpty)
    }

    func testControllerPresentationGroupsSessionsInHostOrder() {
        let release = ControllerSessionSummary(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000002")!,
            title: "Release",
            project: "Console",
            group: "Deploy",
            lifecycle: "live",
            activity: "busy",
            occupantGeneration: 1,
            lastOutputSequence: 5,
            hasWriter: false,
            unreadCount: 1
        )
        let monitor = ControllerSessionSummary(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000003")!,
            title: "Monitor",
            project: "Console",
            group: "Deploy",
            lifecycle: "live",
            activity: "idle",
            occupantGeneration: 1,
            lastOutputSequence: 6,
            hasWriter: false,
            unreadCount: 0
        )

        let groups = ControllerPresentation.sessionGroups([Fixture.session, release, monitor])

        XCTAssertEqual(groups.count, 2)
        XCTAssertEqual(groups[0].sessions.map(\.id), [Fixture.session.id])
        XCTAssertNil(ControllerPresentation.sessionGroupTitle(groups[0].id))
        XCTAssertEqual(groups[1].sessions.map(\.id), [release.id, monitor.id])
        XCTAssertEqual(
            ControllerPresentation.sessionGroupTitle(groups[1].id),
            "\u{2068}Console\u{2069} · \u{2068}Deploy\u{2069}"
        )
    }

    func testControllerPresentationSeparatesOpenTerminalsFromHistory() {
        let open = ControllerSessionSummary(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000020")!,
            origin: .terminal,
            runtime: "local_shell",
            capabilities: [.observeSessions, .attachOutput, .sendInput],
            title: "Local Terminal",
            project: nil,
            group: nil,
            lifecycle: "live",
            activity: "unknown",
            occupantGeneration: 1,
            lastOutputSequence: 2,
            hasWriter: false,
            unreadCount: 0
        )
        let closed = ControllerSessionSummary(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000021")!,
            title: "Previous task",
            project: nil,
            group: nil,
            lifecycle: "exited",
            activity: "done",
            occupantGeneration: nil,
            lastOutputSequence: 2,
            hasWriter: false,
            unreadCount: 0
        )

        XCTAssertEqual(ControllerPresentation.openTerminals([closed, open]).map(\.id), [open.id])
        XCTAssertEqual(ControllerPresentation.previousSessions([closed, open]).map(\.id), [closed.id])
    }

    private func waitUntil(
        attempts: Int = 100,
        condition: @MainActor () -> Bool
    ) async throws {
        for _ in 0..<attempts {
            if condition() { return }
            try await Task.sleep(for: .milliseconds(20))
        }
        XCTFail("Condition did not become true before timeout")
    }
}

private extension PairedHostRecord {
    func replacingCapabilities(_ capabilityBits: UInt16) throws -> Self {
        try Self(
            id: id,
            displayName: displayName,
            route: route,
            hostStaticPublicKey: hostStaticPublicKey,
            deviceStaticKeyId: deviceStaticKeyId,
            deviceId: deviceId,
            identityGeneration: identityGeneration,
            revocationEpoch: revocationEpoch,
            sessionGeneration: sessionGeneration,
            capabilityBits: capabilityBits,
            pairedAt: pairedAt
        )
    }
}

private struct LiveControllerFixtureConfig: Decodable, Sendable {
    let schemaVersion: UInt16
    let fixtureID: UUID
    let offerText: String
    let controlAddress: String
    let controlPort: UInt16
    let controlToken: String
    let sessionID: UUID

    private enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case fixtureID = "fixture_id"
        case offerText = "offer_text"
        case controlAddress = "control_address"
        case controlPort = "control_port"
        case controlToken = "control_token"
        case sessionID = "session_id"
    }

    static func load() -> Self? {
        guard let url = Bundle(for: ControllerPairingFleetTests.self)
            .url(forResource: "controller-v1", withExtension: "json") else {
            return nil
        }
        guard let data = try? Data(contentsOf: url),
              let value = try? JSONDecoder().decode(Self.self, from: data),
              value.schemaVersion == 1,
              !value.offerText.isEmpty,
              !value.controlAddress.isEmpty,
              !value.controlToken.isEmpty else {
            return nil
        }
        return value
    }
}

private struct LiveControllerControlResponse: Decodable, Sendable {
    let ok: Bool
    let value: String?
}

private enum LiveControllerControlError: Error {
    case invalidPort
    case connectionClosed
    case oversizedResponse
    case rejected(String?)
}

private actor LiveControllerControlClient {
    private static let maximumResponseBytes = 4 * 1_024
    private let config: LiveControllerFixtureConfig

    init(config: LiveControllerFixtureConfig) {
        self.config = config
    }

    func waitForValue(command: String, expected: String? = nil) async throws -> String {
        for _ in 0..<200 {
            let response = try await send(command)
            if response.ok,
               let value = response.value,
               expected == nil || value == expected {
                return value
            }
            try await Task.sleep(for: .milliseconds(25))
        }
        throw LiveControllerControlError.rejected(nil)
    }

    func command(_ command: String) async throws -> LiveControllerControlResponse {
        let response = try await send(command)
        guard response.ok else {
            throw LiveControllerControlError.rejected(response.value)
        }
        return response
    }

    private func send(_ command: String) async throws -> LiveControllerControlResponse {
        let host = NWEndpoint.Host(config.controlAddress)
        guard let port = NWEndpoint.Port(rawValue: config.controlPort) else {
            throw LiveControllerControlError.invalidPort
        }
        let connection = NWConnection(host: host, port: port, using: .tcp)
        defer { connection.cancel() }
        try await Self.start(connection)
        var request = try JSONEncoder().encode([
            "token": config.controlToken,
            "command": command,
        ])
        request.append(0x0A)
        try await Self.send(request, over: connection)
        let response = try await Self.receive(over: connection)
        return try JSONDecoder().decode(LiveControllerControlResponse.self, from: response)
    }

    private static func start(_ connection: NWConnection) async throws {
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                let gate = LiveControllerConnectionGate()
                connection.stateUpdateHandler = { state in
                    switch state {
                    case .ready:
                        if gate.claim() { continuation.resume() }
                    case .failed(let error):
                        if gate.claim() { continuation.resume(throwing: error) }
                    case .cancelled:
                        if gate.claim() { continuation.resume(throwing: CancellationError()) }
                    default:
                        break
                    }
                }
                connection.start(queue: DispatchQueue(label: "com.termirust.tests.controller-control"))
            }
        } onCancel: {
            connection.cancel()
        }
    }

    private static func send(_ data: Data, over connection: NWConnection) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            connection.send(content: data, completion: .contentProcessed { error in
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume()
                }
            })
        }
    }

    private static func receive(over connection: NWConnection) async throws -> Data {
        var response = Data()
        while response.count <= maximumResponseBytes {
            let chunk: (Data, Bool) = try await withCheckedThrowingContinuation { continuation in
                connection.receive(
                    minimumIncompleteLength: 1,
                    maximumLength: maximumResponseBytes + 1 - response.count
                ) { data, _, complete, error in
                    if let error {
                        continuation.resume(throwing: error)
                    } else {
                        continuation.resume(returning: (data ?? Data(), complete))
                    }
                }
            }
            response.append(chunk.0)
            if chunk.1 { break }
            if chunk.0.isEmpty { throw LiveControllerControlError.connectionClosed }
        }
        guard !response.isEmpty else { throw LiveControllerControlError.connectionClosed }
        guard response.count <= maximumResponseBytes else {
            throw LiveControllerControlError.oversizedResponse
        }
        return response
    }
}

private final class LiveControllerConnectionGate: @unchecked Sendable {
    private let lock = NSLock()
    private var claimed = false

    func claim() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard !claimed else { return false }
        claimed = true
        return true
    }
}

@MainActor
private func waitForCapabilities(
    connection: ControllerConnectionActor,
    host: PairedHostRecord,
    required: UInt16
) async throws -> ControllerFleetSnapshot {
    var lastSnapshot: ControllerFleetSnapshot?
    for _ in 0..<100 {
        let snapshot = try await connection.fetchSessions(host: host) { _ in }
        lastSnapshot = snapshot
        if snapshot.capabilityBits & required == required { return snapshot }
        try await Task.sleep(for: .milliseconds(25))
    }
    throw LiveControllerControlError.rejected(
        "capabilities=\(lastSnapshot?.capabilityBits ?? 0)"
    )
}

private actor FixtureConnection: ControllerConnecting {
    let record: PairedHostRecord
    private let grantedCapabilityBits: UInt16
    private var failures: [ControllerFailure]
    private let repeatingFailure: ControllerFailure?
    private let finishFailure: ControllerPairingError?
    private let fetchPairingFailure: ControllerPairingError?
    private(set) var fetchCount = 0

    init(
        record: PairedHostRecord,
        failures: [ControllerFailure] = [],
        repeatingFailure: ControllerFailure? = nil,
        finishFailure: ControllerPairingError? = nil,
        fetchPairingFailure: ControllerPairingError? = nil,
        grantedCapabilityBits: UInt16? = nil
    ) {
        self.record = record
        self.grantedCapabilityBits = grantedCapabilityBits ?? record.capabilityBits
        self.failures = failures
        self.repeatingFailure = repeatingFailure
        self.finishFailure = finishFailure
        self.fetchPairingFailure = fetchPairingFailure
    }

    func beginPairing(
        offerText: String,
        hostName: String,
        deviceName: String,
        deviceID: UUID
    ) async throws -> ControllerPairingChallenge {
        ControllerPairingChallenge(
            hostFingerprint: record.id,
            route: record.route,
            sas: "ABCD-1234",
            expiresAt: .now.addingTimeInterval(60)
        )
    }

    func finishPairing(matches: Bool) async throws -> PairedHostRecord {
        guard matches else { throw ControllerPairingError.rejected }
        if let finishFailure { throw finishFailure }
        return record
    }

    func fetchSessions(
        host: PairedHostRecord,
        progress: @escaping @Sendable (ControllerConnectionProgress) async -> Void
    ) async throws -> ControllerFleetSnapshot {
        fetchCount += 1
        await progress(.authenticating)
        if let fetchPairingFailure { throw fetchPairingFailure }
        if !failures.isEmpty { throw failures.removeFirst() }
        if let repeatingFailure { throw repeatingFailure }
        await progress(.syncing)
        return ControllerFleetSnapshot(
            revision: 1,
            updateSequence: 1,
            capabilityBits: grantedCapabilityBits,
            sessions: [Fixture.session]
        )
    }

    func forgetDeviceSecret(host: PairedHostRecord) async throws {}
    func cancel() async {}
}

private extension ControllerRetryPolicy {
    static func immediateTestPolicy(maxAttempts: Int) -> Self {
        Self(
            maxAttempts: maxAttempts,
            maxElapsedSeconds: 90,
            baseDelaySeconds: 1,
            maxDelaySeconds: 30,
            randomUnit: { 0 },
            sleep: { _ in }
        )
    }
}

private struct Fixture {
    let directory: URL
    let record: PairedHostRecord
    let hostStore: PairedHostStore
    let cacheStore: ControllerFleetCacheStore

    static let session = ControllerSessionSummary(
        id: UUID(uuidString: "00000000-0000-0000-0000-000000000001")!,
        title: "Build agent",
        project: nil,
        group: nil,
        lifecycle: "running",
        activity: "busy",
        occupantGeneration: 1,
        lastOutputSequence: 4,
        hasWriter: false,
        unreadCount: 0
    )

    static func make() throws -> Self {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let record = try PairedHostRecord(
            id: "host-fixture",
            displayName: "Office Mac",
            route: HostRoute(address: "192.168.1.20", port: 55_555),
            hostStaticPublicKey: Data(repeating: 4, count: 32),
            deviceStaticKeyId: "controller.device.fixture",
            deviceId: UUID(uuidString: "00000000-0000-0000-0000-000000000002")!,
            identityGeneration: 1,
            revocationEpoch: 0,
            sessionGeneration: 0,
            capabilityBits: 1,
            pairedAt: Date(timeIntervalSince1970: 1)
        )
        return try Self(
            directory: directory,
            record: record,
            hostStore: PairedHostStore(fileURL: directory.appendingPathComponent("hosts.json")),
            cacheStore: ControllerFleetCacheStore(fileURL: directory.appendingPathComponent("cache.json"))
        )
    }
}
