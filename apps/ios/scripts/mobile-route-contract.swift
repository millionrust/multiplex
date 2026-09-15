import Foundation

@main
private enum MobileRouteContractRunner {
    static func main() throws {
        guard CommandLine.arguments.count == 2 else { throw RunnerError("fixture path required") }
        let data = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
        let root = try object(JSONSerialization.jsonObject(with: data))
        try require(integer(root["schema_version"]) == 1, "schema version drift")
        for value in try array(root["routes"]) {
            let route = try object(value)
            _ = try projection(route)
        }
        for value in try array(root["invalid_cases"]) {
            let item = try object(value)
            do {
                _ = try projection(item)
                throw RunnerError("invalid case unexpectedly passed: \(try string(item["name"]))")
            } catch let error as MobileRouteContractError {
                try require(error.rawValue == string(item["expected_error"]), "wrong error: \(try string(item["name"]))")
            }
        }
        print("Swift mobile route contract v1 passed all canonical and invalid cases.")
    }

    private static func projection(_ item: [String: Any]) throws -> MobileRouteProjection {
        try .validated(
            itemKind: string(item["item_kind"]),
            credentialOwner: string(item["credential_owner"]),
            continuityOwner: string(item["continuity_owner"]),
            capabilities: array(item["capabilities"]).map { try string($0) },
            canOpenTerminal: boolean(item["can_open_terminal"])
        )
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
    private static func boolean(_ value: Any?) throws -> Bool {
        guard let value = value as? Bool else { throw RunnerError("expected boolean") }
        return value
    }
}

private struct RunnerError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}
