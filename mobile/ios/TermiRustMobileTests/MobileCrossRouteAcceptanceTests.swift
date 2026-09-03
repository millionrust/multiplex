import Foundation
import XCTest
@testable import TermiRustMobile

final class MobileCrossRouteAcceptanceTests: XCTestCase {
    func testCanonicalCrossRouteDecisions() throws {
        let fixture = try CrossRouteFixture.load(bundle: Bundle(for: Self.self))
        XCTAssertEqual(fixture.schemaVersion, 1)
        XCTAssertGreaterThanOrEqual(fixture.cases.count, 15)

        for item in fixture.cases {
            XCTAssertEqual(try item.decision(), item.expected, item.name)
            XCTAssertFalse(item.expected.replayTerminalInput, item.name)
        }
    }
}

private struct CrossRouteFixture: Decodable {
    let schemaVersion: Int
    let cases: [Case]

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case cases
    }

    struct Case: Decodable {
        let name: String
        let route: String
        let event: String
        let tmuxEnabled: Bool
        let tmuxAvailable: Bool
        let hostKeyMatches: Bool
        let authorityValid: Bool
        let writerHeld: Bool
        let pendingInput: Bool
        let expected: MobileCrossRouteDecision

        enum CodingKeys: String, CodingKey {
            case name, route, event, expected
            case tmuxEnabled = "tmux_enabled"
            case tmuxAvailable = "tmux_available"
            case hostKeyMatches = "host_key_matches"
            case authorityValid = "authority_valid"
            case writerHeld = "writer_held"
            case pendingInput = "pending_input"
        }

        func decision() throws -> MobileCrossRouteDecision {
            try MobileCrossRouteAcceptance.decide(
                route: try XCTUnwrap(MobileTerminalRoute(rawValue: route)),
                event: try XCTUnwrap(MobileRouteEvent(rawValue: event)),
                tmuxEnabled: tmuxEnabled,
                tmuxAvailable: tmuxAvailable,
                hostKeyMatches: hostKeyMatches,
                authorityValid: authorityValid,
                writerHeld: writerHeld,
                pendingInput: pendingInput
            )
        }
    }

    static func load(bundle: Bundle) throws -> CrossRouteFixture {
        let url = try XCTUnwrap(
            bundle.url(forResource: "mobile-cross-route-acceptance-v1", withExtension: "json")
        )
        return try JSONDecoder().decode(Self.self, from: Data(contentsOf: url))
    }
}
