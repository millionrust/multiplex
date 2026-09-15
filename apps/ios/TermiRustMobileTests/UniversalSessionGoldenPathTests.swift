import Foundation
import XCTest
@testable import TermiRustMobile

final class UniversalSessionGoldenPathTests: XCTestCase {
    func testSharedIdentityCapabilitiesAndWriterLifecycle() throws {
        let fixture = try UniversalSessionFixture.load()
        XCTAssertEqual(fixture.schemaVersion, 1)
        XCTAssertEqual(fixture.controller.capabilityBits, 31)
        XCTAssertEqual(fixture.controller.revocationEpoch, 3)
        XCTAssertEqual(Set(fixture.scenarios), Set([
            "same_identity_and_capabilities",
            "single_writer",
            "background_releases_writer",
            "acknowledged_input_not_replayed",
            "revocation_stops_mutation",
        ]))

        let session = ControllerSessionSummary(
            id: fixture.session.sessionID,
            hostInstanceID: fixture.session.hostInstanceID,
            origin: fixture.session.origin,
            runtime: fixture.session.runtime,
            capabilities: fixture.session.capabilities,
            title: "Golden Session",
            project: nil,
            group: nil,
            lifecycle: "live",
            activity: "busy",
            occupantGeneration: fixture.session.occupantGeneration,
            lastOutputSequence: fixture.session.lastOutputSequence,
            hasWriter: false,
            unreadCount: 0
        )
        try session.validate()
        XCTAssertEqual(session.hostInstanceID, fixture.session.hostInstanceID)
        XCTAssertEqual(session.capabilities, fixture.session.capabilities)

        let identity = ReadOnlyAttachIdentity(
            hostID: "golden-host-fingerprint",
            hostInstanceID: session.hostInstanceID,
            sessionID: session.id,
            occupantGeneration: fixture.session.occupantGeneration
        )
        var first = try WriterControlReducer(identity: identity)
        var second = try WriterControlReducer(identity: identity)
        try first.beginAcquire(commandID: fixture.commands.firstWriter)
        try first.finishAcquire(commandID: fixture.commands.firstWriter, applied: true)
        try second.beginAcquire(commandID: fixture.commands.secondWriter)
        try second.finishAcquire(commandID: fixture.commands.secondWriter, applied: false)
        XCTAssertEqual(first.lease, .held)
        XCTAssertEqual(second.lease, .busy)

        let input = Data(fixture.inputBytes)
        try first.enqueue(input, kind: .keyboard, commandID: fixture.commands.input)
        XCTAssertEqual(first.dequeue()?.bytes, input)
        XCTAssertNil(first.dequeue())

        first.setForeground(false)
        XCTAssertEqual(first.lease, .lost)
        XCTAssertEqual(first.queuedBytes, 0)
        XCTAssertThrowsError(try first.enqueue(input, kind: .keyboard))

        var reconnected = try WriterControlReducer(identity: identity)
        XCTAssertNil(reconnected.dequeue(), "acknowledged input must not be reconstructed on reconnect")
        try reconnected.beginAcquire(commandID: fixture.commands.reconnectWriter)
        try reconnected.finishAcquire(commandID: fixture.commands.reconnectWriter, applied: true)
        reconnected.markLeaseLost()
        XCTAssertThrowsError(
            try reconnected.enqueue(
                input,
                kind: .keyboard,
                commandID: fixture.commands.deniedInput
            )
        )

        let viewport = TerminalViewportState(
            columns: fixture.viewport.columns,
            rows: fixture.viewport.rows
        )
        XCTAssertNoThrow(try TerminalLimits.controllerDefault.validate(viewport: viewport))
    }
}

private struct UniversalSessionFixture: Decodable {
    let schemaVersion: Int
    let session: Session
    let controller: Controller
    let commands: Commands
    let inputBytes: [UInt8]
    let viewport: Viewport
    let scenarios: [String]

    struct Session: Decodable {
        let sessionID: UUID
        let hostInstanceID: UUID
        let occupantGeneration: UInt64
        let sessionGeneration: UInt64
        let origin: ControllerSessionOrigin
        let runtime: String
        let capabilities: [ControllerSessionCapability]
        let lastOutputSequence: UInt64

        private enum CodingKeys: String, CodingKey {
            case sessionID = "session_id"
            case hostInstanceID = "host_instance_id"
            case occupantGeneration = "occupant_generation"
            case sessionGeneration = "session_generation"
            case origin
            case runtime
            case capabilities
            case lastOutputSequence = "last_output_sequence"
        }
    }

    struct Commands: Decodable {
        let firstWriter: UUID
        let secondWriter: UUID
        let input: UUID
        let release: UUID
        let secondAfterRelease: UUID
        let deniedInput: UUID
        let reconnectWriter: UUID
        let resize: UUID
        let stop: UUID

        private enum CodingKeys: String, CodingKey {
            case firstWriter = "first_writer"
            case secondWriter = "second_writer"
            case input
            case release
            case secondAfterRelease = "second_after_release"
            case deniedInput = "denied_input"
            case reconnectWriter = "reconnect_writer"
            case resize
            case stop
        }
    }

    struct Controller: Decodable {
        let deviceID: UUID
        let identityGeneration: UInt64
        let revocationEpoch: UInt64
        let capabilityBits: UInt16

        private enum CodingKeys: String, CodingKey {
            case deviceID = "device_id"
            case identityGeneration = "identity_generation"
            case revocationEpoch = "revocation_epoch"
            case capabilityBits = "capability_bits"
        }
    }

    struct Viewport: Decodable {
        let columns: Int
        let rows: Int
    }

    static func load() throws -> Self {
        let url = try XCTUnwrap(
            Bundle(for: UniversalSessionGoldenPathTests.self)
                .url(forResource: "universal-session-v1", withExtension: "json")
        )
        return try JSONDecoder().decode(Self.self, from: Data(contentsOf: url))
    }

    private enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case session
        case controller
        case commands
        case inputBytes = "input_bytes"
        case viewport
        case scenarios
    }
}
