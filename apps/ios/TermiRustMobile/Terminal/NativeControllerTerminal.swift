import Foundation
import TermiRustMobileCrypto

final class NativeControllerTerminalSession: @unchecked Sendable {
    private var handle: OpaquePointer?

    init?(
        viewport: TerminalViewportState,
        limits: TerminalLimits
    ) {
        let cellsPerRow = max(viewport.columns, 1)
        let cellBound = max(0, limits.maxRetainedCells / cellsPerRow - viewport.rows)
        let byteBound = max(0, limits.maxModelBytes / max(cellsPerRow * 64, 1) - viewport.rows)
        let scrollback = min(limits.maxScrollbackRows, cellBound, byteBound)
        handle = termirust_mobile_terminal_create(
            UInt16(clamping: viewport.columns),
            UInt16(clamping: viewport.rows),
            scrollback
        )
        if handle == nil { return nil }
    }

    deinit {
        termirust_mobile_terminal_destroy(handle)
    }

    func process(_ data: Data) throws -> BoundedTerminalSnapshot {
        guard let handle else { throw NativeTerminalError.closed }
        let result = data.withUnsafeBytes { bytes in
            termirust_mobile_terminal_process(
                handle,
                bytes.bindMemory(to: UInt8.self).baseAddress,
                data.count
            )
        }
        return try decode(result)
    }

    func feed(_ data: Data) throws {
        guard let handle else { throw NativeTerminalError.closed }
        let accepted = data.withUnsafeBytes { bytes in
            termirust_mobile_terminal_feed(
                handle,
                bytes.bindMemory(to: UInt8.self).baseAddress,
                data.count
            )
        }
        guard accepted else {
            throw NativeTerminalError.operationFailed("Native terminal rejected the frame.")
        }
    }

    func resize(_ viewport: TerminalViewportState) throws -> BoundedTerminalSnapshot {
        guard let handle else { throw NativeTerminalError.closed }
        return try decode(termirust_mobile_terminal_resize(
            handle,
            UInt16(clamping: viewport.columns),
            UInt16(clamping: viewport.rows)
        ))
    }

    func snapshot() throws -> BoundedTerminalSnapshot {
        guard let handle else { throw NativeTerminalError.closed }
        return try decode(termirust_mobile_terminal_snapshot(handle))
    }

    private func decode(_ result: TermiRustMobileResult) throws -> BoundedTerminalSnapshot {
        defer { termirust_mobile_free_result(result) }
        guard result.ok else {
            throw NativeTerminalError.operationFailed(String(
                decoding: data(from: result.error),
                as: UTF8.self
            ))
        }
        return try JSONDecoder().decode(
            NativeTerminalSnapshot.self,
            from: data(from: result.data)
        ).model()
    }

    private func data(from buffer: TermiRustMobileByteBuffer) -> Data {
        guard let pointer = buffer.ptr, buffer.len > 0 else { return Data() }
        return Data(bytes: pointer, count: buffer.len)
    }
}

private enum NativeTerminalError: Error {
    case closed
    case operationFailed(String)
}

private struct NativeTerminalSnapshot: Decodable {
    let schemaVersion: Int
    let columns: Int
    let rows: Int
    let lines: [String]
    let cells: [[NativeTerminalCell]]
    let cursorRow: Int
    let cursorColumn: Int
    let cursorVisible: Bool
    let applicationCursor: Bool
    let applicationKeypad: Bool
    let alternateScreen: Bool
    let bracketedPaste: Bool
    let mouseMode: String
    let mouseEncoding: String
    let scrollbackRows: Int
    let retainedCells: Int
    let accountedBytes: Int

    enum CodingKeys: String, CodingKey {
        case columns, rows, lines, cells
        case schemaVersion = "schema_version"
        case cursorRow = "cursor_row"
        case cursorColumn = "cursor_column"
        case cursorVisible = "cursor_visible"
        case applicationCursor = "application_cursor"
        case applicationKeypad = "application_keypad"
        case alternateScreen = "alternate_screen"
        case bracketedPaste = "bracketed_paste"
        case mouseMode = "mouse_mode"
        case mouseEncoding = "mouse_encoding"
        case scrollbackRows = "scrollback_rows"
        case retainedCells = "retained_cells"
        case accountedBytes = "accounted_bytes"
    }

    func model() throws -> BoundedTerminalSnapshot {
        guard schemaVersion == 1,
              columns > 0,
              rows > 0,
              cells.count >= rows,
              let mouseMode = TerminalMouseMode(rawValue: mouseMode),
              let mouseEncoding = TerminalMouseEncoding(rawValue: mouseEncoding) else {
            throw NativeTerminalError.operationFailed("Unsupported native terminal snapshot.")
        }
        let content = try cells.map { try $0.map { try $0.model() } }
        guard content.allSatisfy({ $0.count <= columns }) else {
            throw NativeTerminalError.operationFailed("Invalid native terminal row width.")
        }
        let padded = content.map { row in
            row + Array(repeating: .blank(), count: columns - row.count)
        }
        return BoundedTerminalSnapshot(
            lines: lines,
            cells: padded,
            contentCells: content,
            cursorRow: cursorRow,
            cursorColumn: cursorColumn,
            retainedCells: retainedCells,
            accountedBytes: accountedBytes,
            truncation: nil,
            cursorVisible: cursorVisible,
            applicationCursor: applicationCursor,
            applicationKeypad: applicationKeypad,
            alternateScreen: alternateScreen,
            bracketedPaste: bracketedPaste,
            mouseMode: mouseMode,
            mouseEncoding: mouseEncoding,
            scrollbackRows: scrollbackRows
        )
    }
}

private struct NativeTerminalCell: Decodable {
    let text: String
    let width: Int
    let foreground: NativeTerminalColor
    let background: NativeTerminalColor
    let bold: Bool
    let dim: Bool
    let italic: Bool
    let underline: Bool
    let inverse: Bool

    func model() throws -> BoundedTerminalCell {
        guard let width = TerminalCellWidth(rawValue: width) else {
            throw NativeTerminalError.operationFailed("Unsupported native terminal cell width.")
        }
        return BoundedTerminalCell(
            text: text,
            width: width,
            style: TerminalCellStyle(
                foreground: try foreground.model(),
                background: try background.model(),
                bold: bold,
                dim: dim,
                italic: italic,
                underline: underline,
                inverse: inverse
            )
        )
    }
}

private struct NativeTerminalColor: Decodable {
    let kind: String
    let value: UInt8?
    let red: UInt8?
    let green: UInt8?
    let blue: UInt8?

    func model() throws -> TerminalCellColor {
        switch kind {
        case "default": return .default
        case "indexed":
            guard let value else { break }
            return .indexed(value)
        case "rgb":
            guard let red, let green, let blue else { break }
            return .rgb(red: red, green: green, blue: blue)
        default: break
        }
        throw NativeTerminalError.operationFailed("Unsupported native terminal color.")
    }
}

private extension BoundedTerminalCell {
    var isDefaultBlank: Bool {
        text == " " && width == .narrow && style == TerminalCellStyle()
    }
}
