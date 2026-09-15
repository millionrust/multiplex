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

    func testProductionNativeEngineHandlesFullScreenEditing() throws {
        var terminal = try BoundedTerminalBuffer(
            viewport: TerminalViewportState(columns: 12, rows: 4)
        )
        try terminal.process(Data(
            "one\r\ntwo\r\nthree\u{001B}[2;1H\u{001B}[Linsert\u{001B}[3;1H\u{001B}[2@>>".utf8
        ))

        let snapshot = terminal.snapshot()
        XCTAssertEqual(Array(snapshot.lines.prefix(3)), ["one", "insert", ">>two"])
        XCTAssertEqual(snapshot.cursorRow, 2)
        XCTAssertEqual(snapshot.cursorColumn, 2)
    }

    func testProductionNativeEngineHandlesInteractiveFixtureAtEveryNetworkSplit() throws {
        let fixture = try loadInteractiveFixture()
        XCTAssertEqual(fixture.schemaVersion, 1)

        for testCase in fixture.cases {
            let input = Data(testCase.input.utf8)
            for split in 0 ... input.count {
                let terminal = try XCTUnwrap(NativeControllerTerminalSession(
                    viewport: TerminalViewportState(
                        columns: testCase.columns,
                        rows: testCase.rows
                    ),
                    limits: TerminalLimits(maxScrollbackRows: testCase.scrollback)
                ))
                try terminal.feed(input.prefix(split))
                try terminal.feed(input.dropFirst(split))

                let snapshot = try terminal.snapshot()
                let expected = testCase.expected
                XCTAssertEqual(snapshot.lines, expected.lines, "\(testCase.name), split \(split)")
                XCTAssertEqual(snapshot.cursorRow, expected.cursorRow, testCase.name)
                XCTAssertEqual(snapshot.cursorColumn, expected.cursorColumn, testCase.name)
                XCTAssertEqual(snapshot.cursorVisible, expected.cursorVisible, testCase.name)
                XCTAssertEqual(snapshot.applicationCursor, expected.applicationCursor, testCase.name)
                XCTAssertEqual(snapshot.applicationKeypad, expected.applicationKeypad, testCase.name)
                XCTAssertEqual(snapshot.alternateScreen, expected.alternateScreen, testCase.name)
                XCTAssertEqual(snapshot.bracketedPaste, expected.bracketedPaste, testCase.name)
                XCTAssertEqual(snapshot.mouseMode.rawValue, expected.mouseMode, testCase.name)
                XCTAssertEqual(snapshot.mouseEncoding.rawValue, expected.mouseEncoding, testCase.name)
                XCTAssertEqual(snapshot.scrollbackRows, expected.scrollbackRows, testCase.name)
            }
        }
    }

    private func loadInteractiveFixture() throws -> InteractiveFixture {
        let url = try XCTUnwrap(Bundle(for: Self.self).url(
            forResource: "terminal-interactive-v1",
            withExtension: "json"
        ))
        return try JSONDecoder().decode(InteractiveFixture.self, from: Data(contentsOf: url))
    }
}

private struct InteractiveFixture: Decodable {
    let schemaVersion: Int
    let cases: [InteractiveCase]

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case cases
    }
}

private struct InteractiveCase: Decodable {
    let name: String
    let columns: Int
    let rows: Int
    let scrollback: Int
    let input: String
    let expected: InteractiveExpected
}

private struct InteractiveExpected: Decodable {
    let lines: [String]
    let cursorRow: Int
    let cursorColumn: Int
    let cursorVisible: Bool
    let applicationCursor: Bool
    let applicationKeypad: Bool
    let alternateScreen: Bool
    let bracketedPaste: Bool
    let mouseMode: String
    let mouseEncoding: String
    let scrollbackRows: Int

    enum CodingKeys: String, CodingKey {
        case lines
        case cursorRow = "cursor_row"
        case cursorColumn = "cursor_column"
        case cursorVisible = "cursor_visible"
        case applicationCursor = "application_cursor"
        case applicationKeypad = "application_keypad"
        case alternateScreen = "alternate_screen"
        case bracketedPaste = "bracketed_paste"
        case mouseMode = "mouse_mode"
        case mouseEncoding = "mouse_encoding"
        case scrollbackRows = "scrollback_rows"
    }
}
