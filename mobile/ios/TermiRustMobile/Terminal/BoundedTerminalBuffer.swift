import Foundation

enum TerminalTruncationReason: Equatable, Sendable {
    case frameLimit, parserCarryLimit, retainedRowsLimit, retainedCellsLimit
    case graphemeArenaLimit, styleArenaLimit, modelLimit
}

enum TerminalMouseMode: String, Equatable, Sendable {
    case none, press
    case pressRelease = "press_release"
    case buttonMotion = "button_motion"
    case anyMotion = "any_motion"
}

enum TerminalMouseEncoding: String, Equatable, Sendable {
    case `default`, utf8, sgr
}

enum TerminalCellColor: Equatable, Sendable {
    case `default`
    case indexed(UInt8)
    case rgb(red: UInt8, green: UInt8, blue: UInt8)
}

struct TerminalCellStyle: Equatable, Sendable {
    var foreground: TerminalCellColor = .default
    var background: TerminalCellColor = .default
    var bold = false
    var dim = false
    var italic = false
    var underline = false
    var inverse = false
}

enum TerminalCellWidth: Int, Equatable, Sendable {
    case continuation = 0
    case narrow = 1
    case wide = 2
}

struct BoundedTerminalCell: Equatable, Sendable {
    var text: String
    var width: TerminalCellWidth
    var style: TerminalCellStyle

    static func blank(style: TerminalCellStyle = .init()) -> Self {
        Self(text: " ", width: .narrow, style: style)
    }

    static func continuation(style: TerminalCellStyle) -> Self {
        Self(text: "", width: .continuation, style: style)
    }
}

struct BoundedTerminalSnapshot: Equatable, Sendable {
    let lines: [String]
    let cells: [[BoundedTerminalCell]]
    let contentCells: [[BoundedTerminalCell]]
    let cursorRow: Int
    let cursorColumn: Int
    let retainedCells: Int
    let accountedBytes: Int
    let truncation: TerminalTruncationReason?
    let cursorVisible: Bool
    let applicationCursor: Bool
    let alternateScreen: Bool
    let bracketedPaste: Bool
    let mouseMode: TerminalMouseMode
    let mouseEncoding: TerminalMouseEncoding
    let scrollbackRows: Int
}

struct BoundedTerminalBuffer: Sendable {
    private enum ParserMode: Sendable {
        case ground, escape
        case csi([UInt8])
        case osc(count: Int, escapePending: Bool)
    }

    private struct StoredScreen: Sendable {
        var rows: [[BoundedTerminalCell]]
        var cursorRow: Int
        var cursorColumn: Int
        var savedCursor: (row: Int, column: Int)
    }

    private let limits: TerminalLimits
    private(set) var viewport: TerminalViewportState
    private var rows: [[BoundedTerminalCell]]
    private var cursorRow = 0
    private var cursorColumn = 0
    private var savedCursor = (row: 0, column: 0)
    private var mode: ParserMode = .ground
    private var utf8Carry = Data()
    private var currentStyle = TerminalCellStyle()
    private var retainedCellCount = 0
    private var graphemeByteCount = 0
    private var styleByteCount = 0
    private(set) var truncation: TerminalTruncationReason?
    private var primaryScreen: StoredScreen?
    private var cursorVisible = true
    private var applicationCursor = false
    private var alternateScreen = false
    private var bracketedPaste = false
    private var mouseMode = TerminalMouseMode.none
    private var mouseEncoding = TerminalMouseEncoding.default

    init(
        viewport: TerminalViewportState,
        limits: TerminalLimits = .controllerDefault
    ) throws {
        try limits.validate(viewport: viewport)
        self.limits = limits
        self.viewport = viewport
        rows = Array(repeating: [], count: viewport.rows)
    }

    mutating func reset(viewport: TerminalViewportState? = nil) throws {
        let next = viewport ?? self.viewport
        try limits.validate(viewport: next)
        self.viewport = next
        rows = Array(repeating: [], count: next.rows)
        cursorRow = 0
        cursorColumn = 0
        savedCursor = (0, 0)
        mode = .ground
        utf8Carry.removeAll(keepingCapacity: true)
        currentStyle = .init()
        truncation = nil
        primaryScreen = nil
        cursorVisible = true
        applicationCursor = false
        alternateScreen = false
        bracketedPaste = false
        mouseMode = .none
        mouseEncoding = .default
        recalculateAccounting()
    }

    mutating func resize(_ next: TerminalViewportState) throws {
        try limits.validate(viewport: next)
        let oldVisibleRows = viewport.rows
        Self.resizeRows(&rows, from: oldVisibleRows, to: next.rows, columns: next.columns)
        if var stored = primaryScreen {
            Self.resizeRows(&stored.rows, from: oldVisibleRows, to: next.rows, columns: next.columns)
            stored.cursorRow = min(stored.cursorRow, next.rows - 1)
            stored.cursorColumn = min(stored.cursorColumn, next.columns - 1)
            stored.savedCursor.row = min(stored.savedCursor.row, next.rows - 1)
            stored.savedCursor.column = min(stored.savedCursor.column, next.columns - 1)
            primaryScreen = stored
        }
        viewport = next
        cursorRow = min(cursorRow, rows.count - 1)
        cursorColumn = min(cursorColumn, next.columns - 1)
        savedCursor.row = min(savedCursor.row, rows.count - 1)
        savedCursor.column = min(savedCursor.column, next.columns - 1)
        recalculateAccounting()
        enforceLimits()
    }

    mutating func process(_ data: Data) throws {
        guard !data.isEmpty else { return }
        guard data.count <= limits.maxFrameBytes else {
            truncation = .frameLimit
            throw ReadOnlyAttachFailure.frameTooLarge
        }
        for byte in data {
            consume(byte)
            guard parserCarryBytes <= limits.maxParserCarryBytes else {
                mode = .ground
                utf8Carry.removeAll(keepingCapacity: true)
                truncation = .parserCarryLimit
                enforceLimits()
                return
            }
        }
        enforceLimits()
    }

    func snapshot() -> BoundedTerminalSnapshot {
        BoundedTerminalSnapshot(
            lines: rows.map(renderLine),
            cells: rows.map(paddedRow),
            contentCells: rows,
            cursorRow: max(0, cursorRow - scrollbackRows),
            cursorColumn: cursorColumn,
            retainedCells: retainedCellCount,
            accountedBytes: accountedBytes,
            truncation: truncation,
            cursorVisible: cursorVisible,
            applicationCursor: applicationCursor,
            alternateScreen: alternateScreen,
            bracketedPaste: bracketedPaste,
            mouseMode: mouseMode,
            mouseEncoding: mouseEncoding,
            scrollbackRows: scrollbackRows
        )
    }

    private mutating func consume(_ byte: UInt8) {
        switch mode {
        case .ground:
            consumeGround(byte)
        case .escape:
            switch byte {
            case 0x5B: mode = .csi([])
            case 0x5D: mode = .osc(count: 0, escapePending: false)
            default: mode = .ground
            }
        case .csi(var bytes):
            if (0x40...0x7E).contains(byte) {
                executeCSI(final: byte, bytes: bytes)
                mode = .ground
            } else {
                bytes.append(byte)
                mode = .csi(bytes)
            }
        case .osc(let count, let escapePending):
            if byte == 0x07 || (escapePending && byte == 0x5C) {
                mode = .ground
            } else {
                mode = .osc(count: count.saturatingAdd(1), escapePending: byte == 0x1B)
            }
        }
    }

    private mutating func consumeGround(_ byte: UInt8) {
        switch byte {
        case 0x1B: flushIncompleteUTF8(); mode = .escape
        case 0x0A: flushIncompleteUTF8(); lineFeed()
        case 0x0D: flushIncompleteUTF8(); cursorColumn = 0
        case 0x08: flushIncompleteUTF8(); cursorColumn = max(0, cursorColumn - 1)
        case 0x09:
            flushIncompleteUTF8()
            cursorColumn = min(viewport.columns - 1, ((cursorColumn / 8) + 1) * 8)
        case 0x00...0x1F, 0x7F: flushIncompleteUTF8()
        default: utf8Carry.append(byte); consumeCompleteUTF8()
        }
    }

    private mutating func consumeCompleteUTF8() {
        while let first = utf8Carry.first {
            let expected = utf8Length(first)
            if expected == 0 {
                utf8Carry.removeFirst()
                writeReplacement()
                continue
            }
            guard utf8Carry.count >= expected else { return }
            let prefix = Data(utf8Carry.prefix(expected))
            utf8Carry.removeFirst(expected)
            guard let value = String(data: prefix, encoding: .utf8),
                  value.unicodeScalars.count == 1,
                  let scalar = value.unicodeScalars.first else {
                writeReplacement()
                continue
            }
            writeScalar(scalar)
        }
    }

    private mutating func flushIncompleteUTF8() {
        guard !utf8Carry.isEmpty else { return }
        utf8Carry.removeAll(keepingCapacity: true)
        writeReplacement()
    }

    private mutating func writeReplacement() {
        for scalar in "\u{FFFD}".unicodeScalars { writeScalar(scalar) }
    }

    private mutating func writeScalar(_ scalar: Unicode.Scalar) {
        let scalarWidth = GeneratedTerminalCellWidth.width(of: scalar)
        if scalarWidth == 0 {
            appendZeroWidthScalar(scalar)
            return
        }
        if cursorColumn >= viewport.columns
            || (scalarWidth == 2 && cursorColumn + 1 >= viewport.columns) {
            cursorColumn = 0
            lineFeed()
        }
        ensureCursorRow()
        let previousCount = rows[cursorRow].count
        let previousBytes = rowGraphemeBytes(rows[cursorRow])
        while rows[cursorRow].count < cursorColumn { rows[cursorRow].append(.blank()) }
        repairWideCell(at: cursorColumn)
        let width: TerminalCellWidth = scalarWidth == 2 ? .wide : .narrow
        replaceOrAppend(
            BoundedTerminalCell(text: String(scalar), width: width, style: currentStyle),
            at: cursorColumn
        )
        if width == .wide {
            replaceOrAppend(.continuation(style: currentStyle), at: cursorColumn + 1)
        }
        cursorColumn += scalarWidth
        updateAccounting(previousCount: previousCount, previousBytes: previousBytes)
        enforceLimits()
    }

    private mutating func appendZeroWidthScalar(_ scalar: Unicode.Scalar) {
        ensureCursorRow()
        var index = min(cursorColumn - 1, rows[cursorRow].count - 1)
        if index >= 0, rows[cursorRow][index].width == .continuation { index -= 1 }
        guard index >= 0 else { return }
        let previousBytes = rowGraphemeBytes(rows[cursorRow])
        rows[cursorRow][index].text.unicodeScalars.append(scalar)
        graphemeByteCount += rowGraphemeBytes(rows[cursorRow]) - previousBytes
        enforceLimits()
    }

    private mutating func repairWideCell(at column: Int) {
        guard column < rows[cursorRow].count else { return }
        if rows[cursorRow][column].width == .continuation, column > 0 {
            rows[cursorRow][column - 1] = .blank()
        } else if rows[cursorRow][column].width == .wide,
                  column + 1 < rows[cursorRow].count,
                  rows[cursorRow][column + 1].width == .continuation {
            rows[cursorRow][column + 1] = .blank()
        }
    }

    private mutating func replaceOrAppend(_ cell: BoundedTerminalCell, at column: Int) {
        while rows[cursorRow].count < column { rows[cursorRow].append(.blank()) }
        if column == rows[cursorRow].count { rows[cursorRow].append(cell) }
        else { rows[cursorRow][column] = cell }
    }

    private mutating func lineFeed() {
        cursorRow += 1
        ensureCursorRow()
        enforceLimits()
    }

    private mutating func ensureCursorRow() {
        while cursorRow >= rows.count { rows.append([]) }
    }

    private mutating func executeCSI(final: UInt8, bytes: [UInt8]) {
        let parameters = csiParameters(bytes)
        let first = parameters.first ?? 0
        switch final {
        case 0x41: cursorRow = max(0, cursorRow - max(first, 1))
        case 0x42: cursorRow = min(rows.count - 1, cursorRow + max(first, 1))
        case 0x43: cursorColumn = min(viewport.columns - 1, cursorColumn + max(first, 1))
        case 0x44: cursorColumn = max(0, cursorColumn - max(first, 1))
        case 0x45: cursorRow = min(rows.count - 1, cursorRow + max(first, 1)); cursorColumn = 0
        case 0x46: cursorRow = max(0, cursorRow - max(first, 1)); cursorColumn = 0
        case 0x47: cursorColumn = min(viewport.columns - 1, max(first, 1) - 1)
        case 0x48, 0x66:
            cursorRow = min(rows.count - 1, max(parameters.first ?? 1, 1) - 1)
            cursorColumn = min(viewport.columns - 1, max(parameters.dropFirst().first ?? 1, 1) - 1)
        case 0x4A where first == 2: clearScreen()
        case 0x4B: clearLine(mode: first)
        case 0x6D: applySGR(parameters)
        case 0x73: savedCursor = (cursorRow, cursorColumn)
        case 0x75:
            cursorRow = min(savedCursor.row, rows.count - 1)
            cursorColumn = min(savedCursor.column, viewport.columns - 1)
        case 0x68 where bytes.first == 0x3F: setPrivateModes(parameters, enabled: true)
        case 0x6C where bytes.first == 0x3F: setPrivateModes(parameters, enabled: false)
        default: break
        }
    }

    private mutating func applySGR(_ rawParameters: [Int]) {
        let parameters = rawParameters.isEmpty ? [0] : rawParameters
        var index = 0
        while index < parameters.count {
            let parameter = parameters[index]
            switch parameter {
            case 0: currentStyle = .init()
            case 1: currentStyle.bold = true
            case 2: currentStyle.dim = true
            case 3: currentStyle.italic = true
            case 4: currentStyle.underline = true
            case 7: currentStyle.inverse = true
            case 22: currentStyle.bold = false; currentStyle.dim = false
            case 23: currentStyle.italic = false
            case 24: currentStyle.underline = false
            case 27: currentStyle.inverse = false
            case 30...37: currentStyle.foreground = .indexed(UInt8(parameter - 30))
            case 39: currentStyle.foreground = .default
            case 40...47: currentStyle.background = .indexed(UInt8(parameter - 40))
            case 49: currentStyle.background = .default
            case 90...97: currentStyle.foreground = .indexed(UInt8(parameter - 90 + 8))
            case 100...107: currentStyle.background = .indexed(UInt8(parameter - 100 + 8))
            case 38, 48:
                let parsed = extendedColor(parameters, startingAt: index)
                if let color = parsed.color {
                    if parameter == 38 { currentStyle.foreground = color }
                    else { currentStyle.background = color }
                }
                index += parsed.consumed
            default: break
            }
            index += 1
        }
    }

    private func extendedColor(
        _ parameters: [Int],
        startingAt index: Int
    ) -> (color: TerminalCellColor?, consumed: Int) {
        guard let mode = parameters[safe: index + 1] else { return (nil, 0) }
        if mode == 5, let value = parameters[safe: index + 2], (0...255).contains(value) {
            return (.indexed(UInt8(value)), 2)
        }
        if mode == 2,
           let red = parameters[safe: index + 2],
           let green = parameters[safe: index + 3],
           let blue = parameters[safe: index + 4],
           (0...255).contains(red), (0...255).contains(green), (0...255).contains(blue) {
            return (.rgb(red: UInt8(red), green: UInt8(green), blue: UInt8(blue)), 4)
        }
        return (nil, 0)
    }

    private mutating func setPrivateModes(_ modes: [Int], enabled: Bool) {
        for mode in modes {
            switch mode {
            case 1: applicationCursor = enabled
            case 9: updateMouseMode(.press, enabled: enabled)
            case 25: cursorVisible = enabled
            case 1000: updateMouseMode(.pressRelease, enabled: enabled)
            case 1002: updateMouseMode(.buttonMotion, enabled: enabled)
            case 1003: updateMouseMode(.anyMotion, enabled: enabled)
            case 1005: updateMouseEncoding(.utf8, enabled: enabled)
            case 1006: updateMouseEncoding(.sgr, enabled: enabled)
            case 1049: if enabled { enterAlternateScreen() } else { leaveAlternateScreen() }
            case 2004: bracketedPaste = enabled
            default: break
            }
        }
    }

    private mutating func updateMouseMode(_ next: TerminalMouseMode, enabled: Bool) {
        if enabled { mouseMode = next }
        else if mouseMode == next { mouseMode = .none }
    }

    private mutating func updateMouseEncoding(_ next: TerminalMouseEncoding, enabled: Bool) {
        if enabled { mouseEncoding = next }
        else if mouseEncoding == next { mouseEncoding = .default }
    }

    private mutating func enterAlternateScreen() {
        guard !alternateScreen else { return }
        primaryScreen = StoredScreen(
            rows: rows,
            cursorRow: cursorRow,
            cursorColumn: cursorColumn,
            savedCursor: savedCursor
        )
        rows = Array(repeating: [], count: viewport.rows)
        cursorRow = 0
        cursorColumn = 0
        savedCursor = (0, 0)
        alternateScreen = true
        recalculateAccounting()
    }

    private mutating func leaveAlternateScreen() {
        guard alternateScreen else { return }
        if let primaryScreen {
            rows = primaryScreen.rows
            cursorRow = primaryScreen.cursorRow
            cursorColumn = primaryScreen.cursorColumn
            savedCursor = primaryScreen.savedCursor
        }
        primaryScreen = nil
        alternateScreen = false
        recalculateAccounting()
    }

    private mutating func clearScreen() {
        rows = Array(repeating: [], count: viewport.rows)
        cursorRow = 0
        cursorColumn = 0
        recalculateAccounting()
    }

    private mutating func clearLine(mode: Int) {
        ensureCursorRow()
        switch mode {
        case 1:
            while rows[cursorRow].count <= cursorColumn { rows[cursorRow].append(.blank()) }
            for column in 0...cursorColumn {
                rows[cursorRow][column] = .blank(style: currentStyle)
            }
        case 2: rows[cursorRow].removeAll(keepingCapacity: true)
        default:
            if cursorColumn < rows[cursorRow].count {
                rows[cursorRow].removeSubrange(cursorColumn..<rows[cursorRow].count)
            }
        }
        recalculateAccounting()
    }

    private static func resizeRows(
        _ target: inout [[BoundedTerminalCell]],
        from oldRows: Int,
        to newRows: Int,
        columns: Int
    ) {
        let visibleStart = max(0, target.count - oldRows)
        if newRows < oldRows {
            let start = min(target.count, visibleStart + newRows)
            let end = min(target.count, visibleStart + oldRows)
            if start < end { target.removeSubrange(start..<end) }
        } else {
            while target.count < visibleStart + newRows { target.append([]) }
        }
        for index in target.indices where target[index].count > columns {
            target[index].removeSubrange(columns..<target[index].count)
        }
    }

    private mutating func enforceLimits() {
        while alternateScreen && rows.count > viewport.rows { evictOldestRow() }
        while rows.count > viewport.rows,
              (scrollbackRows > limits.maxScrollbackRows
                || retainedCellCount > limits.maxRetainedCells
                || graphemeByteCount > limits.maxGraphemeBytes
                || styleByteCount > limits.maxStyleBytes
                || accountedBytes > limits.maxModelBytes) {
            evictOldestRow()
        }
        guard retainedCellCount <= limits.maxRetainedCells,
              graphemeByteCount <= limits.maxGraphemeBytes,
              styleByteCount <= limits.maxStyleBytes,
              accountedBytes <= limits.maxModelBytes else {
            truncateActiveRow()
            return
        }
        if scrollbackRows > limits.maxScrollbackRows { truncation = .retainedRowsLimit }
    }

    private mutating func evictOldestRow() {
        let removed = rows.removeFirst()
        retainedCellCount -= removed.count
        graphemeByteCount -= rowGraphemeBytes(removed)
        styleByteCount -= removed.count
        cursorRow = max(0, cursorRow - 1)
        savedCursor.row = max(0, savedCursor.row - 1)
    }

    private mutating func truncateActiveRow() {
        ensureCursorRow()
        if retainedCellCount > limits.maxRetainedCells { truncation = .retainedCellsLimit }
        else if graphemeByteCount > limits.maxGraphemeBytes { truncation = .graphemeArenaLimit }
        else if styleByteCount > limits.maxStyleBytes { truncation = .styleArenaLimit }
        else { truncation = .modelLimit }
        while !rows[cursorRow].isEmpty,
              (retainedCellCount > limits.maxRetainedCells
                || graphemeByteCount > limits.maxGraphemeBytes
                || styleByteCount > limits.maxStyleBytes
                || accountedBytes > limits.maxModelBytes) {
            let removed = rows[cursorRow].removeLast()
            retainedCellCount -= 1
            graphemeByteCount -= removed.text.utf8.count
            styleByteCount -= 1
        }
        if rows[cursorRow].count < viewport.columns {
            rows[cursorRow].append(.init(text: "\u{FFFD}", width: .narrow, style: currentStyle))
            retainedCellCount += 1
            graphemeByteCount += 3
            styleByteCount += 1
        }
        cursorColumn = min(rows[cursorRow].count, viewport.columns - 1)
    }

    private mutating func recalculateAccounting() {
        retainedCellCount = rows.reduce(0) { $0.saturatingAdd($1.count) }
        graphemeByteCount = rows.reduce(0) { total, row in
            total.saturatingAdd(row.reduce(0) { $0.saturatingAdd($1.text.utf8.count) })
        }
        styleByteCount = retainedCellCount
    }

    private mutating func updateAccounting(previousCount: Int, previousBytes: Int) {
        let row = rows[cursorRow]
        retainedCellCount += row.count - previousCount
        graphemeByteCount += rowGraphemeBytes(row) - previousBytes
        styleByteCount += row.count - previousCount
    }

    private func rowGraphemeBytes(_ row: [BoundedTerminalCell]) -> Int {
        row.reduce(0) { $0.saturatingAdd($1.text.utf8.count) }
    }

    private func renderLine(_ row: [BoundedTerminalCell]) -> String {
        row.filter { $0.width != .continuation }.map(\.text).joined()
    }

    private func paddedRow(_ row: [BoundedTerminalCell]) -> [BoundedTerminalCell] {
        if row.count >= viewport.columns { return Array(row.prefix(viewport.columns)) }
        return row + Array(repeating: .blank(), count: viewport.columns - row.count)
    }

    private var scrollbackRows: Int { max(0, rows.count - viewport.rows) }
    private var parserCarryBytes: Int {
        let modeBytes = switch mode {
        case .csi(let bytes): bytes.count
        case .osc(let count, _): count
        default: 0
        }
        return modeBytes.saturatingAdd(utf8Carry.count)
    }
    private var accountedBytes: Int {
        graphemeByteCount
            .saturatingAdd(styleByteCount)
            .saturatingAdd(retainedCellCount.saturatingMultiply(16))
            .saturatingAdd(parserCarryBytes)
    }

    private func csiParameters(_ bytes: [UInt8]) -> [Int] {
        String(decoding: bytes, as: UTF8.self)
            .trimmingCharacters(in: CharacterSet(charactersIn: "?<>!"))
            .split(separator: ";", omittingEmptySubsequences: false)
            .map { Int($0) ?? 0 }
    }

    private func utf8Length(_ byte: UInt8) -> Int {
        switch byte {
        case 0x00...0x7F: 1
        case 0xC2...0xDF: 2
        case 0xE0...0xEF: 3
        case 0xF0...0xF4: 4
        default: 0
        }
    }
}

private extension Collection {
    subscript(safe index: Index) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}

private extension Int {
    func saturatingAdd(_ value: Int) -> Int {
        addingReportingOverflow(value).overflow ? .max : self + value
    }

    func saturatingMultiply(_ value: Int) -> Int {
        multipliedReportingOverflow(by: value).overflow ? .max : self * value
    }
}
