import XCTest

@testable import TermiRustMobile

/// The phone's side of a screen session: what it asks for, what it accepts back, and what it
/// refuses to send on a device that was never granted it.
final class ControllerScreenSessionTests: XCTestCase {
    private func ticketResponse(
        commandId: UUID = UUID(),
        ticket: [Int] = Array(repeating: 7, count: 32),
        pointer: Bool = true,
        keyboard: Bool = false,
        extra: [String: Any] = [:]
    ) throws -> Data {
        var object: [String: Any] = [
            "kind": "screen_opened",
            "command_id": commandId.uuidString,
            "ticket": ticket,
            "can_control_pointer": pointer,
            "can_control_keyboard": keyboard,
        ]
        object.merge(extra) { _, new in new }
        return try JSONSerialization.data(withJSONObject: object)
    }

    func testOpenScreenAsksForASessionAndReadsItsTicket() throws {
        XCTAssertEqual(ControllerScreenCommand.openBody()["kind"] as? String, "open_screen")
        XCTAssertEqual(ControllerScreenCommand.closeBody()["kind"] as? String, "close_screen")

        let commandId = UUID()
        let opened = try ControllerScreenResponse.ticket(
            from: try ticketResponse(commandId: commandId, pointer: true, keyboard: false)
        )
        XCTAssertEqual(opened.commandId, commandId)
        XCTAssertEqual(opened.ticket.count, 32)
        XCTAssertTrue(opened.canControlPointer)
        XCTAssertFalse(opened.canControlKeyboard)
        XCTAssertTrue(opened.canControl)
    }

    func testAResponseThatIsNotAScreenTicketIsRefused() throws {
        // A ticket is exactly 32 bytes.
        XCTAssertThrowsError(
            try ControllerScreenResponse.ticket(
                from: try ticketResponse(ticket: Array(repeating: 7, count: 31))
            )
        ) { error in
            XCTAssertEqual(error as? ControllerScreenError, .invalidTicket)
        }
        // Values outside a byte are not a ticket either.
        XCTAssertThrowsError(
            try ControllerScreenResponse.ticket(
                from: try ticketResponse(ticket: Array(repeating: 300, count: 32))
            )
        ) { error in
            XCTAssertEqual(error as? ControllerScreenError, .invalidTicket)
        }
        // Unexpected fields mean this is not the message it claims to be.
        XCTAssertThrowsError(
            try ControllerScreenResponse.ticket(from: try ticketResponse(extra: ["surplus": 1]))
        ) { error in
            XCTAssertEqual(error as? ControllerScreenError, .malformedResponse)
        }
        let wrongKind = try JSONSerialization.data(withJSONObject: [
            "kind": "completed", "command_id": UUID().uuidString, "applied": true,
        ])
        XCTAssertThrowsError(try ControllerScreenResponse.ticket(from: wrongKind)) { error in
            XCTAssertEqual(error as? ControllerScreenError, .malformedResponse)
        }
    }

    func testEachChunkClaimsTheCapabilityItsContentsNeed() {
        XCTAssertEqual(ControllerScreenPump.capability(for: .observe), .observeScreens)
        XCTAssertEqual(ControllerScreenPump.capability(for: .pointer), .controlPointer)
        XCTAssertEqual(ControllerScreenPump.capability(for: .keyboard), .controlKeyboard)
    }

    func testAPhoneDoesNotSendInputItWasNeverGranted() throws {
        let pointerOnly = ControllerScreenPump(
            ticket: ControllerScreenTicket(
                commandId: UUID(),
                ticket: Data(repeating: 7, count: 32),
                canControlPointer: true,
                canControlKeyboard: false
            )
        )
        XCTAssertTrue(pointerOnly.allows(.observe))
        XCTAssertTrue(pointerOnly.allows(.pointer))
        XCTAssertFalse(pointerOnly.allows(.keyboard), "the keyboard was never granted")

        let viewer = ScreenViewer(cacheBytes: 1 << 20)
        try viewer.connect(ticket: pointerOnly.ticket.ticket)
        viewer.subscribe(surface: 1, preview: false)
        viewer.sendPointerMove(surface: 1, x: 4, y: 8)
        let frames = try pointerOnly.drain(viewer)
        XCTAssertEqual(frames.map(\.0), [.observeScreens, .observeScreens, .controlPointer])
        XCTAssertTrue(frames.allSatisfy { !$0.1.isEmpty })

        // Typing stops here rather than at the computer, which would end the session.
        viewer.sendText(surface: 1, text: "ls\n")
        XCTAssertThrowsError(try pointerOnly.drain(viewer)) { error in
            XCTAssertEqual(error as? ControllerScreenError, .notGranted)
        }
    }

    func testAWatcherWithNoControlSendsOnlyWatchingTraffic() throws {
        let watcher = ControllerScreenPump(
            ticket: ControllerScreenTicket(
                commandId: UUID(),
                ticket: Data(repeating: 9, count: 32),
                canControlPointer: false,
                canControlKeyboard: false
            )
        )
        XCTAssertFalse(watcher.ticket.canControl)
        let viewer = ScreenViewer(cacheBytes: 1 << 20)
        try viewer.connect(ticket: watcher.ticket.ticket)
        XCTAssertEqual(try watcher.drain(viewer).map(\.0), [.observeScreens])

        viewer.sendPointerMove(surface: 1, x: 1, y: 1)
        XCTAssertThrowsError(try watcher.drain(viewer)) { error in
            XCTAssertEqual(error as? ControllerScreenError, .notGranted)
        }
    }
}
