import Foundation

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

private struct Expected: Decodable, Equatable {
    let lines: [String]
    let cells: [[Cell]]
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
        case lines, cells
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

@main
private struct TerminalConformanceV2Runner {
    static func main() throws {
        guard CommandLine.arguments.count == 2 else { throw RunnerError.usage }
        let data = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
        let fixture = try JSONDecoder().decode(Fixture.self, from: data)
        try require(fixture.schemaVersion == 2, "fixture schema")
        try require(
            fixture.unicodeWidthVersion == GeneratedTerminalCellWidth.unicodeWidthVersion,
            "Unicode width version"
        )

        for testCase in fixture.cases {
            try require(
                try render(testCase, styles: fixture.styles, split: nil) == testCase.expected,
                "\(testCase.name) configured operations"
            )
            for (operationIndex, operation) in testCase.operations.enumerated()
                where operation.kind == "process" {
                guard let bytes = operation.bytes else { throw RunnerError.invalidFixture }
                for split in 0...bytes.count {
                    try require(
                        try render(
                            testCase,
                            styles: fixture.styles,
                            split: (operationIndex, split)
                        ) == testCase.expected,
                        "\(testCase.name) operation \(operationIndex) split at \(split)"
                    )
                }
            }
        }
        print("Swift terminal-conformance-v2 passed \(fixture.cases.count) cases at every process split.")
    }

    private static func render(
        _ testCase: Case,
        styles: [Style],
        split: (operation: Int, byte: Int)?
    ) throws -> Expected {
        var terminal = try BoundedTerminalBuffer(
            viewport: .init(columns: testCase.columns, rows: testCase.rows),
            limits: limits(scrollback: testCase.scrollback)
        )
        for (index, operation) in testCase.operations.enumerated() {
            switch operation.kind {
            case "process":
                guard let bytes = operation.bytes else { throw RunnerError.invalidFixture }
                if split?.operation == index, let boundary = split?.byte {
                    try terminal.process(Data(bytes[..<boundary]))
                    try terminal.process(Data(bytes[boundary...]))
                } else {
                    try terminal.process(Data(bytes))
                }
            case "resize":
                guard let columns = operation.columns, let rows = operation.rows else {
                    throw RunnerError.invalidFixture
                }
                try terminal.resize(.init(columns: columns, rows: rows))
            default:
                throw RunnerError.invalidFixture
            }
        }
        let snapshot = terminal.snapshot()
        return Expected(
            lines: snapshot.lines.map(trimTrailingSpaces),
            cells: snapshot.cells.map { row in
                row.map { cell in
                    Cell(
                        text: cell.text,
                        width: cell.width.rawValue,
                        style: styles.firstIndex { $0.matches(cell.style) } ?? -1
                    )
                }
            },
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

    private static func limits(scrollback: Int) -> TerminalLimits {
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

    private static func trimTrailingSpaces(_ value: String) -> String {
        String(value.reversed().drop(while: { $0 == " " }).reversed())
    }

    private static func require(_ condition: Bool, _ context: String) throws {
        guard condition else { throw RunnerError.mismatch(context) }
    }
}

private enum RunnerError: Error, CustomStringConvertible {
    case usage
    case invalidFixture
    case mismatch(String)

    var description: String {
        switch self {
        case .usage: "usage: terminal-conformance-v2 <fixture.json>"
        case .invalidFixture: "terminal conformance v2 fixture is invalid"
        case .mismatch(let context): "terminal conformance mismatch: \(context)"
        }
    }
}
