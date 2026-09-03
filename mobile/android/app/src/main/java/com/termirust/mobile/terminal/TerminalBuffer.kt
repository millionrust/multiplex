package com.termirust.mobile.terminal

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.io.ByteArrayOutputStream
import kotlin.math.max
import kotlin.math.min

class TerminalBuffer(private val maxLines: Int = 2_000) {
    private val _lines = MutableStateFlow<List<String>>(emptyList())
    val lines: StateFlow<List<String>> = _lines

    private val transcript = ByteArrayOutputStream()
    private val fallback = FallbackTerminalBuffer(maxLines)
    private var nativeUnavailable = false
    private var columns = 80
    private var rows = 24

    fun append(text: String) {
        append(text.encodeToByteArray())
    }

    fun append(bytes: ByteArray) {
        if (bytes.isEmpty()) {
            return
        }
        if (renderWithNative(bytes)) {
            return
        }
        fallback.append(bytes.decodeToString())
        _lines.value = fallback.lines()
    }

    fun resize(columns: Int, rows: Int) {
        val nextColumns = columns.coerceAtLeast(1)
        val nextRows = rows.coerceAtLeast(1)
        if (this.columns == nextColumns && this.rows == nextRows) {
            return
        }
        this.columns = nextColumns
        this.rows = nextRows
        if (!nativeUnavailable) {
            renderWithNative(ByteArray(0))
        }
    }

    fun clear() {
        transcript.reset()
        nativeUnavailable = false
        fallback.clear()
        _lines.value = emptyList()
    }

    private fun renderWithNative(newBytes: ByteArray): Boolean {
        if (nativeUnavailable) {
            return false
        }
        if (newBytes.isNotEmpty()) {
            transcript.write(newBytes)
        }
        val rendered = NativeMobileTerminal.renderUtf8OrNull(
            input = transcript.toByteArray(),
            columns = columns,
            rows = rows,
            scrollbackRows = maxLines,
        )
        if (rendered == null) {
            nativeUnavailable = true
            return false
        }
        _lines.value = rendered
            .decodeToString()
            .split('\n')
            .ifEmpty { listOf("") }
        return true
    }
}

private class FallbackTerminalBuffer(private val maxLines: Int) {
    private val rows = mutableListOf<StringBuilder>()
    private var row = 0
    private var column = 0

    fun append(text: String) {
        var index = 0
        while (index < text.length) {
            val char = text[index]
            if (char == '\u001B') {
                index = consumeEscape(text, index + 1)
                continue
            }
            when (char) {
                '\r' -> column = 0
                '\n' -> newLine()
                '\b' -> column = max(0, column - 1)
                else -> write(char)
            }
            index += 1
        }
    }

    fun clear() {
        rows.clear()
        row = 0
        column = 0
    }

    fun lines(): List<String> {
        trimScrollback()
        return rows
            .take(min(rows.size, maxLines))
            .map { it.toString() }
    }

    private fun write(char: Char) {
        ensureRow()
        val current = rows[row]
        while (current.length < column) {
            current.append(' ')
        }
        if (column == current.length) {
            current.append(char)
        } else {
            current.setCharAt(column, char)
        }
        column += 1
    }

    private fun newLine() {
        row += 1
        column = 0
        ensureRow()
        trimScrollback()
    }

    private fun ensureRow() {
        while (rows.size <= row) {
            rows.add(StringBuilder())
        }
    }

    private fun consumeEscape(text: String, start: Int): Int {
        if (start >= text.length) {
            return start
        }
        if (text[start] != '[') {
            return start
        }

        var index = start + 1
        val parameters = StringBuilder()
        while (index < text.length) {
            val char = text[index]
            if (char in '@'..'~') {
                handleCsi(parameters.toString(), char)
                return index + 1
            }
            parameters.append(char)
            index += 1
        }
        return text.length
    }

    private fun handleCsi(parameters: String, final: Char) {
        val first = parameters
            .split(';')
            .firstOrNull()
            ?.filter { it.isDigit() }
            ?.toIntOrNull()
        when (final) {
            'm' -> Unit
            'H', 'f' -> {
                row = 0
                column = 0
                ensureRow()
            }
            'J' -> if (first == null || first == 2) {
                rows.clear()
                row = 0
                column = 0
            }
            'K' -> {
                ensureRow()
                val current = rows[row]
                if (column < current.length) {
                    current.delete(column, current.length)
                }
            }
            'C' -> column += first ?: 1
            'D' -> column = max(0, column - (first ?: 1))
            'A' -> row = max(0, row - (first ?: 1))
            'B' -> {
                row += first ?: 1
                ensureRow()
            }
        }
    }

    private fun trimScrollback() {
        if (rows.size <= maxLines) {
            return
        }
        val remove = rows.size - maxLines
        repeat(remove) { rows.removeAt(0) }
        row = max(0, row - remove)
    }
}
