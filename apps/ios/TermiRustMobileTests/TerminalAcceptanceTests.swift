import Foundation
import XCTest
@testable import TermiRustMobile

final class TerminalAcceptanceTests: XCTestCase {
    private struct Fixture: Decodable {
        let schemaVersion: Int
        let limits: Limits
        let layoutCases: [LayoutCase]
        let accessibleOutputCases: [AccessibleOutputCase]
        let backgroundCases: [BackgroundCase]

        enum CodingKeys: String, CodingKey {
            case schemaVersion = "schema_version"
            case limits
            case layoutCases = "layout_cases"
            case accessibleOutputCases = "accessible_output_cases"
            case backgroundCases = "background_cases"
        }
    }

    private struct Limits: Decodable {
        let minimumFontSize: Double
        let maximumFontSize: Double
        let maximumAccessibilityCharacters: Int
        let iosMinimumTarget: Double

        enum CodingKeys: String, CodingKey {
            case minimumFontSize = "minimum_font_size"
            case maximumFontSize = "maximum_font_size"
            case maximumAccessibilityCharacters = "maximum_accessibility_characters"
            case iosMinimumTarget = "ios_minimum_target"
        }
    }

    private struct LayoutCase: Decodable {
        let name: String
        let width: Double
        let height: Double
        let requestedFontSize: Double
        let textScale: Double
        let expectedFontSize: Double
        let expectedColumns: Int
        let expectedRows: Int
        let expectedCompact: Bool

        enum CodingKeys: String, CodingKey {
            case name, width, height
            case requestedFontSize = "requested_font_size"
            case textScale = "text_scale"
            case expectedFontSize = "expected_font_size"
            case expectedColumns = "expected_columns"
            case expectedRows = "expected_rows"
            case expectedCompact = "expected_compact"
        }
    }

    private struct AccessibleOutputCase: Decodable {
        let name: String
        let lines: [String]
        let maximumCharacters: Int
        let expected: String

        enum CodingKeys: String, CodingKey {
            case name, lines, expected
            case maximumCharacters = "maximum_characters"
        }
    }

    private struct BackgroundCase: Decodable {
        let name: String
        let writerHeld: Bool
        let expectedReleaseWriter: Bool
        let expectedCoverPrivacy: Bool
        let expectedClearPendingInput: Bool
        let expectedClearPendingResize: Bool

        enum CodingKeys: String, CodingKey {
            case name
            case writerHeld = "writer_held"
            case expectedReleaseWriter = "expected_release_writer"
            case expectedCoverPrivacy = "expected_cover_privacy"
            case expectedClearPendingInput = "expected_clear_pending_input"
            case expectedClearPendingResize = "expected_clear_pending_resize"
        }
    }

    func testCanonicalAcceptanceFixture() throws {
        let fixture = try loadFixture()
        XCTAssertEqual(fixture.schemaVersion, 1)
        XCTAssertEqual(fixture.limits.minimumFontSize, TerminalAcceptance.minimumFontSize)
        XCTAssertEqual(fixture.limits.maximumFontSize, TerminalAcceptance.maximumFontSize)
        XCTAssertEqual(
            fixture.limits.maximumAccessibilityCharacters,
            TerminalAcceptance.maximumAccessibilityCharacters
        )
        XCTAssertEqual(fixture.limits.iosMinimumTarget, TerminalAcceptance.minimumTouchTarget)

        for item in fixture.layoutCases {
            let layout = TerminalAcceptance.layout(
                width: item.width,
                height: item.height,
                requestedFontSize: item.requestedFontSize,
                textScale: item.textScale
            )
            XCTAssertEqual(layout.fontSize, item.expectedFontSize, item.name)
            XCTAssertEqual(layout.columns, item.expectedColumns, item.name)
            XCTAssertEqual(layout.rows, item.expectedRows, item.name)
            XCTAssertEqual(layout.compactControls, item.expectedCompact, item.name)
        }
        for item in fixture.accessibleOutputCases {
            XCTAssertEqual(
                TerminalAcceptance.accessibleOutput(
                    lines: item.lines,
                    maximumCharacters: item.maximumCharacters
                ),
                item.expected,
                item.name
            )
        }
        for item in fixture.backgroundCases {
            let decision = TerminalAcceptance.backgroundDecision(writerHeld: item.writerHeld)
            XCTAssertEqual(decision.releaseWriter, item.expectedReleaseWriter, item.name)
            XCTAssertEqual(decision.coverPrivacy, item.expectedCoverPrivacy, item.name)
            XCTAssertEqual(decision.clearPendingInput, item.expectedClearPendingInput, item.name)
            XCTAssertEqual(decision.clearPendingResize, item.expectedClearPendingResize, item.name)
        }
    }

    private func loadFixture() throws -> Fixture {
        let url = try XCTUnwrap(
            Bundle(for: Self.self).url(
                forResource: "terminal-acceptance-v1",
                withExtension: "json"
            )
        )
        return try JSONDecoder().decode(Fixture.self, from: Data(contentsOf: url))
    }
}
