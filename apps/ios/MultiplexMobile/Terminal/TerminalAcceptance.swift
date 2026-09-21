import Foundation

struct TerminalAcceptanceLayout: Equatable, Sendable {
    let fontSize: Double
    let displayedFontSize: Double
    let columns: Int
    let rows: Int
    let compactControls: Bool
}

struct TerminalBackgroundDecision: Equatable, Sendable {
    let releaseWriter: Bool
    let coverPrivacy: Bool
    let clearPendingInput: Bool
    let clearPendingResize: Bool
}

enum TerminalAcceptance {
    static let minimumFontSize = 10.0
    static let maximumFontSize = 32.0
    static let maximumAccessibilityCharacters = 4_096
    static let minimumTouchTarget = 44.0

    static func layout(
        width: Double,
        height: Double,
        requestedFontSize: Double,
        textScale: Double
    ) -> TerminalAcceptanceLayout {
        let fontSize = requestedFontSize.clamped(to: minimumFontSize...maximumFontSize)
        let scale = max(textScale, 1)
        let displayedFontSize = fontSize * scale
        let usableWidth = max(width - 24, 1)
        let usableHeight = max(height - 24, 1)
        let columns = Int((usableWidth / (displayedFontSize * 0.62)).rounded(.down))
            .clamped(to: 20...400)
        let rows = Int((usableHeight / (displayedFontSize * 1.35)).rounded(.down))
            .clamped(to: 5...200)
        return TerminalAcceptanceLayout(
            fontSize: fontSize,
            displayedFontSize: displayedFontSize,
            columns: columns,
            rows: rows,
            compactControls: width < 600 || scale >= 1.6
        )
    }

    static func accessibleOutput(
        lines: [String],
        maximumCharacters: Int = maximumAccessibilityCharacters
    ) -> String {
        guard maximumCharacters > 0 else { return "" }
        let output = lines.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
        guard output.count > maximumCharacters else { return output }
        return String(output.suffix(maximumCharacters))
    }

    static func backgroundDecision(writerHeld: Bool) -> TerminalBackgroundDecision {
        TerminalBackgroundDecision(
            releaseWriter: writerHeld,
            coverPrivacy: true,
            clearPendingInput: true,
            clearPendingResize: true
        )
    }
}

private extension Comparable {
    func clamped(to range: ClosedRange<Self>) -> Self {
        min(max(self, range.lowerBound), range.upperBound)
    }
}
