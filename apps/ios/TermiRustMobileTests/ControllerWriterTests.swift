import Foundation
import XCTest
@testable import TermiRustMobile

final class ControllerWriterTests: XCTestCase {
    func testLeaseRaceAndBackgroundNeverQueueInput() throws {
        let identity = try makeIdentity()
        var reducer = try WriterControlReducer(identity: identity)
        let request = UUID()
        try reducer.beginAcquire(commandID: request)
        try reducer.finishAcquire(commandID: request, applied: false)
        XCTAssertEqual(reducer.lease, .busy)

        let retry = UUID()
        try reducer.beginAcquire(commandID: retry)
        try reducer.finishAcquire(commandID: retry, applied: true)
        try reducer.enqueue(Data("pwd\n".utf8), kind: .keyboard)
        XCTAssertEqual(reducer.queuedBytes, 4)

        reducer.setForeground(false)
        XCTAssertEqual(reducer.lease, .lost)
        XCTAssertEqual(reducer.queuedBytes, 0)
        XCTAssertThrowsError(try reducer.enqueue(Data("offline".utf8), kind: .keyboard))
    }

    func testInputAndPasteBoundariesFailClosed() throws {
        var reducer = try WriterControlReducer(identity: try makeIdentity())
        let request = UUID()
        try reducer.beginAcquire(commandID: request)
        try reducer.finishAcquire(commandID: request, applied: true)

        XCTAssertNoThrow(try reducer.enqueue(
            Data(repeating: 65, count: 16 * 1_024),
            kind: .keyboard
        ))
        XCTAssertThrowsError(try reducer.enqueue(
            Data(repeating: 65, count: 16 * 1_024 + 1),
            kind: .keyboard
        )) { error in
            XCTAssertEqual(error as? WriterControlFailure, .inputChunkTooLarge)
        }
        XCTAssertThrowsError(try reducer.enqueue(Data("one\ntwo".utf8), kind: .paste)) {
            error in
            XCTAssertEqual(error as? WriterControlFailure, .pasteConfirmationRequired)
        }
        XCTAssertNoThrow(try reducer.enqueue(
            Data("one\ntwo".utf8),
            kind: .paste,
            confirmed: true
        ))
    }

    func testWireCommandsBindExactIdentityGenerationAndLimits() throws {
        let identity = try makeIdentity()
        let commandID = UUID()
        let data = try ControllerWriterWireCodec.encodeInput(
            commandID: commandID,
            sessionGeneration: 9,
            deadlineMillis: 10_000,
            identity: identity,
            bytes: Data([27, 91, 65])
        )
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let command = try XCTUnwrap(object["command"] as? [String: Any])
        XCTAssertEqual(object["command_id"] as? String, commandID.uuidString)
        XCTAssertEqual((object["session_generation"] as? NSNumber)?.uint64Value, 9)
        XCTAssertEqual(command["kind"] as? String, "input")
        XCTAssertEqual(command["session_id"] as? String, identity.sessionID.uuidString)
        XCTAssertEqual((command["occupant_generation"] as? NSNumber)?.uint64Value, 7)
        XCTAssertEqual(command["bytes"] as? [Int], [27, 91, 65])
    }

    @MainActor
    func testViewModelAcquiresWritesAndReleasesOnBackground() async throws {
        let fixture = FixtureWriterConnection()
        let host = try PairedHostRecord(
            id: "host-id",
            displayName: "Build Host",
            route: try HostRoute(address: "192.168.1.20", port: 22_222),
            hostStaticPublicKey: Data(repeating: 9, count: 32),
            deviceStaticKeyId: "fixture-key",
            deviceId: UUID(),
            identityGeneration: 1,
            revocationEpoch: 1,
            sessionGeneration: 3,
            capabilityBits: 0b1111
        )
        let session = ControllerSessionSummary(
            id: UUID(),
            title: "Build",
            project: nil,
            group: nil,
            lifecycle: "live",
            activity: nil,
            occupantGeneration: 7,
            lastOutputSequence: 0,
            hasWriter: false,
            unreadCount: 0
        )
        let viewModel = try ControllerTerminalViewModel(
            host: host,
            session: session,
            connection: fixture,
            viewport: TerminalViewportState(columns: 40, rows: 5)
        )
        viewModel.updateViewport(columns: 36, rows: 8, final: true)

        viewModel.start()
        try await waitUntil { viewModel.attachState == .live }
        viewModel.requestControl()
        try await waitUntil { viewModel.canSendInput }
        let resizeViewports = await fixture.resizeViewports()
        XCTAssertEqual(
            resizeViewports,
            [TerminalViewportState(columns: 36, rows: 8)]
        )

        viewModel.updateViewport(columns: 36, rows: 4, final: true)
        viewModel.sendKeyboardBytes(Data("queued during resize".utf8))
        try await Task.sleep(for: .milliseconds(10))
        let inputCountDuringResize = await fixture.inputCount()
        XCTAssertEqual(inputCountDuringResize, 0)
        try await waitUntil {
            await fixture.inputs() == [Data("queued during resize".utf8)]
        }

        viewModel.sendKeyboardBytes(Data("echo ok\n".utf8))
        try await waitUntil {
            await fixture.inputs() == [
                Data("queued during resize".utf8),
                Data("echo ok\n".utf8),
            ]
        }

        viewModel.suspend()
        XCTAssertEqual(viewModel.writerLease, .lost)
        XCTAssertTrue(viewModel.privacyCovered)
        try await waitUntil { await fixture.releaseCount() == 1 }
    }

    @MainActor
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

    private func makeIdentity() throws -> ReadOnlyAttachIdentity {
        let identity = ReadOnlyAttachIdentity(
            hostID: "host-id",
            sessionID: UUID(),
            occupantGeneration: 7
        )
        try identity.validate()
        return identity
    }
}

private actor FixtureWriterConnection: ControllerConnecting {
    private var eventHandler: (@Sendable (ControllerReadOnlyWireEvent) async throws -> Void)?
    private var written: [Data] = []
    private var releases = 0
    private var resizeRequests: [TerminalViewportState] = []

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
        try await onEvent(.attached(
            replayThroughSequence: cursor.outputSequence,
            hasWriterLease: false
        ))
        while !Task.isCancelled { try await Task.sleep(for: .seconds(1)) }
    }

    func attachInteractive(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewportState,
        onEvent: @escaping @Sendable (ControllerReadOnlyWireEvent) async throws -> Void
    ) async throws {
        _ = (host, viewport)
        eventHandler = onEvent
        try await onEvent(.attached(
            replayThroughSequence: cursor.outputSequence,
            hasWriterLease: false
        ))
        while !Task.isCancelled { try await Task.sleep(for: .seconds(1)) }
    }

    func requestWriter(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID
    ) async throws {
        _ = (host, identity)
        try await eventHandler?(.completed(commandID: commandID, applied: true))
    }

    func releaseWriter(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID
    ) async throws {
        _ = (host, identity)
        releases += 1
        try await eventHandler?(.completed(commandID: commandID, applied: true))
    }

    func sendInput(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID,
        bytes: Data
    ) async throws {
        _ = (host, identity)
        written.append(bytes)
        try await eventHandler?(.completed(commandID: commandID, applied: true))
    }

    func sendResize(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID,
        viewport: TerminalViewportState
    ) async throws {
        _ = (host, identity)
        resizeRequests.append(viewport)
        try await Task.sleep(for: .milliseconds(50))
        try await eventHandler?(.completed(commandID: commandID, applied: true))
    }

    func forgetDeviceSecret(host: PairedHostRecord) async throws { _ = host }
    func cancel() async {}
    func inputs() -> [Data] { written }
    func releaseCount() -> Int { releases }
    func resizeViewports() -> [TerminalViewportState] { resizeRequests }
    func inputCount() -> Int { written.count }
}
