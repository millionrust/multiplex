import Foundation
import XCTest
@testable import TermiRustMobile

final class TerminalConformanceV1Tests: XCTestCase {
    private struct Fixture: Decodable {
        let schemaVersion: Int
        let cases: [Case]

        enum CodingKeys: String, CodingKey {
            case schemaVersion = "schema_version"
            case cases
        }
    }

    private struct Case: Decodable {
        let name: String
        let columns: Int
        let rows: Int
        let scrollback: Int
        let chunks: [[UInt8]]
        let expected: Expected
    }

    private struct Expected: Decodable, Equatable {
        let lines: [String]
        let cursorRow: Int
        let cursorColumn: Int
        let cursorVisible: Bool
        let applicationCursor: Bool
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
            case alternateScreen = "alternate_screen"
            case bracketedPaste = "bracketed_paste"
            case mouseMode = "mouse_mode"
            case mouseEncoding = "mouse_encoding"
            case scrollbackRows = "scrollback_rows"
        }
    }

    func testTerminalConformanceV1MatchesCanonicalFixtureAtEverySplit() throws {
        let fixture = try loadFixture()
        XCTAssertEqual(fixture.schemaVersion, 1)

        for testCase in fixture.cases {
            let configured = try render(testCase, chunks: testCase.chunks.map { Data($0) })
            XCTAssertEqual(configured, testCase.expected, testCase.name)

            let bytes = testCase.chunks.flatMap { $0 }
            for split in 0...bytes.count {
                let chunks = [Data(bytes[..<split]), Data(bytes[split...])]
                XCTAssertEqual(
                    try render(testCase, chunks: chunks),
                    testCase.expected,
                    "\(testCase.name) split at \(split)"
                )
            }
        }
    }

    private func render(_ testCase: Case, chunks: [Data]) throws -> Expected {
        let limits = TerminalLimits(
            maxColumns: 400,
            maxRows: 200,
            maxFrameBytes: 1_048_576,
            maxQueuedFrames: 64,
            maxQueuedFrameBytes: 4_194_304,
            maxScrollbackRows: testCase.scrollback,
            maxRetainedCells: 1_000_000,
            maxGraphemeBytes: 16_777_216,
            maxStyleBytes: 8_388_608,
            maxParserCarryBytes: 4_194_304,
            maxModelBytes: 33_554_432
        )
        var terminal = try BoundedTerminalBuffer(
            viewport: TerminalViewportState(columns: testCase.columns, rows: testCase.rows),
            limits: limits
        )
        for chunk in chunks { try terminal.process(chunk) }
        let snapshot = terminal.snapshot()
        return Expected(
            lines: snapshot.lines.map { $0.trimmingTrailingSpaces() },
            cursorRow: snapshot.cursorRow,
            cursorColumn: snapshot.cursorColumn,
            cursorVisible: snapshot.cursorVisible,
            applicationCursor: snapshot.applicationCursor,
            alternateScreen: snapshot.alternateScreen,
            bracketedPaste: snapshot.bracketedPaste,
            mouseMode: snapshot.mouseMode.rawValue,
            mouseEncoding: snapshot.mouseEncoding.rawValue,
            scrollbackRows: snapshot.scrollbackRows
        )
    }

    private func loadFixture() throws -> Fixture {
        let url = try XCTUnwrap(
            Bundle(for: Self.self).url(
                forResource: "terminal-conformance-v1",
                withExtension: "json"
            )
        )
        return try JSONDecoder().decode(Fixture.self, from: Data(contentsOf: url))
    }
}

private extension String {
    func trimmingTrailingSpaces() -> String {
        String(reversed().drop(while: { $0 == " " }).reversed())
    }
}
