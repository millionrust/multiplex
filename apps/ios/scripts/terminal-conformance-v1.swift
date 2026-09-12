import Foundation

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

@main
private struct TerminalConformanceRunner {
    static func main() throws {
        guard CommandLine.arguments.count == 2 else {
            throw RunnerError.usage
        }
        let data = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
        let fixture = try JSONDecoder().decode(Fixture.self, from: data)
        guard fixture.schemaVersion == 1 else { throw RunnerError.schema }

        for testCase in fixture.cases {
            try require(
                render(testCase, chunks: testCase.chunks.map { Data($0) }) == testCase.expected,
                "\(testCase.name) configured chunks"
            )
            let bytes = testCase.chunks.flatMap { $0 }
            for split in 0...bytes.count {
                try require(
                    render(
                        testCase,
                        chunks: [Data(bytes[..<split]), Data(bytes[split...])]
                    ) == testCase.expected,
                    "\(testCase.name) split at \(split)"
                )
            }
        }
        try verifyResourceLimits()
        print("Swift terminal-conformance-v1 passed \(fixture.cases.count) cases at every split.")
    }

    private static func render(_ testCase: Case, chunks: [Data]) throws -> Expected {
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
            lines: snapshot.lines.map(trimTrailingSpaces),
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

    private static func trimTrailingSpaces(_ value: String) -> String {
        String(value.reversed().drop(while: { $0 == " " }).reversed())
    }

    private static func verifyResourceLimits() throws {
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
        do {
            try terminal.process(Data(repeating: 65, count: 9))
            throw RunnerError.mismatch("oversized frame was accepted")
        } catch ReadOnlyAttachFailure.frameTooLarge {
            try require(terminal.snapshot().truncation == .frameLimit, "frame limit state")
        }

        try terminal.reset()
        try terminal.process(Data([0x1B, 0x5D, 65, 65, 65, 65, 65]))
        try require(
            terminal.snapshot().truncation == .parserCarryLimit,
            "parser carry limit state"
        )
    }

    private static func require(_ condition: Bool, _ context: String) throws {
        guard condition else { throw RunnerError.mismatch(context) }
    }
}

private enum RunnerError: Error, CustomStringConvertible {
    case usage
    case schema
    case mismatch(String)

    var description: String {
        switch self {
        case .usage: "usage: terminal-conformance-v1 <fixture.json>"
        case .schema: "terminal conformance fixture schema is not v1"
        case .mismatch(let context): "terminal conformance mismatch: \(context)"
        }
    }
}
