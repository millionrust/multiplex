import Foundation

@main
private enum TerminalInteractionRunner {
    static func main() throws {
        guard CommandLine.arguments.count == 2 else {
            throw RunnerError.message("usage: terminal-interaction <fixture.json>")
        }
        let data = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
        let root = try object(try JSONSerialization.jsonObject(with: data))
        try require(integer(root["schema_version"]) == 1, "unexpected schema version")
        let limits = try object(root["limits"])
        try require(integer(limits["max_paste_bytes"]) == TerminalInteraction.maxPasteBytes, "paste limit drift")
        try require(integer(limits["paste_confirmation_bytes"]) == TerminalInteraction.pasteConfirmationBytes, "paste confirmation drift")
        try require(integer(limits["max_url_bytes"]) == TerminalInteraction.maxURLBytes, "URL byte limit drift")
        try require(integer(limits["max_urls"]) == TerminalInteraction.maxURLs, "URL count drift")

        for value in try array(root["key_cases"]) {
            let item = try object(value)
            let name = try string(item["name"])
            let key = TerminalInputKey(rawValue: try string(item["key"])) ?? .text
            let actual = TerminalInteraction.encode(
                key,
                text: optionalString(item["text"]),
                modifiers: .init(
                    shift: boolean(item["shift"]),
                    control: boolean(item["control"]),
                    alt: boolean(item["alt"])
                ),
                applicationCursor: boolean(item["application_cursor"])
            )
            let expected = Data(try bytes(item["expected"]))
            try require(actual == expected, "\(name): key bytes differ")
        }

        for value in try array(root["paste_cases"]) {
            let item = try object(value)
            let name = try string(item["name"])
            let input: String
            if let value = optionalString(item["input"]) {
                input = value
            } else {
                let repeatValue = try object(item["input_repeat"])
                input = String(
                    repeating: try string(repeatValue["value"]),
                    count: integer(repeatValue["count"])
                )
            }
            let normalized = TerminalInteraction.normalizePaste(input)
            try require(
                TerminalInteraction.pasteRequiresConfirmation(normalized) == boolean(item["requires_confirmation"]),
                "\(name): paste classification differs"
            )
            if item["expected"] != nil {
                let expected = Data(try bytes(item["expected"]))
                try require(
                    TerminalInteraction.preparePaste(normalized, bracketed: boolean(item["bracketed"])) == expected,
                    "\(name): paste bytes differ"
                )
            }
        }

        for value in try array(root["selection_cases"]) {
            let item = try object(value)
            let rows = try array(item["rows"]).map { rowValue in
                try array(rowValue).map { cellValue in
                    let cell = try object(cellValue)
                    return BoundedTerminalCell(
                        text: try string(cell["text"]),
                        width: TerminalCellWidth(rawValue: integer(cell["width"])) ?? .narrow,
                        style: .init()
                    )
                }
            }
            let start = try point(item["start"])
            let end = try point(item["end"])
            let expected = try string(item["expected"])
            let name = try string(item["name"])
            try require(
                TerminalInteraction.selectionText(rows: rows, start: start, end: end) == expected,
                "\(name): selection differs"
            )
        }

        for value in try array(root["ime_cases"]) {
            let item = try object(value)
            var state = TerminalIMEState()
            var emissions: [Data] = []
            for operationValue in try array(item["operations"]) {
                let operation = try object(operationValue)
                switch try string(operation["kind"]) {
                case "update": state.update(optionalString(operation["text"]) ?? "")
                case "cancel": state.cancel()
                case "commit":
                    if let bytes = state.commit(optionalString(operation["text"]) ?? "") { emissions.append(bytes) }
                case "finish":
                    if let bytes = state.finish() { emissions.append(bytes) }
                default: throw RunnerError.message("unknown IME operation")
                }
            }
            let expected = try array(item["expected_emissions"]).map { Data(try bytes($0)) }
            try require(emissions == expected, "\(try string(item["name"])): IME emissions differ")
        }

        for value in try array(root["url_cases"]) {
            let item = try object(value)
            let actual = TerminalInteraction.visibleHTTPURLs(in: try string(item["text"])).map(\.absoluteString)
            let expected = try array(item["expected"]).map(string)
            try require(actual == expected, "\(try string(item["name"])): URLs differ")
        }
        print("Swift terminal-interaction-v1 passed all canonical cases.")
    }

    private static func point(_ value: Any?) throws -> TerminalSelectionPoint {
        let value = try object(value)
        return .init(row: integer(value["row"]), column: integer(value["column"]))
    }

    private static func object(_ value: Any?) throws -> [String: Any] {
        guard let value = value as? [String: Any] else { throw RunnerError.message("expected object") }
        return value
    }

    private static func array(_ value: Any?) throws -> [Any] {
        guard let value = value as? [Any] else { throw RunnerError.message("expected array") }
        return value
    }

    private static func string(_ value: Any?) throws -> String {
        guard let value = value as? String else { throw RunnerError.message("expected string") }
        return value
    }

    private static func optionalString(_ value: Any?) -> String? { value as? String }
    private static func integer(_ value: Any?) -> Int { (value as? NSNumber)?.intValue ?? -1 }
    private static func boolean(_ value: Any?) -> Bool { (value as? NSNumber)?.boolValue ?? false }
    private static func bytes(_ value: Any?) throws -> [UInt8] {
        try array(value).map { UInt8(integer($0)) }
    }

    private static func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        if !condition() { throw RunnerError.message(message) }
    }
}

private enum RunnerError: Error, CustomStringConvertible {
    case message(String)
    var description: String {
        switch self { case .message(let value): value }
    }
}
