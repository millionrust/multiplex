import Foundation
import TermiRustMobileCrypto

@MainActor
final class TerminalBuffer: ObservableObject {
    @Published private(set) var lines: [String] = []

    private let maxLines: Int
    private var transcript = Data()
    private var fallback: FallbackTerminalBuffer
    private var columns = 80
    private var rows = 24

    init(maxLines: Int = 2_000) {
        self.maxLines = maxLines
        self.fallback = FallbackTerminalBuffer(maxLines: maxLines)
    }

    func append(_ text: String) {
        append(Data(text.utf8))
    }

    func append(_ data: Data) {
        guard !data.isEmpty else {
            return
        }
        if renderWithNative(newData: data) {
            return
        }
        fallback.append(String(decoding: data, as: UTF8.self))
        lines = fallback.lines()
    }

    func resize(columns: Int, rows: Int) {
        let nextColumns = max(columns, 1)
        let nextRows = max(rows, 1)
        guard self.columns != nextColumns || self.rows != nextRows else {
            return
        }
        self.columns = nextColumns
        self.rows = nextRows
        _ = renderWithNative(newData: Data())
    }

    func clear() {
        transcript.removeAll(keepingCapacity: true)
        fallback.clear()
        lines.removeAll()
    }

    private func renderWithNative(newData: Data) -> Bool {
        transcript.append(newData)
        let result = transcript.withUnsafeBytes { bytes in
            termirust_mobile_render_terminal_utf8(
                bytes.bindMemory(to: UInt8.self).baseAddress,
                transcript.count,
                UInt16(clamping: columns),
                UInt16(clamping: rows),
                maxLines
            )
        }
        defer {
            termirust_mobile_free_result(result)
        }

        guard result.ok else {
            transcript.removeLast(newData.count)
            return false
        }

        let output = String(data: data(from: result.data), encoding: .utf8) ?? ""
        lines = output.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        return true
    }

    private func data(from buffer: TermiRustMobileByteBuffer) -> Data {
        guard let ptr = buffer.ptr, buffer.len > 0 else {
            return Data()
        }
        return Data(bytes: ptr, count: buffer.len)
    }
}

private final class FallbackTerminalBuffer {
    private let maxLines: Int
    private var rows: [String] = []
    private var row = 0
    private var column = 0

    init(maxLines: Int) {
        self.maxLines = maxLines
    }

    func append(_ text: String) {
        let chars = Array(text)
        var index = 0
        while index < chars.count {
            let char = chars[index]
            if char == "\u{1B}" {
                index = consumeEscape(chars, from: index + 1)
                continue
            }

            switch char {
            case "\r":
                column = 0
            case "\n":
                newLine()
            case "\u{8}":
                column = max(0, column - 1)
            default:
                write(char)
            }
            index += 1
        }
    }

    func clear() {
        rows.removeAll()
        row = 0
        column = 0
    }

    func lines() -> [String] {
        trimScrollback()
        return Array(rows.suffix(maxLines))
    }

    private func write(_ char: Character) {
        ensureRow()
        var current = rows[row]
        while current.count < column {
            current.append(" ")
        }
        if column == current.count {
            current.append(char)
        } else {
            let index = current.index(current.startIndex, offsetBy: column)
            current.replaceSubrange(index...index, with: String(char))
        }
        rows[row] = current
        column += 1
    }

    private func newLine() {
        row += 1
        column = 0
        ensureRow()
        trimScrollback()
    }

    private func ensureRow() {
        while rows.count <= row {
            rows.append("")
        }
    }

    private func consumeEscape(_ chars: [Character], from start: Int) -> Int {
        guard start < chars.count else {
            return start
        }
        guard chars[start] == "[" else {
            return start
        }

        var index = start + 1
        var parameters = ""
        while index < chars.count {
            let char = chars[index]
            if let scalar = char.unicodeScalars.first,
               scalar.value >= 0x40,
               scalar.value <= 0x7E {
                handleCsi(parameters: parameters, final: char)
                return index + 1
            }
            parameters.append(char)
            index += 1
        }
        return chars.count
    }

    private func handleCsi(parameters: String, final: Character) {
        let first = parameters
            .split(separator: ";")
            .first
            .map { $0.filter(\.isNumber) }
            .flatMap { Int($0) }

        switch final {
        case "m":
            break
        case "H", "f":
            row = 0
            column = 0
            ensureRow()
        case "J":
            if first == nil || first == 2 {
                rows.removeAll()
                row = 0
                column = 0
            }
        case "K":
            ensureRow()
            let current = rows[row]
            if column < current.count {
                let index = current.index(current.startIndex, offsetBy: column)
                rows[row] = String(current[..<index])
            }
        case "C":
            column += first ?? 1
        case "D":
            column = max(0, column - (first ?? 1))
        case "A":
            row = max(0, row - (first ?? 1))
        case "B":
            row += first ?? 1
            ensureRow()
        default:
            break
        }
    }

    private func trimScrollback() {
        guard rows.count > maxLines else {
            return
        }
        let removeCount = rows.count - maxLines
        rows.removeFirst(removeCount)
        row = max(0, row - removeCount)
    }
}
