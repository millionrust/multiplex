import Foundation
import XCTest
@testable import TermiRustMobile

final class TerminalInteractionTests: XCTestCase {
    private struct Fixture: Decodable {
        let schemaVersion: Int
        let limits: Limits
        let keyCases: [KeyCase]
        let pasteCases: [PasteCase]
        let selectionCases: [SelectionCase]
        let imeCases: [IMECase]
        let urlCases: [URLCase]

        enum CodingKeys: String, CodingKey {
            case schemaVersion = "schema_version"
            case limits
            case keyCases = "key_cases"
            case pasteCases = "paste_cases"
            case selectionCases = "selection_cases"
            case imeCases = "ime_cases"
            case urlCases = "url_cases"
        }
    }

    private struct Limits: Decodable {
        let maxPasteBytes: Int
        let pasteConfirmationBytes: Int
        let maxURLBytes: Int
        let maxURLs: Int

        enum CodingKeys: String, CodingKey {
            case maxPasteBytes = "max_paste_bytes"
            case pasteConfirmationBytes = "paste_confirmation_bytes"
            case maxURLBytes = "max_url_bytes"
            case maxURLs = "max_urls"
        }
    }

    private struct KeyCase: Decodable {
        let name: String
        let key: String
        let text: String?
        let shift: Bool?
        let control: Bool?
        let alt: Bool?
        let applicationCursor: Bool?
        let expected: [UInt8]

        enum CodingKeys: String, CodingKey {
            case name, key, text, shift, control, alt, expected
            case applicationCursor = "application_cursor"
        }
    }

    private struct PasteCase: Decodable {
        let name: String
        let input: String?
        let inputRepeat: Repeat?
        let bracketed: Bool
        let requiresConfirmation: Bool
        let expected: [UInt8]?

        enum CodingKeys: String, CodingKey {
            case name, input, bracketed, expected
            case inputRepeat = "input_repeat"
            case requiresConfirmation = "requires_confirmation"
        }
    }

    private struct Repeat: Decodable { let value: String; let count: Int }
    private struct Point: Decodable { let row: Int; let column: Int }
    private struct Cell: Decodable { let text: String; let width: Int }
    private struct SelectionCase: Decodable {
        let name: String
        let rows: [[Cell]]
        let start: Point
        let end: Point
        let expected: String
    }
    private struct IMECase: Decodable {
        let name: String
        let operations: [IMEOperation]
        let expectedEmissions: [[UInt8]]

        enum CodingKeys: String, CodingKey {
            case name, operations
            case expectedEmissions = "expected_emissions"
        }
    }
    private struct IMEOperation: Decodable { let kind: String; let text: String? }
    private struct URLCase: Decodable { let name: String; let text: String; let expected: [String] }

    func testCanonicalInteractionFixture() throws {
        let fixture = try loadFixture()
        XCTAssertEqual(fixture.schemaVersion, 1)
        XCTAssertEqual(fixture.limits.maxPasteBytes, TerminalInteraction.maxPasteBytes)
        XCTAssertEqual(
            fixture.limits.pasteConfirmationBytes,
            TerminalInteraction.pasteConfirmationBytes
        )
        XCTAssertEqual(fixture.limits.maxURLBytes, TerminalInteraction.maxURLBytes)
        XCTAssertEqual(fixture.limits.maxURLs, TerminalInteraction.maxURLs)

        for item in fixture.keyCases {
            let key = TerminalInputKey(rawValue: item.key) ?? .text
            XCTAssertEqual(
                TerminalInteraction.encode(
                    key,
                    text: item.text,
                    modifiers: .init(
                        shift: item.shift ?? false,
                        control: item.control ?? false,
                        alt: item.alt ?? false
                    ),
                    applicationCursor: item.applicationCursor ?? false
                ),
                Data(item.expected),
                item.name
            )
        }

        for item in fixture.pasteCases {
            let input = try pasteInput(item)
            let normalized = TerminalInteraction.normalizePaste(input)
            XCTAssertLessThanOrEqual(normalized.count, TerminalInteraction.maxPasteBytes, item.name)
            XCTAssertEqual(
                TerminalInteraction.pasteRequiresConfirmation(normalized),
                item.requiresConfirmation,
                item.name
            )
            if let expected = item.expected {
                XCTAssertEqual(
                    TerminalInteraction.preparePaste(normalized, bracketed: item.bracketed),
                    Data(expected),
                    item.name
                )
            }
        }

        for item in fixture.selectionCases {
            let rows = item.rows.map { row in
                row.map { cell in
                    BoundedTerminalCell(
                        text: cell.text,
                        width: TerminalCellWidth(rawValue: cell.width) ?? .narrow,
                        style: .init()
                    )
                }
            }
            XCTAssertEqual(
                TerminalInteraction.selectionText(
                    rows: rows,
                    start: .init(row: item.start.row, column: item.start.column),
                    end: .init(row: item.end.row, column: item.end.column)
                ),
                item.expected,
                item.name
            )
        }

        for item in fixture.imeCases {
            var state = TerminalIMEState()
            var emissions: [Data] = []
            for operation in item.operations {
                switch operation.kind {
                case "update": state.update(operation.text ?? "")
                case "cancel": state.cancel()
                case "commit":
                    if let bytes = state.commit(operation.text ?? "") { emissions.append(bytes) }
                case "finish":
                    if let bytes = state.finish() { emissions.append(bytes) }
                default: XCTFail("Unknown IME operation in \(item.name)")
                }
            }
            XCTAssertEqual(emissions, item.expectedEmissions.map { Data($0) }, item.name)
        }

        for item in fixture.urlCases {
            XCTAssertEqual(
                TerminalInteraction.visibleHTTPURLs(in: item.text).map(\.absoluteString),
                item.expected,
                item.name
            )
        }
    }

    func testSelectableContentExcludesPaddingAndOSCLinks() throws {
        var terminal = try BoundedTerminalBuffer(
            viewport: .init(columns: 40, rows: 2)
        )
        try terminal.process(Data("hi https://example.com\u{1B}]8;;https://evil.example\u{07}!".utf8))
        let snapshot = terminal.snapshot()
        XCTAssertEqual(snapshot.cells[0].count, 40)
        XCTAssertLessThan(snapshot.contentCells[0].count, snapshot.cells[0].count)
        XCTAssertEqual(
            TerminalInteraction.visibleHTTPURLs(
                in: snapshot.lines.joined(separator: "\n")
            ).map(\.absoluteString),
            ["https://example.com"]
        )
    }

    func testControllerFollowTargetStopsAtCursorOrLatestContentNotBlankPadding() {
        XCTAssertEqual(
            ControllerTerminalFollowTarget.row(
                lines: ["prompt", "", "", ""],
                cursorRow: 0,
                scrollbackRows: 0
            ),
            0
        )
        XCTAssertEqual(
            ControllerTerminalFollowTarget.row(
                lines: ["old", "new", "", ""],
                cursorRow: 1,
                scrollbackRows: 1
            ),
            2
        )
        XCTAssertEqual(
            ControllerTerminalFollowTarget.row(
                lines: ["prompt", "later output", "", ""],
                cursorRow: 0,
                scrollbackRows: 0
            ),
            1
        )
        XCTAssertNil(
            ControllerTerminalFollowTarget.row(
                lines: [],
                cursorRow: 0,
                scrollbackRows: 0
            )
        )
    }

    func testControllerCursorMapsViewportRowAndWideCellContinuation() {
        let cells = [
            BoundedTerminalCell(text: "界", width: .wide, style: .init()),
            BoundedTerminalCell.continuation(style: .init()),
        ]
        XCTAssertEqual(
            ControllerTerminalCursor.column(
                rowIndex: 4,
                cells: cells,
                cursorRow: 2,
                cursorColumn: 1,
                scrollbackRows: 2,
                visible: true
            ),
            0
        )
        XCTAssertEqual(
            ControllerTerminalCursor.column(
                rowIndex: 4,
                cells: cells,
                cursorRow: 2,
                cursorColumn: 5,
                scrollbackRows: 2,
                visible: true
            ),
            5
        )
        XCTAssertNil(
            ControllerTerminalCursor.column(
                rowIndex: 3,
                cells: cells,
                cursorRow: 2,
                cursorColumn: 1,
                scrollbackRows: 2,
                visible: true
            )
        )
        XCTAssertNil(
            ControllerTerminalCursor.column(
                rowIndex: 4,
                cells: cells,
                cursorRow: 2,
                cursorColumn: 1,
                scrollbackRows: 2,
                visible: false
            )
        )
    }

    func testControllerTerminalUsesCompactChromeOnlyForFocusedLandscape() {
        XCTAssertTrue(
            ControllerTerminalLayout.usesFocusedLandscape(
                verticalSizeClassIsCompact: true,
                keyboardPresented: true
            )
        )
        XCTAssertFalse(
            ControllerTerminalLayout.usesFocusedLandscape(
                verticalSizeClassIsCompact: false,
                keyboardPresented: true
            )
        )
        XCTAssertFalse(
            ControllerTerminalLayout.usesFocusedLandscape(
                verticalSizeClassIsCompact: true,
                keyboardPresented: false
            )
        )
    }

    func testControllerTerminalDesktopWidthPreservesAtLeastEightyColumns() {
        XCTAssertEqual(
            ControllerTerminalWidth.columns(fitting: 39, usesDesktopWidth: false),
            39
        )
        XCTAssertEqual(
            ControllerTerminalWidth.columns(fitting: 39, usesDesktopWidth: true),
            80
        )
        XCTAssertEqual(
            ControllerTerminalWidth.columns(fitting: 96, usesDesktopWidth: true),
            96
        )
    }

    private func pasteInput(_ item: PasteCase) throws -> String {
        if let input = item.input { return input }
        let repeated = try XCTUnwrap(item.inputRepeat)
        return String(repeating: repeated.value, count: repeated.count)
    }

    private func loadFixture() throws -> Fixture {
        let url = try XCTUnwrap(
            Bundle(for: Self.self).url(
                forResource: "terminal-interaction-v1",
                withExtension: "json"
            )
        )
        return try JSONDecoder().decode(Fixture.self, from: Data(contentsOf: url))
    }
}
