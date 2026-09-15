import Foundation
import XCTest
@testable import TermiRustMobile

final class TerminalConformanceV2Tests: XCTestCase {
    private struct Fixture: Decodable {
        let schemaVersion: Int
        let unicodeWidthVersion: String
        let styles: [Style]
        let cases: [Case]

        enum CodingKeys: String, CodingKey {
            case schemaVersion = "schema_version"
            case unicodeWidthVersion = "unicode_width_version"
            case styles, cases
        }
    }

    private struct Case: Decodable {
        let name: String
        let columns: Int
        let rows: Int
        let scrollback: Int
        let operations: [Operation]
        let expected: Expected
    }

    private struct Operation: Decodable {
        let kind: String
        let bytes: [UInt8]?
        let columns: Int?
        let rows: Int?
    }

    private struct Expected: Decodable {
        let lines: [String]
        let cells: [[Cell]]
        let cursorRow: Int
        let cursorColumn: Int

        enum CodingKeys: String, CodingKey {
            case lines, cells
            case cursorRow = "cursor_row"
            case cursorColumn = "cursor_column"
        }
    }

    private struct Cell: Decodable, Equatable {
        let text: String
        let width: Int
        let style: Int
    }

    private struct Style: Decodable {
        let foreground: Color
        let background: Color
        let bold: Bool
        let dim: Bool
        let italic: Bool
        let underline: Bool
        let inverse: Bool

        func matches(_ style: TerminalCellStyle) -> Bool {
            foreground.matches(style.foreground)
                && background.matches(style.background)
                && bold == style.bold
                && dim == style.dim
                && italic == style.italic
                && underline == style.underline
                && inverse == style.inverse
        }
    }

    private struct Color: Decodable {
        let kind: String
        let value: UInt8?
        let red: UInt8?
        let green: UInt8?
        let blue: UInt8?

        func matches(_ color: TerminalCellColor) -> Bool {
            switch (kind, color) {
            case ("default", .default): true
            case ("indexed", .indexed(let actual)): value == actual
            case ("rgb", .rgb(let actualRed, let actualGreen, let actualBlue)):
                red == actualRed && green == actualGreen && blue == actualBlue
            default: false
            }
        }
    }

    func testStyledUnicodeAndResizeConformanceAtEveryProcessSplit() throws {
        let fixture = try loadFixture()
        XCTAssertEqual(fixture.schemaVersion, 2)
        XCTAssertEqual(fixture.unicodeWidthVersion, GeneratedTerminalCellWidth.unicodeWidthVersion)

        for testCase in fixture.cases {
            try assertCase(testCase, styles: fixture.styles, split: nil)
            for (operationIndex, operation) in testCase.operations.enumerated()
                where operation.kind == "process" {
                let count = try XCTUnwrap(operation.bytes).count
                for split in 0...count {
                    try assertCase(
                        testCase,
                        styles: fixture.styles,
                        split: (operationIndex, split)
                    )
                }
            }
        }
    }

    private func assertCase(
        _ testCase: Case,
        styles: [Style],
        split: (operation: Int, byte: Int)?
    ) throws {
        var terminal = try BoundedTerminalBuffer(
            viewport: .init(columns: testCase.columns, rows: testCase.rows),
            limits: limits(scrollback: testCase.scrollback)
        )
        for (index, operation) in testCase.operations.enumerated() {
            switch operation.kind {
            case "process":
                let bytes = try XCTUnwrap(operation.bytes)
                if split?.operation == index {
                    let boundary = try XCTUnwrap(split?.byte)
                    try terminal.process(Data(bytes[..<boundary]))
                    try terminal.process(Data(bytes[boundary...]))
                } else {
                    try terminal.process(Data(bytes))
                }
            case "resize":
                try terminal.resize(.init(
                    columns: try XCTUnwrap(operation.columns),
                    rows: try XCTUnwrap(operation.rows)
                ))
            default:
                XCTFail("Unknown operation \(operation.kind)")
            }
        }

        let snapshot = terminal.snapshot()
        XCTAssertEqual(
            snapshot.lines.map(trimTrailingSpaces),
            testCase.expected.lines,
            testCase.name
        )
        XCTAssertEqual(snapshot.cursorRow, testCase.expected.cursorRow, testCase.name)
        XCTAssertEqual(snapshot.cursorColumn, testCase.expected.cursorColumn, testCase.name)
        let actual = snapshot.cells.map { row in
            row.map { cell in
                Cell(
                    text: cell.text,
                    width: cell.width.rawValue,
                    style: styles.firstIndex { $0.matches(cell.style) } ?? -1
                )
            }
        }
        XCTAssertEqual(actual, testCase.expected.cells, testCase.name)
    }

    private func limits(scrollback: Int) -> TerminalLimits {
        TerminalLimits(
            maxColumns: 400,
            maxRows: 200,
            maxFrameBytes: 1_048_576,
            maxQueuedFrames: 64,
            maxQueuedFrameBytes: 4_194_304,
            maxScrollbackRows: scrollback,
            maxRetainedCells: 1_000_000,
            maxGraphemeBytes: 16_777_216,
            maxStyleBytes: 8_388_608,
            maxParserCarryBytes: 4_194_304,
            maxModelBytes: 33_554_432
        )
    }

    private func trimTrailingSpaces(_ value: String) -> String {
        String(value.reversed().drop(while: { $0 == " " }).reversed())
    }

    private func loadFixture() throws -> Fixture {
        let url = try XCTUnwrap(
            Bundle(for: Self.self).url(
                forResource: "terminal-conformance-v2",
                withExtension: "json"
            )
        )
        return try JSONDecoder().decode(Fixture.self, from: Data(contentsOf: url))
    }
}
