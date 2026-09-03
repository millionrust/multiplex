import Foundation
import XCTest
@testable import TermiRustMobile

final class BoundedTerminalBufferTests: XCTestCase {
    func testSplitUTF8AndCursorControlsAreIncremental() throws {
        var terminal = try BoundedTerminalBuffer(
            viewport: TerminalViewportState(columns: 20, rows: 4)
        )
        let value = Array("AéB".utf8)
        try terminal.process(Data(value.prefix(2)))
        try terminal.process(Data(value.dropFirst(2)))
        try terminal.process(Data("\rZ\u{001B}[2CX".utf8))

        let snapshot = terminal.snapshot()
        XCTAssertEqual(snapshot.lines[0], "ZéBX")
        XCTAssertEqual(snapshot.cursorColumn, 4)
    }

    func testOSCIsInertAndDoesNotReachRenderedText() throws {
        var terminal = try BoundedTerminalBuffer(
            viewport: TerminalViewportState(columns: 40, rows: 4)
        )
        try terminal.process(Data("safe\u{001B}]52;c;secret\u{0007}visible".utf8))

        XCTAssertEqual(terminal.snapshot().lines[0], "safevisible")
    }

    func testEvictionDropsOnlyCompleteScrollbackRows() throws {
        let limits = TerminalLimits(
            maxColumns: 20,
            maxRows: 4,
            maxFrameBytes: 1_024,
            maxQueuedFrames: 4,
            maxQueuedFrameBytes: 4_096,
            maxScrollbackRows: 2,
            maxRetainedCells: 1_000,
            maxGraphemeBytes: 8_192,
            maxStyleBytes: 8_192,
            maxParserCarryBytes: 1_024,
            maxModelBytes: 32_768
        )
        var terminal = try BoundedTerminalBuffer(
            viewport: TerminalViewportState(columns: 20, rows: 2),
            limits: limits
        )
        try terminal.process(Data("one\r\ntwo\r\nthree\r\nfour\r\nfive".utf8))

        let lines = terminal.snapshot().lines
        XCTAssertEqual(lines.count, 4)
        XCTAssertEqual(lines.suffix(2), ["four", "five"])
    }

    func testLineFeedPreservesCursorColumnWithoutCarriageReturn() throws {
        var terminal = try BoundedTerminalBuffer(
            viewport: TerminalViewportState(columns: 20, rows: 2)
        )

        try terminal.process(Data("one\ntwo".utf8))

        XCTAssertEqual(terminal.snapshot().lines, ["one", "   two"])
    }

    func testFrameAndParserCarryCapsFailClosed() throws {
        let limits = TerminalLimits(
            maxColumns: 20,
            maxRows: 4,
            maxFrameBytes: 8,
            maxQueuedFrames: 2,
            maxQueuedFrameBytes: 16,
            maxScrollbackRows: 4,
            maxRetainedCells: 100,
            maxGraphemeBytes: 1_024,
            maxStyleBytes: 1_024,
            maxParserCarryBytes: 4,
            maxModelBytes: 4_096
        )
        var terminal = try BoundedTerminalBuffer(
            viewport: TerminalViewportState(columns: 20, rows: 2),
            limits: limits
        )
        XCTAssertThrowsError(try terminal.process(Data(repeating: 65, count: 9)))
        XCTAssertEqual(terminal.snapshot().truncation, .frameLimit)

        try terminal.reset()
        try terminal.process(Data([0x1B, 0x5D, 65, 65, 65, 65, 65]))
        XCTAssertEqual(terminal.snapshot().truncation, .parserCarryLimit)
        XCTAssertTrue(terminal.snapshot().lines.allSatisfy(\.isEmpty))
    }
}
