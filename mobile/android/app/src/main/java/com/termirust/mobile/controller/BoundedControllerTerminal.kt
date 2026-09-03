package com.termirust.mobile.controller

import java.nio.charset.StandardCharsets

enum class TerminalTruncationReason {
    FRAME_LIMIT, PARSER_CARRY_LIMIT, RETAINED_ROWS_LIMIT, RETAINED_CELLS_LIMIT,
    GRAPHEME_ARENA_LIMIT, STYLE_ARENA_LIMIT, MODEL_LIMIT,
}

enum class TerminalMouseMode(val wireName: String) {
    NONE("none"),
    PRESS("press"),
    PRESS_RELEASE("press_release"),
    BUTTON_MOTION("button_motion"),
    ANY_MOTION("any_motion"),
}

enum class TerminalMouseEncoding(val wireName: String) {
    DEFAULT("default"), UTF8("utf8"), SGR("sgr"),
}

sealed interface TerminalCellColor {
    data object Default : TerminalCellColor
    data class Indexed(val value: Int) : TerminalCellColor
    data class Rgb(val red: Int, val green: Int, val blue: Int) : TerminalCellColor
}

data class TerminalCellStyle(
    val foreground: TerminalCellColor = TerminalCellColor.Default,
    val background: TerminalCellColor = TerminalCellColor.Default,
    val bold: Boolean = false,
    val dim: Boolean = false,
    val italic: Boolean = false,
    val underline: Boolean = false,
    val inverse: Boolean = false,
)

enum class TerminalCellWidth(val columns: Int) {
    CONTINUATION(0), NARROW(1), WIDE(2),
}

data class BoundedTerminalCell(
    val text: String,
    val width: TerminalCellWidth,
    val style: TerminalCellStyle,
) {
    companion object {
        fun blank(style: TerminalCellStyle = TerminalCellStyle()) =
            BoundedTerminalCell(" ", TerminalCellWidth.NARROW, style)

        fun continuation(style: TerminalCellStyle) =
            BoundedTerminalCell("", TerminalCellWidth.CONTINUATION, style)
    }
}

data class BoundedTerminalSnapshot(
    val lines: List<String>,
    val cells: List<List<BoundedTerminalCell>>,
    val contentCells: List<List<BoundedTerminalCell>>,
    val cursorRow: Int,
    val cursorColumn: Int,
    val retainedCells: Int,
    val accountedBytes: Int,
    val truncation: TerminalTruncationReason?,
    val cursorVisible: Boolean,
    val applicationCursor: Boolean,
    val applicationKeypad: Boolean,
    val alternateScreen: Boolean,
    val bracketedPaste: Boolean,
    val mouseMode: TerminalMouseMode,
    val mouseEncoding: TerminalMouseEncoding,
    val scrollbackRows: Int,
)

class BoundedControllerTerminal(
    viewport: TerminalViewport,
    private val limits: TerminalLimits = TerminalLimits(),
) {
    private sealed interface ParserMode {
        data object Ground : ParserMode
        data object Escape : ParserMode
        data class Csi(val bytes: MutableList<Byte>) : ParserMode
        data class Osc(val count: Int, val escapePending: Boolean) : ParserMode
    }

    private data class StoredScreen(
        val rows: MutableList<MutableList<BoundedTerminalCell>>,
        var cursorRow: Int,
        var cursorColumn: Int,
        var savedCursorRow: Int,
        var savedCursorColumn: Int,
    )

    var viewport: TerminalViewport = viewport
        private set
    private val rows = mutableListOf<MutableList<BoundedTerminalCell>>()
    private var cursorRow = 0
    private var cursorColumn = 0
    private var savedCursorRow = 0
    private var savedCursorColumn = 0
    private var mode: ParserMode = ParserMode.Ground
    private val utf8Carry = mutableListOf<Byte>()
    private var currentStyle = TerminalCellStyle()
    private var retainedCells = 0
    private var graphemeBytes = 0
    private var styleBytes = 0
    private var truncation: TerminalTruncationReason? = null
    private var primaryScreen: StoredScreen? = null
    private var cursorVisible = true
    private var applicationCursor = false
    private var applicationKeypad = false
    private var alternateScreen = false
    private var bracketedPaste = false
    private var mouseMode = TerminalMouseMode.NONE
    private var mouseEncoding = TerminalMouseEncoding.DEFAULT
    private var nativeTerminal: NativeControllerTerminalSession? = null

    init {
        limits.validate(viewport)
        repeat(viewport.rows) { rows += mutableListOf<BoundedTerminalCell>() }
        nativeTerminal = NativeControllerTerminalSession.openOrNull(viewport, limits)
    }

    fun reset(nextViewport: TerminalViewport = viewport) {
        limits.validate(nextViewport)
        viewport = nextViewport
        rows.clear()
        repeat(nextViewport.rows) { rows += mutableListOf<BoundedTerminalCell>() }
        cursorRow = 0
        cursorColumn = 0
        savedCursorRow = 0
        savedCursorColumn = 0
        mode = ParserMode.Ground
        utf8Carry.clear()
        currentStyle = TerminalCellStyle()
        truncation = null
        primaryScreen = null
        cursorVisible = true
        applicationCursor = false
        applicationKeypad = false
        alternateScreen = false
        bracketedPaste = false
        mouseMode = TerminalMouseMode.NONE
        mouseEncoding = TerminalMouseEncoding.DEFAULT
        recalculateAccounting()
        nativeTerminal?.close()
        nativeTerminal = NativeControllerTerminalSession.openOrNull(nextViewport, limits)
    }

    fun resize(nextViewport: TerminalViewport) {
        limits.validate(nextViewport)
        val oldVisibleRows = viewport.rows
        resizeRows(rows, oldVisibleRows, nextViewport.rows, nextViewport.columns)
        primaryScreen?.let { stored ->
            resizeRows(stored.rows, oldVisibleRows, nextViewport.rows, nextViewport.columns)
            stored.cursorRow = stored.cursorRow.coerceAtMost(nextViewport.rows - 1)
            stored.cursorColumn = stored.cursorColumn.coerceAtMost(nextViewport.columns - 1)
            stored.savedCursorRow = stored.savedCursorRow.coerceAtMost(nextViewport.rows - 1)
            stored.savedCursorColumn = stored.savedCursorColumn.coerceAtMost(nextViewport.columns - 1)
        }
        viewport = nextViewport
        cursorRow = cursorRow.coerceIn(0, rows.lastIndex)
        cursorColumn = cursorColumn.coerceIn(0, nextViewport.columns - 1)
        savedCursorRow = savedCursorRow.coerceIn(0, rows.lastIndex)
        savedCursorColumn = savedCursorColumn.coerceIn(0, nextViewport.columns - 1)
        recalculateAccounting()
        enforceLimits()
        updateNative { it.resize(nextViewport); Unit }
    }

    fun process(bytes: ByteArray) {
        if (bytes.isEmpty()) return
        if (bytes.size > limits.maxFrameBytes) {
            truncation = TerminalTruncationReason.FRAME_LIMIT
            throw IllegalArgumentException("terminal frame exceeds limit")
        }
        for (byte in bytes) {
            consume(byte.toInt() and 0xff)
            if (parserCarryBytes > limits.maxParserCarryBytes) {
                mode = ParserMode.Ground
                utf8Carry.clear()
                truncation = TerminalTruncationReason.PARSER_CARRY_LIMIT
                enforceLimits()
                return
            }
        }
        enforceLimits()
        updateNative { it.feed(bytes) }
    }

    fun snapshot() = runCatching { nativeTerminal?.snapshot() }.getOrNull()
        ?.let { it.copy(truncation = truncation ?: it.truncation) }
        ?: BoundedTerminalSnapshot(
        lines = rows.map(::renderLine),
        cells = rows.map(::paddedRow),
        contentCells = rows.map { it.toList() },
        cursorRow = (cursorRow - scrollbackRows).coerceAtLeast(0),
        cursorColumn = cursorColumn,
        retainedCells = retainedCells,
        accountedBytes = accountedBytes,
        truncation = truncation,
        cursorVisible = cursorVisible,
        applicationCursor = applicationCursor,
        applicationKeypad = applicationKeypad,
        alternateScreen = alternateScreen,
        bracketedPaste = bracketedPaste,
        mouseMode = mouseMode,
        mouseEncoding = mouseEncoding,
        scrollbackRows = scrollbackRows,
    )

    fun close() {
        nativeTerminal?.close()
        nativeTerminal = null
    }

    private fun updateNative(operation: (NativeControllerTerminalSession) -> Unit) {
        val terminal = nativeTerminal ?: return
        runCatching { operation(terminal) }
            .onFailure {
                terminal.close()
                nativeTerminal = null
            }
    }

    private fun consume(byte: Int) {
        when (val current = mode) {
            ParserMode.Ground -> consumeGround(byte)
            ParserMode.Escape -> mode = when (byte) {
                0x5b -> ParserMode.Csi(mutableListOf())
                0x5d -> ParserMode.Osc(0, false)
                0x3d -> { applicationKeypad = true; ParserMode.Ground }
                0x3e -> { applicationKeypad = false; ParserMode.Ground }
                else -> ParserMode.Ground
            }
            is ParserMode.Csi -> {
                if (byte in 0x40..0x7e) {
                    executeCsi(byte, current.bytes)
                    mode = ParserMode.Ground
                } else {
                    current.bytes += byte.toByte()
                }
            }
            is ParserMode.Osc -> {
                mode = if (byte == 0x07 || current.escapePending && byte == 0x5c) {
                    ParserMode.Ground
                } else {
                    ParserMode.Osc(current.count.saturatedAdd(1), byte == 0x1b)
                }
            }
        }
    }

    private fun consumeGround(byte: Int) {
        when (byte) {
            0x1b -> { flushIncompleteUtf8(); mode = ParserMode.Escape }
            0x0a -> { flushIncompleteUtf8(); lineFeed() }
            0x0d -> { flushIncompleteUtf8(); cursorColumn = 0 }
            0x08 -> { flushIncompleteUtf8(); cursorColumn = (cursorColumn - 1).coerceAtLeast(0) }
            0x09 -> {
                flushIncompleteUtf8()
                cursorColumn = (((cursorColumn / 8) + 1) * 8).coerceAtMost(viewport.columns - 1)
            }
            in 0x00..0x1f, 0x7f -> flushIncompleteUtf8()
            else -> {
                utf8Carry += byte.toByte()
                consumeCompleteUtf8()
            }
        }
    }

    private fun consumeCompleteUtf8() {
        while (utf8Carry.isNotEmpty()) {
            val expected = utf8Length(utf8Carry.first().toInt() and 0xff)
            if (expected == 0) {
                utf8Carry.removeAt(0)
                writeCodePoint(0xfffd)
                continue
            }
            if (utf8Carry.size < expected) return
            val prefix = ByteArray(expected) { utf8Carry.removeAt(0) }
            val value = prefix.toString(StandardCharsets.UTF_8)
            if (value.toByteArray(StandardCharsets.UTF_8).contentEquals(prefix)) {
                writeCodePoint(value.codePointAt(0))
            } else {
                writeCodePoint(0xfffd)
            }
        }
    }

    private fun flushIncompleteUtf8() {
        if (utf8Carry.isEmpty()) return
        utf8Carry.clear()
        writeCodePoint(0xfffd)
    }

    private fun writeCodePoint(codePoint: Int) {
        val widthColumns = GeneratedTerminalCellWidth.width(codePoint)
        if (widthColumns == 0) {
            appendZeroWidthCodePoint(codePoint)
            return
        }
        if (cursorColumn >= viewport.columns ||
            widthColumns == 2 && cursorColumn + 1 >= viewport.columns
        ) {
            cursorColumn = 0
            lineFeed()
        }
        ensureCursorRow()
        val row = rows[cursorRow]
        val previousCount = row.size
        val previousBytes = rowGraphemeBytes(row)
        while (row.size < cursorColumn) row += BoundedTerminalCell.blank()
        repairWideCell(row, cursorColumn)
        val width = if (widthColumns == 2) TerminalCellWidth.WIDE else TerminalCellWidth.NARROW
        replaceOrAppend(
            row,
            cursorColumn,
            BoundedTerminalCell(String(Character.toChars(codePoint)), width, currentStyle),
        )
        if (width == TerminalCellWidth.WIDE) {
            replaceOrAppend(row, cursorColumn + 1, BoundedTerminalCell.continuation(currentStyle))
        }
        cursorColumn += widthColumns
        updateAccounting(row, previousCount, previousBytes)
        enforceLimits()
    }

    private fun appendZeroWidthCodePoint(codePoint: Int) {
        ensureCursorRow()
        val row = rows[cursorRow]
        var index = minOf(cursorColumn - 1, row.lastIndex)
        if (index >= 0 && row[index].width == TerminalCellWidth.CONTINUATION) index -= 1
        if (index < 0) return
        val previousBytes = rowGraphemeBytes(row)
        row[index] = row[index].copy(text = row[index].text + String(Character.toChars(codePoint)))
        graphemeBytes += rowGraphemeBytes(row) - previousBytes
        enforceLimits()
    }

    private fun repairWideCell(row: MutableList<BoundedTerminalCell>, column: Int) {
        if (column >= row.size) return
        if (row[column].width == TerminalCellWidth.CONTINUATION && column > 0) {
            row[column - 1] = BoundedTerminalCell.blank()
        } else if (row[column].width == TerminalCellWidth.WIDE &&
            column + 1 < row.size &&
            row[column + 1].width == TerminalCellWidth.CONTINUATION
        ) {
            row[column + 1] = BoundedTerminalCell.blank()
        }
    }

    private fun replaceOrAppend(
        row: MutableList<BoundedTerminalCell>,
        column: Int,
        cell: BoundedTerminalCell,
    ) {
        while (row.size < column) row += BoundedTerminalCell.blank()
        if (column == row.size) row += cell else row[column] = cell
    }

    private fun lineFeed() {
        cursorRow += 1
        ensureCursorRow()
        enforceLimits()
    }

    private fun ensureCursorRow() {
        while (cursorRow >= rows.size) rows += mutableListOf<BoundedTerminalCell>()
    }

    private fun executeCsi(finalByte: Int, raw: List<Byte>) {
        val parameters = csiParameters(raw)
        val first = parameters.firstOrNull() ?: 0
        when (finalByte) {
            0x41 -> cursorRow = (cursorRow - first.coerceAtLeast(1)).coerceAtLeast(0)
            0x42 -> cursorRow = (cursorRow + first.coerceAtLeast(1)).coerceAtMost(rows.lastIndex)
            0x43 -> cursorColumn = (cursorColumn + first.coerceAtLeast(1)).coerceAtMost(viewport.columns - 1)
            0x44 -> cursorColumn = (cursorColumn - first.coerceAtLeast(1)).coerceAtLeast(0)
            0x45 -> { cursorRow = (cursorRow + first.coerceAtLeast(1)).coerceAtMost(rows.lastIndex); cursorColumn = 0 }
            0x46 -> { cursorRow = (cursorRow - first.coerceAtLeast(1)).coerceAtLeast(0); cursorColumn = 0 }
            0x47 -> cursorColumn = (first.coerceAtLeast(1) - 1).coerceAtMost(viewport.columns - 1)
            0x48, 0x66 -> {
                cursorRow = ((parameters.firstOrNull() ?: 1).coerceAtLeast(1) - 1).coerceAtMost(rows.lastIndex)
                cursorColumn = ((parameters.getOrNull(1) ?: 1).coerceAtLeast(1) - 1).coerceAtMost(viewport.columns - 1)
            }
            0x4a -> if (first == 2) clearScreen()
            0x4b -> clearLine(first)
            0x6d -> applySgr(parameters)
            0x73 -> { savedCursorRow = cursorRow; savedCursorColumn = cursorColumn }
            0x75 -> {
                cursorRow = savedCursorRow.coerceAtMost(rows.lastIndex)
                cursorColumn = savedCursorColumn.coerceAtMost(viewport.columns - 1)
            }
            0x68 -> if ((raw.firstOrNull()?.toInt() ?: 0) == 0x3f) setPrivateModes(parameters, true)
            0x6c -> if ((raw.firstOrNull()?.toInt() ?: 0) == 0x3f) setPrivateModes(parameters, false)
        }
    }

    private fun applySgr(rawParameters: List<Int>) {
        val parameters = rawParameters.ifEmpty { listOf(0) }
        var index = 0
        while (index < parameters.size) {
            when (val parameter = parameters[index]) {
                0 -> currentStyle = TerminalCellStyle()
                1 -> currentStyle = currentStyle.copy(bold = true)
                2 -> currentStyle = currentStyle.copy(dim = true)
                3 -> currentStyle = currentStyle.copy(italic = true)
                4 -> currentStyle = currentStyle.copy(underline = true)
                7 -> currentStyle = currentStyle.copy(inverse = true)
                22 -> currentStyle = currentStyle.copy(bold = false, dim = false)
                23 -> currentStyle = currentStyle.copy(italic = false)
                24 -> currentStyle = currentStyle.copy(underline = false)
                27 -> currentStyle = currentStyle.copy(inverse = false)
                in 30..37 -> currentStyle = currentStyle.copy(
                    foreground = TerminalCellColor.Indexed(parameter - 30),
                )
                39 -> currentStyle = currentStyle.copy(foreground = TerminalCellColor.Default)
                in 40..47 -> currentStyle = currentStyle.copy(
                    background = TerminalCellColor.Indexed(parameter - 40),
                )
                49 -> currentStyle = currentStyle.copy(background = TerminalCellColor.Default)
                in 90..97 -> currentStyle = currentStyle.copy(
                    foreground = TerminalCellColor.Indexed(parameter - 90 + 8),
                )
                in 100..107 -> currentStyle = currentStyle.copy(
                    background = TerminalCellColor.Indexed(parameter - 100 + 8),
                )
                38, 48 -> {
                    val parsed = extendedColor(parameters, index)
                    parsed.first?.let { color ->
                        currentStyle = if (parameter == 38) {
                            currentStyle.copy(foreground = color)
                        } else {
                            currentStyle.copy(background = color)
                        }
                    }
                    index += parsed.second
                }
            }
            index += 1
        }
    }

    private fun extendedColor(
        parameters: List<Int>,
        index: Int,
    ): Pair<TerminalCellColor?, Int> {
        return when (parameters.getOrNull(index + 1)) {
            5 -> parameters.getOrNull(index + 2)
                ?.takeIf { it in 0..255 }
                ?.let { TerminalCellColor.Indexed(it) to 2 }
                ?: (null to 0)
            2 -> {
                val values = (2..4).mapNotNull { parameters.getOrNull(index + it) }
                if (values.size == 3 && values.all { it in 0..255 }) {
                    TerminalCellColor.Rgb(values[0], values[1], values[2]) to 4
                } else {
                    null to 0
                }
            }
            else -> null to 0
        }
    }

    private fun setPrivateModes(modes: List<Int>, enabled: Boolean) {
        for (privateMode in modes) {
            when (privateMode) {
                1 -> applicationCursor = enabled
                9 -> updateMouseMode(TerminalMouseMode.PRESS, enabled)
                25 -> cursorVisible = enabled
                1000 -> updateMouseMode(TerminalMouseMode.PRESS_RELEASE, enabled)
                1002 -> updateMouseMode(TerminalMouseMode.BUTTON_MOTION, enabled)
                1003 -> updateMouseMode(TerminalMouseMode.ANY_MOTION, enabled)
                1005 -> updateMouseEncoding(TerminalMouseEncoding.UTF8, enabled)
                1006 -> updateMouseEncoding(TerminalMouseEncoding.SGR, enabled)
                1049 -> if (enabled) enterAlternateScreen() else leaveAlternateScreen()
                2004 -> bracketedPaste = enabled
            }
        }
    }

    private fun updateMouseMode(next: TerminalMouseMode, enabled: Boolean) {
        if (enabled) mouseMode = next else if (mouseMode == next) mouseMode = TerminalMouseMode.NONE
    }

    private fun updateMouseEncoding(next: TerminalMouseEncoding, enabled: Boolean) {
        if (enabled) {
            mouseEncoding = next
        } else if (mouseEncoding == next) {
            mouseEncoding = TerminalMouseEncoding.DEFAULT
        }
    }

    private fun enterAlternateScreen() {
        if (alternateScreen) return
        primaryScreen = StoredScreen(
            rows.map { it.toMutableList() }.toMutableList(),
            cursorRow,
            cursorColumn,
            savedCursorRow,
            savedCursorColumn,
        )
        rows.clear()
        repeat(viewport.rows) { rows += mutableListOf<BoundedTerminalCell>() }
        cursorRow = 0
        cursorColumn = 0
        savedCursorRow = 0
        savedCursorColumn = 0
        alternateScreen = true
        recalculateAccounting()
    }

    private fun leaveAlternateScreen() {
        if (!alternateScreen) return
        primaryScreen?.let { stored ->
            rows.clear()
            rows += stored.rows
            cursorRow = stored.cursorRow
            cursorColumn = stored.cursorColumn
            savedCursorRow = stored.savedCursorRow
            savedCursorColumn = stored.savedCursorColumn
        }
        primaryScreen = null
        alternateScreen = false
        recalculateAccounting()
    }

    private fun clearScreen() {
        rows.clear()
        repeat(viewport.rows) { rows += mutableListOf<BoundedTerminalCell>() }
        cursorRow = 0
        cursorColumn = 0
        recalculateAccounting()
    }

    private fun clearLine(clearMode: Int) {
        ensureCursorRow()
        val row = rows[cursorRow]
        when (clearMode) {
            1 -> {
                while (row.size <= cursorColumn) row += BoundedTerminalCell.blank()
                for (column in 0..cursorColumn) {
                    row[column] = BoundedTerminalCell.blank(currentStyle)
                }
            }
            2 -> row.clear()
            else -> if (cursorColumn < row.size) row.subList(cursorColumn, row.size).clear()
        }
        recalculateAccounting()
    }

    private fun resizeRows(
        target: MutableList<MutableList<BoundedTerminalCell>>,
        oldRows: Int,
        newRows: Int,
        columns: Int,
    ) {
        val visibleStart = (target.size - oldRows).coerceAtLeast(0)
        if (newRows < oldRows) {
            val start = minOf(target.size, visibleStart + newRows)
            val end = minOf(target.size, visibleStart + oldRows)
            if (start < end) target.subList(start, end).clear()
        } else {
            while (target.size < visibleStart + newRows) {
                target += mutableListOf<BoundedTerminalCell>()
            }
        }
        for (row in target) {
            if (row.size > columns) row.subList(columns, row.size).clear()
        }
    }

    private fun enforceLimits() {
        while (alternateScreen && rows.size > viewport.rows) evictOldestRow()
        var evictedForRows = false
        while (rows.size > viewport.rows &&
            (scrollbackRows > limits.maxScrollbackRows ||
                retainedCells > limits.maxRetainedCells ||
                graphemeBytes > limits.maxGraphemeBytes ||
                styleBytes > limits.maxStyleBytes ||
                accountedBytes > limits.maxModelBytes)
        ) {
            if (scrollbackRows > limits.maxScrollbackRows) evictedForRows = true
            evictOldestRow()
        }
        if (evictedForRows) truncation = TerminalTruncationReason.RETAINED_ROWS_LIMIT
        if (retainedCells > limits.maxRetainedCells ||
            graphemeBytes > limits.maxGraphemeBytes ||
            styleBytes > limits.maxStyleBytes ||
            accountedBytes > limits.maxModelBytes
        ) {
            truncateActiveRow()
            return
        }
    }

    private fun evictOldestRow() {
        val removed = rows.removeAt(0)
        retainedCells -= removed.size
        graphemeBytes -= rowGraphemeBytes(removed)
        styleBytes -= removed.size
        cursorRow = (cursorRow - 1).coerceAtLeast(0)
        savedCursorRow = (savedCursorRow - 1).coerceAtLeast(0)
    }

    private fun truncateActiveRow() {
        ensureCursorRow()
        truncation = when {
            retainedCells > limits.maxRetainedCells -> TerminalTruncationReason.RETAINED_CELLS_LIMIT
            graphemeBytes > limits.maxGraphemeBytes -> TerminalTruncationReason.GRAPHEME_ARENA_LIMIT
            styleBytes > limits.maxStyleBytes -> TerminalTruncationReason.STYLE_ARENA_LIMIT
            else -> TerminalTruncationReason.MODEL_LIMIT
        }
        val row = rows[cursorRow]
        while (row.isNotEmpty() &&
            (retainedCells > limits.maxRetainedCells ||
                graphemeBytes > limits.maxGraphemeBytes ||
                styleBytes > limits.maxStyleBytes ||
                accountedBytes > limits.maxModelBytes)
        ) {
            val removed = row.removeAt(row.lastIndex)
            retainedCells -= 1
            graphemeBytes -= removed.text.toByteArray().size
            styleBytes -= 1
        }
        if (row.size < viewport.columns) {
            row += BoundedTerminalCell("\ufffd", TerminalCellWidth.NARROW, currentStyle)
            retainedCells += 1
            graphemeBytes += 3
            styleBytes += 1
        }
        cursorColumn = row.size.coerceAtMost(viewport.columns - 1)
    }

    private fun recalculateAccounting() {
        retainedCells = rows.sumOf { it.size }
        graphemeBytes = rows.sumOf { row -> row.sumOf { it.text.toByteArray().size } }
        styleBytes = retainedCells
    }

    private fun updateAccounting(
        row: List<BoundedTerminalCell>,
        previousCount: Int,
        previousBytes: Int,
    ) {
        retainedCells += row.size - previousCount
        graphemeBytes += rowGraphemeBytes(row) - previousBytes
        styleBytes += row.size - previousCount
    }

    private fun rowGraphemeBytes(row: List<BoundedTerminalCell>) =
        row.sumOf { it.text.toByteArray().size }

    private fun renderLine(row: List<BoundedTerminalCell>) =
        row.filter { it.width != TerminalCellWidth.CONTINUATION }.joinToString("") { it.text }

    private fun paddedRow(row: List<BoundedTerminalCell>): List<BoundedTerminalCell> {
        if (row.size >= viewport.columns) return row.take(viewport.columns)
        return row + List(viewport.columns - row.size) { BoundedTerminalCell.blank() }
    }

    private val scrollbackRows get() = (rows.size - viewport.rows).coerceAtLeast(0)
    private val parserCarryBytes: Int
        get() = when (val current = mode) {
            is ParserMode.Csi -> current.bytes.size
            is ParserMode.Osc -> current.count
            else -> 0
        }.saturatedAdd(utf8Carry.size)
    private val accountedBytes: Int
        get() = graphemeBytes
            .saturatedAdd(styleBytes)
            .saturatedAdd(retainedCells.saturatedMultiply(16))
            .saturatedAdd(parserCarryBytes)

    private fun csiParameters(bytes: List<Byte>): List<Int> =
        bytes.toByteArray().decodeToString()
            .trimStart('?', '<', '>', '!')
            .split(';')
            .map { it.toIntOrNull() ?: 0 }

    private fun utf8Length(byte: Int) = when (byte) {
        in 0x00..0x7f -> 1
        in 0xc2..0xdf -> 2
        in 0xe0..0xef -> 3
        in 0xf0..0xf4 -> 4
        else -> 0
    }
}

private fun Int.saturatedAdd(value: Int): Int =
    if (this > Int.MAX_VALUE - value) Int.MAX_VALUE else this + value

private fun Int.saturatedMultiply(value: Int): Int =
    if (this != 0 && value > Int.MAX_VALUE / this) Int.MAX_VALUE else this * value
