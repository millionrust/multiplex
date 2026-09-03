package com.termirust.mobile.controller

import java.net.URI

enum class TerminalInputKey {
    TEXT, SPACE, ENTER, BACKSPACE, TAB, ESCAPE,
    UP, DOWN, LEFT, RIGHT, HOME, END, INSERT, DELETE, PAGEUP, PAGEDOWN,
}

data class TerminalInputModifiers(
    val shift: Boolean = false,
    val control: Boolean = false,
    val alt: Boolean = false,
)

data class TerminalSelectionPoint(val row: Int, val column: Int)

object ControllerTerminalFollowTarget {
    fun row(lines: List<String>, cursorRow: Int, scrollbackRows: Int): Int? {
        if (lines.isEmpty()) return null
        val cursor = (scrollbackRows + cursorRow).coerceIn(0, lines.lastIndex)
        val lastContent = lines.indexOfLast { it.isNotBlank() }.coerceAtLeast(0)
        return maxOf(cursor, lastContent)
    }

    fun firstVisibleRow(targetRow: Int, visibleRows: Int): Int =
        (targetRow - visibleRows.coerceAtLeast(1) + 1).coerceAtLeast(0)
}

object ControllerTerminalCursor {
    fun column(
        rowIndex: Int,
        cells: List<BoundedTerminalCell>,
        cursorRow: Int,
        cursorColumn: Int,
        scrollbackRows: Int,
        visible: Boolean,
    ): Int? {
        if (!visible || rowIndex != (scrollbackRows + cursorRow).coerceAtLeast(0)) return null
        val column = cursorColumn.coerceAtLeast(0)
        return if (cells.getOrNull(column)?.width == TerminalCellWidth.CONTINUATION) {
            (column - 1).coerceAtLeast(0)
        } else {
            column
        }
    }
}

object ControllerTerminalLayout {
    fun usesFocusedLandscape(isLandscape: Boolean, keyboardPresented: Boolean): Boolean =
        isLandscape && keyboardPresented
}

object ControllerTerminalWidth {
    const val DESKTOP_COLUMNS = 80

    fun columns(fitting: Int, usesDesktopWidth: Boolean): Int =
        if (usesDesktopWidth) maxOf(fitting, DESKTOP_COLUMNS) else fitting
}

object TerminalInteraction {
    const val MAX_PASTE_BYTES = 256 * 1_024
    const val PASTE_CONFIRMATION_BYTES = 4 * 1_024
    const val MAX_URL_BYTES = 2_048
    const val MAX_URLS = 32
    private val bracketStart = byteArrayOf(0x1b, 0x5b, 0x32, 0x30, 0x30, 0x7e)
    private val bracketEnd = byteArrayOf(0x1b, 0x5b, 0x32, 0x30, 0x31, 0x7e)

    fun encode(
        key: TerminalInputKey,
        text: String? = null,
        modifiers: TerminalInputModifiers = TerminalInputModifiers(),
        applicationCursor: Boolean = false,
    ): ByteArray? {
        var bytes = if (modifiers.control) {
            byteArrayOf(controlByte(key, text) ?: return null)
        } else {
            when (key) {
                TerminalInputKey.ENTER -> byteArrayOf(0x0d)
                TerminalInputKey.BACKSPACE -> byteArrayOf(0x7f)
                TerminalInputKey.TAB -> if (modifiers.shift) "\u001b[Z".encodeToByteArray() else byteArrayOf(0x09)
                TerminalInputKey.ESCAPE -> byteArrayOf(0x1b)
                TerminalInputKey.UP -> cursor('A', applicationCursor)
                TerminalInputKey.DOWN -> cursor('B', applicationCursor)
                TerminalInputKey.RIGHT -> cursor('C', applicationCursor)
                TerminalInputKey.LEFT -> cursor('D', applicationCursor)
                TerminalInputKey.HOME -> cursor('H', applicationCursor)
                TerminalInputKey.END -> cursor('F', applicationCursor)
                TerminalInputKey.INSERT -> "\u001b[2~".encodeToByteArray()
                TerminalInputKey.DELETE -> "\u001b[3~".encodeToByteArray()
                TerminalInputKey.PAGEUP -> "\u001b[5~".encodeToByteArray()
                TerminalInputKey.PAGEDOWN -> "\u001b[6~".encodeToByteArray()
                TerminalInputKey.SPACE -> byteArrayOf(0x20)
                TerminalInputKey.TEXT -> text?.takeIf(String::isNotEmpty)?.encodeToByteArray() ?: return null
            }
        }
        if (modifiers.alt) bytes = byteArrayOf(0x1b) + bytes
        return bytes
    }

    fun encodeCommittedText(text: String, modifiers: TerminalInputModifiers): ByteArray {
        val result = mutableListOf<Byte>()
        text.codePoints().forEach { codePoint ->
            val value = String(Character.toChars(codePoint))
            val key = if (value == " ") TerminalInputKey.SPACE else TerminalInputKey.TEXT
            encode(key, value, modifiers)?.forEach(result::add)
        }
        return result.toByteArray()
    }

    fun normalizePaste(text: String) = text.replace("\r\n", "\n").encodeToByteArray()

    fun pasteRequiresConfirmation(bytes: ByteArray) =
        bytes.size > PASTE_CONFIRMATION_BYTES ||
            bytes.contains(0x0a.toByte()) || bytes.contains(0x0d.toByte())

    fun preparePaste(bytes: ByteArray, bracketed: Boolean) =
        if (bracketed) bracketStart + bytes + bracketEnd else bytes.copyOf()

    fun maximumPastePayload(bracketed: Boolean) =
        MAX_PASTE_BYTES - if (bracketed) bracketStart.size + bracketEnd.size else 0

    fun selectionText(
        rows: List<List<BoundedTerminalCell>>,
        start: TerminalSelectionPoint,
        end: TerminalSelectionPoint,
    ): String {
        if (start.row > end.row || start.row !in rows.indices ||
            (start.row == end.row && start.column >= end.column)
        ) return ""
        return (start.row..end.row.coerceAtMost(rows.lastIndex)).joinToString("\n") { rowIndex ->
            val lower = if (rowIndex == start.row) start.column else 0
            val upper = if (rowIndex == end.row) end.column else Int.MAX_VALUE
            var column = 0
            buildString {
                for (cell in rows[rowIndex]) {
                    if (cell.width == TerminalCellWidth.CONTINUATION) continue
                    if (column in lower until upper) append(cell.text)
                    column += cell.width.columns
                }
            }
        }
    }

    fun visibleHttpUrls(text: String): List<String> {
        val urls = mutableListOf<String>()
        for (token in text.split(Regex("\\s+"))) {
            val starts = listOf(token.indexOf("https://"), token.indexOf("http://")).filter { it >= 0 }
            val start = starts.minOrNull() ?: continue
            val candidate = token.substring(start).trimEnd('.', ',', ';', ':', '!', '?', ')', ']', '}')
            val uri = runCatching { URI(candidate) }.getOrNull() ?: continue
            if (candidate.toByteArray().size <= MAX_URL_BYTES && candidate.all { it.code < 128 } &&
                candidate.none { it in "'\"<>" } && uri.scheme in setOf("http", "https") &&
                !uri.host.isNullOrEmpty()
            ) {
                urls += candidate
                if (urls.size == MAX_URLS) break
            }
        }
        return urls
    }

    private fun cursor(final: Char, application: Boolean) =
        byteArrayOf(0x1b, if (application) 0x4f else 0x5b, final.code.toByte())

    private fun controlByte(key: TerminalInputKey, text: String?): Byte? = when (key) {
        TerminalInputKey.SPACE -> 0
        TerminalInputKey.ENTER -> 0x0d
        TerminalInputKey.BACKSPACE -> 0x7f
        else -> {
            val value = text?.singleOrNull()?.code ?: return null
            val lower = if (value in 'A'.code..'Z'.code) value + 0x20 else value
            when (lower) {
                in 'a'.code..'z'.code -> (lower and 0x1f).toByte()
                '2'.code, '@'.code -> 0
                '3'.code, '['.code -> 27
                '4'.code, '\\'.code -> 28
                '5'.code, ']'.code -> 29
                '6'.code, '^'.code -> 30
                '7'.code, '_'.code, '/'.code -> 31
                else -> null
            }
        }
    }
}

data class TerminalImeState(private var markedText: String = "") {
    fun update(text: String) { markedText = text }
    fun cancel() { markedText = "" }
    fun commit(text: String): ByteArray? {
        markedText = ""
        return text.takeIf(String::isNotEmpty)?.encodeToByteArray()
    }
    fun finish(): ByteArray? {
        val committed = markedText
        markedText = ""
        return committed.takeIf(String::isNotEmpty)?.encodeToByteArray()
    }
}
