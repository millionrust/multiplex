import Foundation
import XCTest
@testable import TermiRustMobile

@MainActor
final class ControllerTerminalViewModelTests: XCTestCase {
    func testAttachBackgroundResumeAndDetachRemainReadOnly() async throws {
        let fixture = FixtureReadOnlyConnection()
        let host = try PairedHostRecord(
            id: "host-id",
            displayName: "Build Mac",
            route: try HostRoute(address: "192.168.1.20", port: 22_222),
            hostStaticPublicKey: Data(repeating: 7, count: 32),
            deviceStaticKeyId: "fixture-key",
            deviceId: UUID(),
            identityGeneration: 1,
            revocationEpoch: 1,
            sessionGeneration: 1,
            capabilityBits: 0b11
        )
        let session = ControllerSessionSummary(
            id: UUID(),
            title: "Tests",
            project: "TermiRust",
            group: nil,
            lifecycle: "live",
            activity: "busy",
            occupantGeneration: 4,
            lastOutputSequence: 0,
            hasWriter: false,
            unreadCount: 0
        )
        let viewModel = try ControllerTerminalViewModel(
            host: host,
            session: session,
            connection: fixture,
            viewport: TerminalViewportState(columns: 40, rows: 4)
        )

        viewModel.updateViewport(columns: 32, rows: 6, final: true)
        XCTAssertEqual(viewModel.screen.cells.first?.count, 32)
        XCTAssertEqual(viewModel.screen.lines.count, 6)

        viewModel.start()
        try await waitUntil {
            viewModel.attachState == .live
                && viewModel.outputSequence == 1
                && viewModel.screen.lines[0] == "run 1"
        }
        XCTAssertEqual(viewModel.screen.lines[0], "run 1")

        viewModel.suspend()
        XCTAssertEqual(viewModel.attachState, .offline)
        try await waitUntil { await fixture.cancelCount() >= 1 }

        viewModel.resume()
        try await waitUntil {
            viewModel.attachState == .live
                && viewModel.outputSequence == 2
                && viewModel.screen.lines[0] == "run 1run 2"
        }
        XCTAssertEqual(viewModel.screen.lines[0], "run 1run 2")

        viewModel.detach()
        XCTAssertEqual(viewModel.attachState, .detached)
        let cursors = await fixture.attachCursors()
        let capabilities = await fixture.requestedCapabilities()
        XCTAssertEqual(cursors, [0, 1])
        XCTAssertEqual(capabilities, [0b10, 0b10])
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

private actor FixtureReadOnlyConnection: ControllerConnecting {
    private var cursors: [UInt64] = []
    private var capabilities: [UInt16] = []
    private var cancellations = 0

    func beginPairing(
        offerText: String,
        hostName: String,
        deviceName: String,
        deviceID: UUID
    ) async throws -> ControllerPairingChallenge {
        _ = (offerText, hostName, deviceName, deviceID)
        throw ControllerConnectionError.capabilityDenied
    }

    func finishPairing(matches: Bool) async throws -> PairedHostRecord {
        _ = matches
        throw ControllerConnectionError.capabilityDenied
    }

    func fetchSessions(
        host: PairedHostRecord,
        progress: @escaping @Sendable (ControllerConnectionProgress) async -> Void
    ) async throws -> ControllerFleetSnapshot {
        _ = (host, progress)
        throw ControllerConnectionError.capabilityDenied
    }

    func attachReadOnly(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewportState,
        onEvent: @escaping @Sendable (ControllerReadOnlyWireEvent) async throws -> Void
    ) async throws {
        _ = (host, viewport)
        cursors.append(cursor.outputSequence)
        capabilities.append(0b10)
        let next = cursor.outputSequence + 1
        try await onEvent(.attached(replayThroughSequence: next, hasWriterLease: false))
        try await onEvent(.output(TerminalOutputFrame(
            sessionID: cursor.identity.sessionID,
            sequence: next,
            bytes: Data("run \(next)".utf8)
        )))
        while !Task.isCancelled { try await Task.sleep(for: .seconds(1)) }
        throw CancellationError()
    }

    func forgetDeviceSecret(host: PairedHostRecord) async throws { _ = host }

    func cancel() async { cancellations += 1 }

    func attachCursors() -> [UInt64] { cursors }
    func requestedCapabilities() -> [UInt16] { capabilities }
    func cancelCount() -> Int { cancellations }
}
