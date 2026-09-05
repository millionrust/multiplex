import Foundation
import XCTest
@testable import TermiRustMobile

final class ControllerReadOnlyTerminalTests: XCTestCase {
    func testAttachCodecBindsCommandToExactCursorAndViewport() throws {
        let commandID = UUID(uuidString: "10000000-0000-0000-0000-000000000001")!
        let reducer = try makeReducer(fromSequence: 9)
        let encoded = try ControllerReadOnlyWireCodec.encodeAttach(
            commandID: commandID,
            sessionGeneration: 4,
            deadlineMillis: 30_000,
            cursor: reducer.cursor,
            viewport: TerminalViewportState(columns: 120, rows: 40)
        )
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        let command = try XCTUnwrap(object["command"] as? [String: Any])

        XCTAssertEqual(Set(object.keys), [
            "version", "command_id", "session_generation", "deadline_millis", "command",
        ])
        XCTAssertEqual(Set(command.keys), [
            "kind", "session_id", "occupant_generation", "from_sequence", "columns", "rows",
        ])
        XCTAssertEqual(command["kind"] as? String, "attach")
        XCTAssertEqual(command["session_id"] as? String, sessionID.uuidString)
        XCTAssertEqual((command["from_sequence"] as? NSNumber)?.uint64Value, 9)
        XCTAssertEqual((command["columns"] as? NSNumber)?.uint32Value, 120)
        XCTAssertEqual((command["rows"] as? NSNumber)?.uint32Value, 40)
    }

    func testAttachCodecAcceptsLegacyInitialSessionGeneration() throws {
        XCTAssertNoThrow(try ControllerReadOnlyWireCodec.encodeAttach(
            commandID: UUID(),
            sessionGeneration: 0,
            deadlineMillis: 30_000,
            cursor: try makeReducer().cursor,
            viewport: TerminalViewportState(columns: 120, rows: 40)
        ))
    }

    func testAttachCodecDecodesBoundReplayBarrierAndOutput() throws {
        let commandID = UUID(uuidString: "10000000-0000-0000-0000-000000000001")!
        let identity = try makeReducer().cursor.identity
        let attached = try JSONSerialization.data(withJSONObject: [
            "kind": "attached",
            "command_id": commandID.uuidString,
            "session_id": sessionID.uuidString,
            "occupant_generation": 7,
            "replay_through_sequence": 12,
            "has_writer_lease": true,
        ])
        XCTAssertEqual(
            try ControllerReadOnlyWireCodec.decode(
                attached,
                commandID: commandID,
                identity: identity
            ),
            .attached(replayThroughSequence: 12, hasWriterLease: true)
        )

        let output = try JSONSerialization.data(withJSONObject: [
            "kind": "output",
            "session_id": sessionID.uuidString,
            "sequence": 10,
            "bytes": [65, 66, 67],
        ])
        XCTAssertEqual(
            try ControllerReadOnlyWireCodec.decode(
                output,
                commandID: commandID,
                identity: identity
            ),
            .output(frame(sequence: 10, bytes: Data("ABC".utf8)))
        )

        let snapshot = try JSONSerialization.data(withJSONObject: [
            "kind": "snapshot",
            "command_id": commandID.uuidString,
            "session_id": sessionID.uuidString,
            "boundary_sequence": 9,
            "columns": 120,
            "rows": 40,
            "chunk_index": 0,
            "chunk_count": 1,
            "bytes": [27, 91, 50, 74],
        ])
        XCTAssertEqual(
            try ControllerReadOnlyWireCodec.decode(
                snapshot,
                commandID: commandID,
                identity: identity
            ),
            .snapshot(TerminalSnapshotChunk(
                sessionID: sessionID,
                boundarySequence: 9,
                viewport: TerminalViewportState(columns: 120, rows: 40),
                chunkIndex: 0,
                chunkCount: 1,
                bytes: Data([27, 91, 50, 74])
            ))
        )
    }

    func testAttachCodecRejectsCrossSessionAndUnknownFields() throws {
        let commandID = UUID(uuidString: "10000000-0000-0000-0000-000000000001")!
        let identity = try makeReducer().cursor.identity
        let payload = try JSONSerialization.data(withJSONObject: [
            "kind": "output",
            "session_id": "00000000-0000-0000-0000-000000000099",
            "sequence": 1,
            "bytes": [65],
            "unexpected": true,
        ])

        XCTAssertThrowsError(
            try ControllerReadOnlyWireCodec.decode(
                payload,
                commandID: commandID,
                identity: identity
            )
        ) { error in
            XCTAssertEqual(error as? ControllerReadOnlyWireError, .malformedResponse)
        }
    }

    func testLimitsEnforceViewportAndFrameBoundaries() throws {
        let limits = TerminalLimits.controllerDefault
        XCTAssertNoThrow(try limits.validate(viewport: TerminalViewportState(columns: 400, rows: 200)))
        XCTAssertThrowsError(
            try limits.validate(viewport: TerminalViewportState(columns: 401, rows: 200))
        ) { error in
            XCTAssertEqual(error as? ReadOnlyAttachFailure, .invalidViewport)
        }

        var queue = try BoundedTerminalFrameQueue(limits: limits)
        XCTAssertThrowsError(
            try queue.enqueue(frame(sequence: 1, bytes: Data(count: limits.maxFrameBytes + 1)))
        ) { error in
            XCTAssertEqual(error as? ReadOnlyAttachFailure, .frameTooLarge)
        }
    }

    func testQueueRejectsSixtyFifthFrameWithoutDroppingAcceptedOutput() throws {
        var queue = try BoundedTerminalFrameQueue()
        for sequence in 1...64 {
            try queue.enqueue(frame(sequence: UInt64(sequence), bytes: Data([UInt8(sequence)])))
        }

        XCTAssertEqual(queue.count, 64)
        XCTAssertEqual(queue.queuedBytes, 64)
        XCTAssertThrowsError(try queue.enqueue(frame(sequence: 65, bytes: Data([65])))) { error in
            XCTAssertEqual(error as? ReadOnlyAttachFailure, .queueFull)
        }
        XCTAssertEqual(queue.count, 64)
        XCTAssertEqual(queue.dequeue()?.sequence, 1)
        XCTAssertEqual(queue.count, 63)
    }

    func testQueueEnforcesAggregateByteBudget() throws {
        let limits = TerminalLimits(maxFrameBytes: 3, maxQueuedFrameBytes: 4)
        var queue = try BoundedTerminalFrameQueue(limits: limits)
        try queue.enqueue(frame(sequence: 1, bytes: Data([1, 2, 3])))

        XCTAssertThrowsError(try queue.enqueue(frame(sequence: 2, bytes: Data([4, 5])))) { error in
            XCTAssertEqual(error as? ReadOnlyAttachFailure, .queueBytesExceeded)
        }
        XCTAssertEqual(queue.queuedBytes, 3)
    }

    func testReducerDeliversContiguousOutputAndAcceptsOnlyIdenticalDuplicate() throws {
        var reducer = try makeReducer()
        try reducer.beginAuthentication()
        try reducer.beginReplayWithoutSnapshot()
        let first = frame(sequence: 1, bytes: Data("one".utf8))

        XCTAssertEqual(try reducer.observe(first), .deliver)
        XCTAssertEqual(try reducer.observe(first), .duplicate)
        XCTAssertEqual(reducer.cursor.outputSequence, 1)
        XCTAssertThrowsError(
            try reducer.observe(frame(sequence: 1, bytes: Data("changed".utf8)))
        ) { error in
            XCTAssertEqual(error as? ReadOnlyAttachFailure, .conflictingDuplicate)
        }
        XCTAssertEqual(reducer.state, .failed(.conflictingDuplicate))
    }

    func testReducerStopsAtGapWithoutAdvancingCursor() throws {
        var reducer = try makeReducer(fromSequence: 8)
        try reducer.beginAuthentication()
        try reducer.beginReplayWithoutSnapshot()

        XCTAssertEqual(
            try reducer.observe(frame(sequence: 10, bytes: Data("ten".utf8))),
            .gap(expected: 9, received: 10)
        )
        XCTAssertEqual(reducer.state, .gap(expected: 9, received: 10))
        XCTAssertEqual(reducer.cursor.outputSequence, 8)
    }

    func testReducerRejectsOutputForAnotherSession() throws {
        var reducer = try makeReducer()
        try reducer.beginAuthentication()
        try reducer.beginReplayWithoutSnapshot()
        let foreign = TerminalOutputFrame(
            sessionID: UUID(uuidString: "00000000-0000-0000-0000-000000000099")!,
            sequence: 1,
            bytes: Data("foreign".utf8)
        )

        XCTAssertThrowsError(try reducer.observe(foreign)) { error in
            XCTAssertEqual(error as? ReadOnlyAttachFailure, .sessionMismatch)
        }
        XCTAssertEqual(reducer.state, .failed(.sessionMismatch))
    }

    func testSnapshotBoundaryResetsReplayCursor() throws {
        var reducer = try makeReducer(fromSequence: 4)
        try reducer.beginAuthentication()
        try reducer.beginSnapshot()
        try reducer.finishSnapshot(boundary: 20)

        XCTAssertEqual(reducer.state, .replaying)
        XCTAssertEqual(reducer.cursor.outputSequence, 20)
        XCTAssertEqual(
            try reducer.observe(frame(sequence: 21, bytes: Data("next".utf8))),
            .deliver
        )
        try reducer.markLive()
        XCTAssertEqual(reducer.state, .live)
    }

    func testSnapshotChunksMustBeOrderedAndBindReplayBarrier() throws {
        var reducer = try makeReducer(fromSequence: 4)
        try reducer.beginAuthentication()
        try reducer.beginSnapshot()
        let first = TerminalSnapshotChunk(
            sessionID: sessionID,
            boundarySequence: 20,
            viewport: TerminalViewportState(columns: 120, rows: 40),
            chunkIndex: 0,
            chunkCount: 2,
            bytes: Data("first".utf8)
        )
        let second = TerminalSnapshotChunk(
            sessionID: sessionID,
            boundarySequence: 20,
            viewport: TerminalViewportState(columns: 120, rows: 40),
            chunkIndex: 1,
            chunkCount: 2,
            bytes: Data("second".utf8)
        )

        XCTAssertFalse(try reducer.observeSnapshot(first))
        XCTAssertTrue(try reducer.observeSnapshot(second))
        try reducer.bindReplayBarrier(through: 22)
        XCTAssertEqual(reducer.state, .replaying)
        XCTAssertEqual(reducer.cursor.outputSequence, 20)
        XCTAssertEqual(
            try reducer.observe(frame(sequence: 21, bytes: Data("next".utf8))),
            .deliver
        )
    }

    func testReplayBarrierTransitionsLiveOnlyAfterBoundary() throws {
        var reducer = try makeReducer(fromSequence: 4)
        try reducer.beginAuthentication()
        try reducer.beginReplay(through: 6)

        XCTAssertEqual(reducer.state, .replaying)
        XCTAssertEqual(
            try reducer.observe(frame(sequence: 5, bytes: Data("five".utf8))),
            .deliver
        )
        XCTAssertEqual(reducer.state, .replaying)
        XCTAssertEqual(
            try reducer.observe(frame(sequence: 6, bytes: Data("six".utf8))),
            .deliver
        )
        XCTAssertEqual(reducer.state, .live)
    }

    private func makeReducer(fromSequence: UInt64 = 0) throws -> ReadOnlyAttachReducer {
        try ReadOnlyAttachReducer(
            identity: ReadOnlyAttachIdentity(
                hostID: "host-fingerprint",
                sessionID: sessionID,
                occupantGeneration: 7
            ),
            fromSequence: fromSequence
        )
    }

    private func frame(sequence: UInt64, bytes: Data) -> TerminalOutputFrame {
        TerminalOutputFrame(sessionID: sessionID, sequence: sequence, bytes: bytes)
    }

    private var sessionID: UUID {
        UUID(uuidString: "00000000-0000-0000-0000-000000000001")!
    }
}
