import Foundation

@main
private enum TerminalAcceptanceRunner {
    static func main() throws {
        guard CommandLine.arguments.count == 2 else { throw RunnerError("fixture path required") }
        let data = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
        let root = try object(try JSONSerialization.jsonObject(with: data))
        try require(integer(root["schema_version"]) == 1, "schema version drift")

        for value in try array(root["layout_cases"]) {
            let item = try object(value)
            let layout = TerminalAcceptance.layout(
                width: try number(item["width"]),
                height: try number(item["height"]),
                requestedFontSize: try number(item["requested_font_size"]),
                textScale: try number(item["text_scale"])
            )
            try require(layout.fontSize == number(item["expected_font_size"]), "font: \(string(item["name"]))")
            try require(layout.columns == integer(item["expected_columns"]), "columns: \(string(item["name"]))")
            try require(layout.rows == integer(item["expected_rows"]), "rows: \(string(item["name"]))")
            try require(layout.compactControls == boolean(item["expected_compact"]), "compact: \(string(item["name"]))")
        }

        let payload = Data(repeating: 65, count: 64 * 1_024)
        let clock = ContinuousClock()
        let started = clock.now
        for _ in 0..<3 {
            var terminal = try BoundedTerminalBuffer(viewport: .init(columns: 120, rows: 40))
            try terminal.process(payload)
            _ = terminal.snapshot()
        }
        let elapsed = started.duration(to: clock.now)
        try require(elapsed < .seconds(5), "terminal parser acceptance exceeded five seconds: \(elapsed)")
        print("Terminal acceptance fixture and bounded 192 KiB parser workload passed in \(elapsed).")
    }

    private static func require(_ condition: @autoclosure () throws -> Bool, _ message: @autoclosure () throws -> String) throws {
        if try !condition() { throw RunnerError(try message()) }
    }
    private static func object(_ value: Any?) throws -> [String: Any] {
        guard let value = value as? [String: Any] else { throw RunnerError("expected object") }
        return value
    }
    private static func array(_ value: Any?) throws -> [Any] {
        guard let value = value as? [Any] else { throw RunnerError("expected array") }
        return value
    }
    private static func string(_ value: Any?) throws -> String {
        guard let value = value as? String else { throw RunnerError("expected string") }
        return value
    }
    private static func integer(_ value: Any?) throws -> Int {
        guard let value = value as? NSNumber else { throw RunnerError("expected integer") }
        return value.intValue
    }
    private static func number(_ value: Any?) throws -> Double {
        guard let value = value as? NSNumber else { throw RunnerError("expected number") }
        return value.doubleValue
    }
    private static func boolean(_ value: Any?) throws -> Bool {
        guard let value = value as? Bool else { throw RunnerError("expected boolean") }
        return value
    }
}

private struct RunnerError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}
