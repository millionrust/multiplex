import Foundation
import XCTest
@testable import TermiRustMobile

final class MobileRouteContractTests: XCTestCase {
    func testCanonicalRoutesAndInvalidCombinations() throws {
        let fixture = try MobileRouteFixture.load(bundle: Bundle(for: Self.self))
        XCTAssertEqual(fixture.schemaVersion, 1)
        XCTAssertEqual(
            Set(fixture.capabilityVocabulary),
            Set(MobileRouteCapability.allCases.map(\.rawValue))
        )
        for item in fixture.routes {
            XCTAssertNoThrow(try item.projection(), item.id)
        }
        for item in fixture.invalidCases {
            XCTAssertThrowsError(try item.projection()) { error in
                XCTAssertEqual(
                    error as? MobileRouteContractError,
                    MobileRouteContractError(rawValue: item.expectedError),
                    item.name
                )
            }
        }
    }

    func testOnlyTerminalOwningKindsCreateTerminalDestinations() throws {
        let fixture = try MobileRouteFixture.load(bundle: Bundle(for: Self.self))
        for item in fixture.routes {
            let projection = try item.projection()
            if projection.canOpenTerminal {
                XCTAssertNoThrow(
                    try MobileTerminalDestination(
                        id: item.id,
                        title: item.displayKind,
                        badge: item.terminalBadge ?? "",
                        route: projection
                    )
                )
            } else {
                XCTAssertThrowsError(
                    try MobileTerminalDestination(
                        id: item.id,
                        title: item.displayKind,
                        badge: item.terminalBadge ?? "",
                        route: projection
                    )
                )
            }
        }
    }
}

private struct MobileRouteFixture: Decodable {
    let schemaVersion: Int
    let capabilityVocabulary: [String]
    let routes: [Route]
    let invalidCases: [InvalidCase]

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case capabilityVocabulary = "capability_vocabulary"
        case routes
        case invalidCases = "invalid_cases"
    }

    struct Route: Decodable {
        let id: String
        let itemKind: String
        let displayKind: String
        let terminalBadge: String?
        let credentialOwner: String
        let continuityOwner: String
        let capabilities: [String]
        let canOpenTerminal: Bool

        enum CodingKeys: String, CodingKey {
            case id, capabilities
            case itemKind = "item_kind"
            case displayKind = "display_kind"
            case terminalBadge = "terminal_badge"
            case credentialOwner = "credential_owner"
            case continuityOwner = "continuity_owner"
            case canOpenTerminal = "can_open_terminal"
        }

        func projection() throws -> MobileRouteProjection {
            try .validated(
                itemKind: itemKind,
                credentialOwner: credentialOwner,
                continuityOwner: continuityOwner,
                capabilities: capabilities,
                canOpenTerminal: canOpenTerminal
            )
        }
    }

    struct InvalidCase: Decodable {
        let name: String
        let itemKind: String
        let credentialOwner: String
        let continuityOwner: String
        let capabilities: [String]
        let canOpenTerminal: Bool
        let expectedError: String

        enum CodingKeys: String, CodingKey {
            case name, capabilities
            case itemKind = "item_kind"
            case credentialOwner = "credential_owner"
            case continuityOwner = "continuity_owner"
            case canOpenTerminal = "can_open_terminal"
            case expectedError = "expected_error"
        }

        func projection() throws -> MobileRouteProjection {
            try .validated(
                itemKind: itemKind,
                credentialOwner: credentialOwner,
                continuityOwner: continuityOwner,
                capabilities: capabilities,
                canOpenTerminal: canOpenTerminal
            )
        }
    }

    static func load(bundle: Bundle) throws -> MobileRouteFixture {
        let url = try XCTUnwrap(
            bundle.url(forResource: "mobile-route-contract-v1", withExtension: "json")
        )
        return try JSONDecoder().decode(Self.self, from: Data(contentsOf: url))
    }
}
