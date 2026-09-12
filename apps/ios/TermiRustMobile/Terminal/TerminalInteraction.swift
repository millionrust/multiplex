import Foundation

enum TerminalInputKey: String, Sendable {
    case text, space, enter, backspace, tab, escape
    case up, down, left, right, home, end, insert, delete, pageup, pagedown
}

struct TerminalInputModifiers: Equatable, Sendable {
    var shift = false
    var control = false
    var alt = false
}

struct TerminalSelectionPoint: Equatable, Sendable {
    let row: Int
    let column: Int
}

enum TerminalInteraction {
    static let maxPasteBytes = 256 * 1_024
    static let pasteConfirmationBytes = 4 * 1_024
    static let maxURLBytes = 2_048
    static let maxURLs = 32
    private static let bracketStart = Data([0x1B, 0x5B, 0x32, 0x30, 0x30, 0x7E])
    private static let bracketEnd = Data([0x1B, 0x5B, 0x32, 0x30, 0x31, 0x7E])

    static func encode(
        _ key: TerminalInputKey,
        text: String? = nil,
        modifiers: TerminalInputModifiers = .init(),
        applicationCursor: Bool = false
    ) -> Data? {
        let bytes: Data
        if modifiers.control {
            guard let byte = controlByte(key: key, text: text) else { return nil }
            bytes = Data([byte])
        } else {
            switch key {
            case .enter: bytes = Data([0x0D])
            case .backspace: bytes = Data([0x7F])
            case .tab where modifiers.shift: bytes = Data([0x1B, 0x5B, 0x5A])
            case .tab: bytes = Data([0x09])
            case .escape: bytes = Data([0x1B])
            case .up: bytes = cursor(final: 0x41, application: applicationCursor)
            case .down: bytes = cursor(final: 0x42, application: applicationCursor)
            case .right: bytes = cursor(final: 0x43, application: applicationCursor)
            case .left: bytes = cursor(final: 0x44, application: applicationCursor)
            case .home: bytes = cursor(final: 0x48, application: applicationCursor)
            case .end: bytes = cursor(final: 0x46, application: applicationCursor)
            case .insert: bytes = Data([0x1B, 0x5B, 0x32, 0x7E])
            case .delete: bytes = Data([0x1B, 0x5B, 0x33, 0x7E])
            case .pageup: bytes = Data([0x1B, 0x5B, 0x35, 0x7E])
            case .pagedown: bytes = Data([0x1B, 0x5B, 0x36, 0x7E])
            case .space: bytes = Data([0x20])
            case .text:
                guard let text, !text.isEmpty else { return nil }
                bytes = Data(text.utf8)
            }
        }
        if modifiers.alt {
            return Data([0x1B]) + bytes
        }
        return bytes
    }

    static func encodeCommittedText(
        _ text: String,
        modifiers: TerminalInputModifiers
    ) -> Data {
        var result = Data()
        for scalar in text.unicodeScalars {
            let value = String(scalar)
            let key: TerminalInputKey = value == " " ? .space : .text
            if let bytes = encode(key, text: value, modifiers: modifiers) {
                result.append(bytes)
            }
        }
        return result
    }

    static func normalizePaste(_ text: String) -> Data {
        Data(text.replacingOccurrences(of: "\r\n", with: "\n").utf8)
    }

    static func pasteRequiresConfirmation(_ bytes: Data) -> Bool {
        bytes.count > pasteConfirmationBytes || bytes.contains(0x0A) || bytes.contains(0x0D)
    }

    static func preparePaste(_ bytes: Data, bracketed: Bool) -> Data {
        guard bracketed else { return bytes }
        return bracketStart + bytes + bracketEnd
    }

    static func maximumPastePayload(bracketed: Bool) -> Int {
        maxPasteBytes - (bracketed ? bracketStart.count + bracketEnd.count : 0)
    }

    static func selectionText(
        rows: [[BoundedTerminalCell]],
        start: TerminalSelectionPoint,
        end: TerminalSelectionPoint
    ) -> String {
        guard start.row <= end.row,
              start.row != end.row || start.column < end.column,
              start.row < rows.count else { return "" }
        var selected: [String] = []
        for rowIndex in start.row...min(end.row, rows.count - 1) {
            let lower = rowIndex == start.row ? start.column : 0
            let upper = rowIndex == end.row ? end.column : Int.max
            var column = 0
            var line = ""
            for cell in rows[rowIndex] where cell.width != .continuation {
                if column >= lower, column < upper { line += cell.text }
                column += cell.width.rawValue
            }
            selected.append(line)
        }
        return selected.joined(separator: "\n")
    }

    static func visibleHTTPURLs(in text: String) -> [URL] {
        var urls: [URL] = []
        for token in text.split(whereSeparator: { $0.isWhitespace }) {
            let value = String(token)
            let starts = [value.range(of: "https://"), value.range(of: "http://")]
                .compactMap { $0?.lowerBound }
            guard let start = starts.min() else { continue }
            let candidate = String(value[start...]).trimmingCharacters(
                in: CharacterSet(charactersIn: ".,;:!?)]}")
            )
            guard candidate.utf8.count <= maxURLBytes,
                  candidate.unicodeScalars.allSatisfy(\.isASCII),
                  !candidate.contains(where: { "'\"<>".contains($0) }),
                  let url = URL(string: candidate),
                  let scheme = url.scheme?.lowercased(),
                  scheme == "http" || scheme == "https",
                  url.host != nil else { continue }
            urls.append(url)
            if urls.count == maxURLs { break }
        }
        return urls
    }

    private static func cursor(final: UInt8, application: Bool) -> Data {
        Data([0x1B, application ? 0x4F : 0x5B, final])
    }

    private static func controlByte(key: TerminalInputKey, text: String?) -> UInt8? {
        switch key {
        case .space: return 0
        case .enter: return 0x0D
        case .backspace: return 0x7F
        default: break
        }
        guard let scalar = text?.unicodeScalars.first,
              text?.unicodeScalars.count == 1,
              scalar.value < 128 else { return nil }
        let value = UInt8(scalar.value)
        let lower = (0x41...0x5A).contains(value) ? value + 0x20 : value
        switch lower {
        case 0x61...0x7A: return lower & 0x1F
        case 0x32, 0x40: return 0
        case 0x33, 0x5B: return 27
        case 0x34, 0x5C: return 28
        case 0x35, 0x5D: return 29
        case 0x36, 0x5E: return 30
        case 0x37, 0x5F, 0x2F: return 31
        default: return nil
        }
    }
}

struct TerminalIMEState: Equatable, Sendable {
    private(set) var markedText = ""

    mutating func update(_ text: String) {
        markedText = text
    }

    mutating func cancel() {
        markedText = ""
    }

    mutating func commit(_ text: String) -> Data? {
        markedText = ""
        return text.isEmpty ? nil : Data(text.utf8)
    }

    mutating func finish() -> Data? {
        let committed = markedText
        markedText = ""
        return committed.isEmpty ? nil : Data(committed.utf8)
    }
}
